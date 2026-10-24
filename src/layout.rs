//! Places equations into rows of `width` cells. Placement is deterministic:
//! ties between rows are broken by the order in which the rows last changed.
use crate::{
    certificate::{Binding, Group, Layout, Network, Placement, Selector, Slot},
    contract::{MAX_CELLS, MAX_ROWS, MIN_ROWS},
    ensure, network, Result,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// How equations are assigned to rows. Only the circuit's structure is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Strategy {
    /// Put each equation in the fullest row that still has room for it.
    Mixed,
    /// Prefer a row that already holds some of the equation's wires.
    Local,
    /// First place each group of wire-connected equations that fits in one row
    /// together, then continue as `Local`.
    Components,
}
/// Settings for building a packed layout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayoutConfig {
    /// How source equations are rewritten with helpers.
    pub options: network::ConversionOptions,
    /// How equations are assigned to rows.
    pub strategy: Strategy,
    /// Minimum row count; a power of two, at least 64.
    pub min_rows: usize,
}
struct Row {
    /// Wire in each column; 0 marks a free cell.
    cells: Vec<u32>,
    /// Column of each wire in this row.
    by_wire: BTreeMap<u32, usize>,
    free: usize,
    /// Value of the placer's clock when the row last changed.
    stamp: u64,
}
struct Placer {
    width: usize,
    rows: Vec<Row>,
    /// `buckets[f]` holds `(stamp, row)` for every row with `f` free cells, so
    /// its first entry is the row that changed least recently.
    buckets: Vec<BTreeSet<(u64, usize)>>,
    /// Counter incremented on every row change.
    clock: u64,
    /// Rows holding each wire, in placement order.
    locations: Vec<Vec<usize>>,
    /// First `(row, column)` of each wire.
    first: Vec<Option<(usize, usize)>>,
}
impl Placer {
    fn new(width: usize, wires: u32) -> Self {
        Self {
            width,
            rows: vec![],
            buckets: vec![BTreeSet::new(); width + 1],
            clock: 0,
            locations: vec![vec![]; wires as usize],
            first: vec![None; wires as usize],
        }
    }
    /// Append an empty row.
    fn fresh(&mut self) -> usize {
        let row = self.rows.len();
        self.clock += 1;
        self.rows.push(Row {
            cells: vec![0; self.width],
            by_wire: BTreeMap::new(),
            free: self.width,
            stamp: self.clock,
        });
        self.buckets[self.width].insert((self.clock, row));
        row
    }
    /// The row with the fewest free cells that still has `n` free, or a new row.
    fn best_fit(&mut self, n: usize) -> usize {
        for free in n..=self.width {
            if let Some((_, row)) = self.buckets[free].first() {
                return *row;
            }
        }
        self.fresh()
    }
    /// Put `wires` in `row`, reusing a wire's cell if the row already holds it
    /// and otherwise taking the leftmost free cells. Returns each wire's column.
    fn assign(&mut self, row: usize, wires: &[u32]) -> Vec<usize> {
        let state = &mut self.rows[row];
        self.buckets[state.free].remove(&(state.stamp, row));
        let mut cursor = 0;
        let mut columns = vec![];
        for wire in wires {
            if let Some(column) = state.by_wire.get(wire) {
                columns.push(*column);
                continue;
            }
            while state.cells[cursor] != 0 {
                cursor += 1;
            }
            state.cells[cursor] = *wire;
            state.by_wire.insert(*wire, cursor);
            state.free -= 1;
            columns.push(cursor);
            self.locations[*wire as usize].push(row);
            self.first[*wire as usize].get_or_insert((row, cursor));
            cursor += 1;
        }
        self.clock += 1;
        state.stamp = self.clock;
        self.buckets[state.free].insert((state.stamp, row));
        columns
    }
}
/// Place every equation of `network` in rows of `width` cells.
///
/// Equations are grouped by canonical shape. With `Strategy::Components`, each
/// connected group of wires that fits in one row is placed first, together
/// with all its equations. Remaining equations with several wires go widest
/// first: `Local` and `Components` prefer a row already holding their wires,
/// `Mixed` takes the fullest row with room. Single-wire equations join the row
/// where their wire first appears; public wires that appear in no equation get
/// a cell of their own.
pub fn pack(network: &Network, config: &LayoutConfig) -> Result<Layout> {
    let (width, strategy, min_rows) = (config.options.width, config.strategy, config.min_rows);
    ensure(
        min_rows >= MIN_ROWS && min_rows.is_power_of_two(),
        format!("minRows must be a power of two, at least {MIN_ROWS}"),
    )?;
    let (groups, entries) = shape_entries(network, width)?;
    let mut placer = Placer::new(width, network.wires);
    let mut placements = vec![];
    let mut handled = BTreeSet::new();
    if strategy == Strategy::Components {
        place_components(
            &mut placer,
            &entries,
            network.wires,
            &mut placements,
            &mut handled,
        );
    }
    place_multi_wire(&mut placer, &entries, &handled, strategy, &mut placements);
    for entry in entries
        .iter()
        .filter(|entry| !handled.contains(&entry.index) && entry.wires.len() == 1)
    {
        let mut entry = entry.clone();
        let row = placer.first[entry.wires[0] as usize]
            .map_or_else(|| placer.best_fit(1), |(row, _)| row);
        entry.row = row;
        entry.columns = placer.assign(row, &entry.wires);
        placements.push(entry);
    }
    // Constant equations occupy no cells; they need only some row to exist.
    for entry in entries.iter().filter(|entry| entry.wires.is_empty()) {
        if placer.rows.is_empty() {
            placer.fresh();
        }
        placements.push(entry.clone());
    }
    let arithmetic_rows = placements
        .iter()
        .map(|placement| placement.row + 1)
        .max()
        .unwrap_or(0);
    let public_bindings = bind_publics(&mut placer, network.publics);
    let used_rows = placer.rows.len().max(1);
    let rows = used_rows.next_power_of_two().max(min_rows);
    ensure(
        rows <= MAX_ROWS && rows.saturating_mul(width) <= MAX_CELLS,
        format!("layout exceeds {MAX_ROWS} rows or {MAX_CELLS} cells"),
    )?;
    let (slots, selectors) = slots_and_selectors(&placements);
    Ok(Layout {
        min_rows,
        width,
        rows,
        used_rows,
        arithmetic_rows,
        groups,
        placements,
        slots,
        selectors,
        public_bindings,
        requires_copy: placer.locations.iter().any(|r| r.len() > 1),
    })
}

/// Canonical shape groups and one unplaced entry per equation, in equation order.
fn shape_entries(network: &Network, width: usize) -> Result<(Vec<Group>, Vec<Placement>)> {
    let mut groups = vec![];
    let mut ids = BTreeMap::new();
    let mut entries = vec![];
    for (index, equation) in network::equations(network).iter().enumerate() {
        let shape = network::shape(equation);
        ensure(shape.wires.len() <= width, "equation exceeds width")?;
        let id = *ids.entry(shape.key).or_insert_with(|| {
            let id = groups.len();
            groups.push(Group {
                id,
                width: shape.wires.len(),
                terms: shape.terms,
            });
            id
        });
        entries.push(Placement {
            index,
            shape: id,
            wires: shape.wires,
            row: 0,
            columns: vec![],
        });
    }
    Ok((groups, entries))
}

/// Representative of `w`'s set in the union-find forest `parent`, with path halving.
fn root(parent: &mut [u32], mut w: u32) -> u32 {
    while parent[w as usize] != w {
        parent[w as usize] = parent[parent[w as usize] as usize];
        w = parent[w as usize];
    }
    w
}

/// Place each group of wire-connected equations whose wires fit in one row,
/// largest first, and add those equations to `handled`.
fn place_components(
    placer: &mut Placer,
    entries: &[Placement],
    wires: u32,
    placements: &mut Vec<Placement>,
    handled: &mut BTreeSet<usize>,
) {
    let mut parent: Vec<_> = (0..wires).collect();
    let seen: BTreeSet<_> = entries
        .iter()
        .flat_map(|entry| entry.wires.iter().copied())
        .collect();
    for entry in entries {
        for wire in entry.wires.iter().skip(1) {
            let left = root(&mut parent, *wire);
            let right = root(&mut parent, entry.wires[0]);
            parent[left as usize] = right;
        }
    }
    let mut components: BTreeMap<u32, (Vec<u32>, Vec<usize>)> = BTreeMap::new();
    for wire in seen {
        let representative = root(&mut parent, wire);
        components.entry(representative).or_default().0.push(wire);
    }
    for entry in entries {
        if let Some(wire) = entry.wires.first() {
            let representative = root(&mut parent, *wire);
            components
                .get_mut(&representative)
                .expect("component")
                .1
                .push(entry.index);
        }
    }
    let mut components: Vec<_> = components.into_values().collect();
    components.sort_by_key(|(wire, _)| (std::cmp::Reverse(wire.len()), wire[0]));
    for (wires, indices) in components {
        if wires.len() > placer.width {
            continue;
        }
        let row = placer.best_fit(wires.len());
        placer.assign(row, &wires);
        for index in indices {
            let mut entry = entries[index].clone();
            entry.row = row;
            entry.columns = placer.assign(row, &entry.wires);
            handled.insert(index);
            placements.push(entry);
        }
    }
}

/// Place the remaining multi-wire equations, widest first.
fn place_multi_wire(
    placer: &mut Placer,
    entries: &[Placement],
    handled: &BTreeSet<usize>,
    strategy: Strategy,
    placements: &mut Vec<Placement>,
) {
    let mut rest: Vec<_> = entries
        .iter()
        .filter(|entry| !handled.contains(&entry.index) && entry.wires.len() > 1)
        .cloned()
        .collect();
    rest.sort_by_key(|entry| {
        (
            std::cmp::Reverse(entry.wires.len()),
            entry.shape,
            entry.index,
        )
    });
    for mut entry in rest {
        let chosen = if strategy == Strategy::Mixed {
            None
        } else {
            local_row(placer, &entry.wires)
        };
        let row = chosen.unwrap_or_else(|| placer.best_fit(entry.wires.len()));
        entry.row = row;
        entry.columns = placer.assign(row, &entry.wires);
        placements.push(entry);
    }
}

/// Number of rows `local_row` considers.
const LOCAL_CANDIDATES: usize = 32;

/// Among the first `LOCAL_CANDIDATES` rows found that hold some of `wires`, the one that has
/// room and shares the most wires; ties go to the row left with fewest free cells.
fn local_row(placer: &Placer, wires: &[u32]) -> Option<usize> {
    let mut seen = BTreeSet::new();
    wires
        .iter()
        .flat_map(|wire| &placer.locations[*wire as usize])
        .filter(|row| seen.insert(**row))
        .take(LOCAL_CANDIDATES)
        .filter_map(|&row| {
            let state = &placer.rows[row];
            let shared = wires
                .iter()
                .filter(|wire| state.by_wire.contains_key(wire))
                .count();
            let missing = wires.len() - shared;
            (missing <= state.free).then(|| (std::cmp::Reverse(shared), state.free - missing, row))
        })
        .min_by_key(|(shared, left_free, _)| (*shared, *left_free))
        .map(|(_, _, row)| row)
}

/// Bind each public value to the first cell of its wire, first giving a cell
/// to public wires that have none.
fn bind_publics(placer: &mut Placer, publics: u32) -> Vec<Binding> {
    let mut bindings = vec![];
    for public in 0..publics {
        let wire = public + 1;
        if placer.first[wire as usize].is_none() {
            let row = placer.best_fit(1);
            placer.assign(row, &[wire]);
        }
        let (row, col) = placer.first[wire as usize].expect("public assigned");
        bindings.push(Binding {
            wire,
            public,
            row,
            col,
        });
    }
    bindings
}

/// One slot per distinct `(shape, columns)` pair, active on the rows that use
/// it, and one selector per distinct set of active rows.
fn slots_and_selectors(placements: &[Placement]) -> (Vec<Slot>, Vec<Selector>) {
    let mut slots: Vec<Slot> = vec![];
    let mut keys = BTreeMap::new();
    for placement in placements {
        let key = (placement.shape, placement.columns.clone());
        let id = *keys.entry(key).or_insert_with(|| {
            let index = slots.len();
            slots.push(Slot {
                shape: placement.shape,
                columns: placement.columns.clone(),
                rows: vec![],
                selector: 0,
            });
            index
        });
        slots[id].rows.push(placement.row);
    }
    let mut selectors = vec![];
    let mut masks = BTreeMap::new();
    for slot in &mut slots {
        slot.rows.sort_unstable();
        slot.rows.dedup();
        let id = *masks.entry(slot.rows.clone()).or_insert_with(|| {
            let id = selectors.len();
            selectors.push(Selector {
                id,
                rows: slot.rows.clone(),
            });
            id
        });
        slot.selector = id;
    }
    (slots, selectors)
}
