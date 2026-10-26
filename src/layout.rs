//! Places equations into rows of `width` cells. Placement is deterministic:
//! ties between rows are broken by the order in which the rows last changed.
use crate::{
    certificate::{Binding, Group, Layout, Network, Placement, Selector, Slot},
    contract::{MAX_CELLS, MAX_ROWS, MAX_WIDTH, MIN_ROWS, MIN_WIDTH},
    ensure, ensure_within_limit, network, Result,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// How equations are assigned to rows. Only the circuit's structure is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Strategy {
    /// Put each equation in the fullest row that still has room for it.
    Mixed,
    /// Prefer a row that already holds some of the equation's wires.
    Local,
    /// First place each group of wire-connected equations that fits in one row
    /// together, then continue as `Local`.
    Components,
    /// Give each shape its own band of columns, filled from the first row down,
    /// so that the equations of a shape share few slots and selectors. The
    /// trace has exactly `minRows` rows (when they suffice) and as many columns
    /// as the bands need; `options.width` only bounds each equation. Wires are
    /// not kept together, so they occupy more cells.
    Columns,
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
/// Place every equation of `network` and bind the public values.
///
/// Equations are grouped by canonical shape. `Strategy::Columns` places them
/// in bands of columns (see [`place_in_bands`]); the other strategies fill
/// rows of `options.width` cells (see [`place_in_rows`]).
pub fn pack(network: &Network, config: &LayoutConfig) -> Result<Layout> {
    let (width, strategy, min_rows) = (config.options.width, config.strategy, config.min_rows);
    ensure(
        min_rows >= MIN_ROWS && min_rows.is_power_of_two(),
        format_args!("minRows must be a power of two, at least {MIN_ROWS}"),
    )?;
    let (groups, entries) = shape_entries(network, width)?;
    let placed = if strategy == Strategy::Columns {
        place_in_bands(&entries, network, min_rows, width)?
    } else {
        place_in_rows(&entries, network, width, strategy)
    };
    let arithmetic_rows = placed
        .placements
        .iter()
        .map(|placement| placement.row + 1)
        .max()
        .unwrap_or(0);
    let rows = placed.used_rows.next_power_of_two().max(min_rows);
    ensure_within_limit(
        rows <= MAX_ROWS && rows.saturating_mul(placed.width) <= MAX_CELLS,
        format_args!("layout exceeds {MAX_ROWS} rows or {MAX_CELLS} cells"),
    )?;
    let (slots, selectors) = slots_and_selectors(&placed.placements);
    Ok(Layout {
        min_rows,
        width: placed.width,
        rows,
        used_rows: placed.used_rows,
        arithmetic_rows,
        groups,
        placements: placed.placements,
        slots,
        selectors,
        public_bindings: placed.public_bindings,
        requires_copy: placed.requires_copy,
    })
}

/// Equations and public values placed in a trace of `width` columns.
struct Placed {
    width: usize,
    placements: Vec<Placement>,
    public_bindings: Vec<Binding>,
    /// Rows holding an equation or a public cell, at least 1.
    used_rows: usize,
    requires_copy: bool,
}

/// Fill rows of `width` cells. With `Strategy::Components`, each connected
/// group of wires that fits in one row is placed first, together with all its
/// equations. Remaining equations with several wires go widest first: `Local`
/// and `Components` prefer a row already holding their wires, `Mixed` takes
/// the fullest row with room. Single-wire equations join the row where their
/// wire first appears; public wires that appear in no equation get a cell of
/// their own.
fn place_in_rows(entries: &[Entry], network: &Network, width: usize, strategy: Strategy) -> Placed {
    let mut placer = Placer::new(width, network.wires);
    let mut placements = vec![];
    let mut handled = BTreeSet::new();
    if strategy == Strategy::Components {
        place_components(
            &mut placer,
            entries,
            network.wires,
            &mut placements,
            &mut handled,
        );
    }
    place_multi_wire(&mut placer, entries, &handled, strategy, &mut placements);
    for entry in entries
        .iter()
        .filter(|entry| !handled.contains(&entry.index) && entry.wires.len() == 1)
    {
        let row = placer.first[entry.wires[0] as usize]
            .map_or_else(|| placer.best_fit(1), |(row, _)| row);
        placements.push(entry.place(&mut placer, row));
    }
    // Constant equations occupy no cells; they need only some row to exist.
    for entry in entries.iter().filter(|entry| entry.wires.is_empty()) {
        if placer.rows.is_empty() {
            placer.fresh();
        }
        placements.push(entry.at(0, vec![]));
    }
    let public_bindings = bind_publics(&mut placer, network.publics);
    Placed {
        width,
        placements,
        public_bindings,
        used_rows: placer.rows.len().max(1),
        requires_copy: placer.locations.iter().any(|r| r.len() > 1),
    }
}

/// A free rectangle of the trace: `width` columns from `column` and `height`
/// rows from `top`.
struct Area {
    column: usize,
    top: usize,
    width: usize,
    height: usize,
}

/// Consecutive rows `top..top + count` of the columns from `column` on.
struct Piece {
    column: usize,
    top: usize,
    count: usize,
}

/// The trace, `rows` rows high, divided into bands of columns as runs are
/// placed. Free space is kept as rectangles cut from the bands.
struct Bands {
    rows: usize,
    free: Vec<Area>,
    /// Columns taken by all bands.
    width: usize,
    /// Rows taken in any band.
    used_rows: usize,
}
impl Bands {
    fn new(rows: usize) -> Self {
        Bands {
            rows,
            free: vec![],
            width: 0,
            used_rows: 0,
        }
    }
    /// Take `count` rows of `width` columns, in pieces. They go in the
    /// narrowest free rectangle at least `width` wide, preferring one with
    /// room for all of them (then the first); a rectangle too short takes what
    /// fits and the rest goes elsewhere. Using a rectangle leaves the columns
    /// to the right of the piece and the rows below it free. Without a fitting
    /// rectangle, a new band is added.
    fn take(&mut self, width: usize, mut count: usize) -> Vec<Piece> {
        let mut pieces = vec![];
        while count > 0 {
            let fitting = (0..self.free.len())
                .filter(|i| self.free[*i].width >= width)
                .min_by_key(|i| {
                    let area = &self.free[*i];
                    (area.width - width, area.height < count, *i)
                });
            let area = match fitting {
                Some(index) => self.free.swap_remove(index),
                None => {
                    self.width += width;
                    Area {
                        column: self.width - width,
                        top: 0,
                        width,
                        height: self.rows,
                    }
                }
            };
            let taken = count.min(area.height);
            pieces.push(Piece {
                column: area.column,
                top: area.top,
                count: taken,
            });
            count -= taken;
            self.used_rows = self.used_rows.max(area.top + taken);
            if area.width > width {
                self.free.push(Area {
                    column: area.column + width,
                    top: area.top,
                    width: area.width - width,
                    height: area.height,
                });
            }
            if area.height > taken {
                self.free.push(Area {
                    column: area.column,
                    top: area.top + taken,
                    width,
                    height: area.height - taken,
                });
            }
        }
        pieces
    }
}

/// The first cell of each wire and its number of cells.
struct Occupied {
    first: Vec<Option<(usize, usize)>>,
    cells: Vec<usize>,
}
impl Occupied {
    fn add(&mut self, wire: u32, row: usize, column: usize) {
        self.first[wire as usize].get_or_insert((row, column));
        self.cells[wire as usize] += 1;
    }
}

/// Equations placed together in one row, sharing the cells of their common wires.
struct Bundle<'a> {
    members: Vec<&'a Entry>,
    /// Distinct wires in order of first appearance; wire `i` goes to the
    /// band's column `i`.
    wires: Vec<u32>,
    /// Shape and wire positions of each member. Bundles with equal keys
    /// differ only in which wires they use, so they can share slots.
    key: Vec<(usize, Vec<u32>)>,
}
impl<'a> Bundle<'a> {
    fn single(entry: &'a Entry) -> Self {
        Bundle {
            members: vec![entry],
            wires: entry.wires.clone(),
            key: vec![(entry.shape, (0..entry.wires.len() as u32).collect())],
        }
    }
    /// The positions of the wires `self` shares with `other`, as pairs of
    /// positions in `self` and in `other`.
    fn shared(&self, other: &Bundle) -> Vec<(u32, u32)> {
        let positions: BTreeMap<_, _> = other.wires.iter().zip(0u32..).collect();
        self.wires
            .iter()
            .zip(0u32..)
            .filter_map(|(wire, i)| positions.get(wire).map(|j| (i, *j)))
            .collect()
    }
    /// `self` followed by `other`, sharing the cells of common wires.
    fn join(mut self, other: Bundle<'a>) -> Self {
        let mut positions: BTreeMap<u32, u32> = self.wires.iter().copied().zip(0..).collect();
        for wire in &other.wires {
            if !positions.contains_key(wire) {
                positions.insert(*wire, self.wires.len() as u32);
                self.wires.push(*wire);
            }
        }
        for (shape, members) in other.key {
            let moved = members
                .iter()
                .map(|p| positions[&other.wires[*p as usize]])
                .collect();
            self.key.push((shape, moved));
        }
        self.members.extend(other.members);
        self
    }
}

/// Most rounds of joining bundles that share wires; joining stops earlier when
/// a round joins nothing.
const BUNDLE_ROUNDS: usize = 64;
/// Wires held by more bundles than this are not used to find pairs to join.
const MAX_HOLDERS: usize = 16;
/// Relative proving costs used to decide joins, in slots: a column of the
/// trace (with its share of the copy argument) costs about `SLOTS_PER_COLUMN`
/// slot constraints, and a selector column about `SLOTS_PER_SELECTOR`. The
/// values come from timing a STARK prover; see `docs/SCALE.md`.
const SLOTS_PER_COLUMN: f64 = 22.0;
const SLOTS_PER_SELECTOR: f64 = 0.5;

/// Weighted cost of `count` bundles of a key with `members` equations: one
/// slot per member and run of at most `rows` bundles, and a selector for a
/// run that does not fill its band.
fn run_cost(count: usize, members: usize, rows: usize) -> f64 {
    let runs = count.div_ceil(rows);
    let partial = usize::from(count % rows != 0);
    (runs * members) as f64 + partial as f64 * SLOTS_PER_SELECTOR
}

/// Group equations into bundles of at most `max_width` wires. Each round
/// collects, for every pair of bundles that share a wire, the pattern
/// `(key, key, shared positions)`. Going from the patterns that save the most
/// cells to the least, it joins the disjoint pairs of a pattern when that
/// lowers the weighted cost of the runs involved (see [`run_cost`]) by more
/// than the columns the shared cells save.
fn bundles<'a>(
    entries: &[&'a Entry],
    wires: u32,
    max_width: usize,
    rows: usize,
) -> Vec<Bundle<'a>> {
    let mut bundles: Vec<Bundle> = entries.iter().map(|entry| Bundle::single(entry)).collect();
    for _ in 0..BUNDLE_ROUNDS {
        let mut ids = BTreeMap::new();
        let keys: Vec<usize> = bundles
            .iter()
            .map(|bundle| {
                let next = ids.len();
                *ids.entry(bundle.key.clone()).or_insert(next)
            })
            .collect();
        let mut holders = vec![vec![]; wires as usize];
        for (index, bundle) in bundles.iter().enumerate() {
            for wire in &bundle.wires {
                holders[*wire as usize].push(index);
            }
        }
        let mut pairs = BTreeSet::new();
        for held in holders
            .iter()
            .filter(|held| (2..=MAX_HOLDERS).contains(&held.len()))
        {
            for (i, a) in held.iter().enumerate() {
                for b in &held[i + 1..] {
                    // Orient each pair by key so that equal patterns compare equal.
                    pairs.insert(if (keys[*a], *a) <= (keys[*b], *b) {
                        (*a, *b)
                    } else {
                        (*b, *a)
                    });
                }
            }
        }
        let mut patterns: BTreeMap<_, Vec<(usize, usize)>> = BTreeMap::new();
        for (a, b) in pairs {
            let shared = bundles[a].shared(&bundles[b]);
            if bundles[a].wires.len() + bundles[b].wires.len() - shared.len() <= max_width {
                patterns
                    .entry((keys[a], keys[b], shared))
                    .or_default()
                    .push((a, b));
            }
        }
        let mut patterns: Vec<_> = patterns.into_iter().collect();
        patterns
            .sort_by_key(|((_, _, shared), pairs)| std::cmp::Reverse(pairs.len() * shared.len()));
        // Instances and members of each key, kept up to date as pairs join.
        let mut count = vec![0usize; ids.len()];
        let mut members = vec![0usize; ids.len()];
        for (bundle, key) in bundles.iter().zip(&keys) {
            count[*key] += 1;
            members[*key] = bundle.key.len();
        }
        let mut joined = vec![None; bundles.len()];
        let mut joins = 0;
        for ((a_key, b_key, shared), pairs) in &patterns {
            let mut taken = BTreeSet::new();
            let chosen: Vec<_> = pairs
                .iter()
                .filter(|(a, b)| {
                    joined[*a].is_none()
                        && joined[*b].is_none()
                        && taken.insert(*a)
                        && taken.insert(*b)
                })
                .collect();
            let (a_key, b_key) = (*a_key, *b_key);
            let mut after = count.clone();
            after[a_key] -= chosen.len();
            after[b_key] -= chosen.len();
            // The cost of the runs of both keys, counted once if they are equal.
            let cost = |counts: &[usize]| {
                let b_cost = if a_key == b_key {
                    0.0
                } else {
                    run_cost(counts[b_key], members[b_key], rows)
                };
                run_cost(counts[a_key], members[a_key], rows) + b_cost
            };
            let saved_columns = (chosen.len() * shared.len()) as f64 / rows as f64;
            let change = cost(&after)
                + run_cost(chosen.len(), members[a_key] + members[b_key], rows)
                - cost(&count)
                - saved_columns * SLOTS_PER_COLUMN;
            if chosen.is_empty() || change >= 0.0 {
                continue;
            }
            count = after;
            for (a, b) in chosen {
                joined[*a] = Some(*b);
                joined[*b] = Some(*a);
                joins += 1;
            }
        }
        if joins == 0 {
            break;
        }
        let mut pending: Vec<Option<Bundle>> = bundles.into_iter().map(Some).collect();
        let mut next = vec![];
        for index in 0..pending.len() {
            let Some(bundle) = pending[index].take() else {
                continue;
            };
            next.push(match joined[index] {
                Some(partner) => {
                    bundle.join(pending[partner].take().expect("partner not yet joined"))
                }
                None => bundle,
            });
        }
        bundles = next;
    }
    bundles
}

/// Place the equations in bundles (see [`bundles`]), one bundle per row in
/// columns as wide as the bundle. The bundles of one key form a run, placed
/// top down in one or more pieces of the free space (see [`Bands::take`]);
/// each member of a piece shares one slot. Runs go widest first, then longest
/// first. Public wires that appear in no equation get a
/// cell of their own, and each public value binds to the first cell its wire
/// was given.
fn place_in_bands(
    entries: &[Entry],
    network: &Network,
    rows: usize,
    max_width: usize,
) -> Result<Placed> {
    let with_wires: Vec<&Entry> = entries
        .iter()
        .filter(|entry| !entry.wires.is_empty())
        .collect();
    let mut by_key: BTreeMap<Vec<(usize, Vec<u32>)>, Vec<Bundle>> = BTreeMap::new();
    for bundle in bundles(&with_wires, network.wires, max_width, rows) {
        by_key.entry(bundle.key.clone()).or_default().push(bundle);
    }
    let mut runs: Vec<_> = by_key.into_values().collect();
    runs.sort_by_key(|run| {
        (
            std::cmp::Reverse(run[0].wires.len()),
            std::cmp::Reverse(run.len()),
            run[0].members[0].index,
        )
    });
    let mut bands = Bands::new(rows);
    let mut occupied = Occupied {
        first: vec![None; network.wires as usize],
        cells: vec![0; network.wires as usize],
    };
    let mut placements = vec![];
    for run in &runs {
        let width = run[0].wires.len();
        let mut bundles = run.iter();
        for piece in bands.take(width, run.len()) {
            for (row, bundle) in (piece.top..piece.top + piece.count).zip(bundles.by_ref()) {
                for (position, wire) in bundle.wires.iter().enumerate() {
                    occupied.add(*wire, row, piece.column + position);
                }
                for (member, (_, positions)) in bundle.members.iter().zip(&bundle.key) {
                    let columns = positions
                        .iter()
                        .map(|p| piece.column + *p as usize)
                        .collect();
                    placements.push(member.at(row, columns));
                }
            }
        }
    }
    // Constant equations occupy no cells; they need only some row to exist.
    for entry in entries.iter().filter(|entry| entry.wires.is_empty()) {
        placements.push(entry.at(0, vec![]));
    }
    let unplaced: Vec<u32> = (1..=network.publics)
        .filter(|wire| occupied.first[*wire as usize].is_none())
        .collect();
    let mut wires = unplaced.iter();
    for piece in bands.take(1, unplaced.len()) {
        for (row, wire) in (piece.top..piece.top + piece.count).zip(wires.by_ref()) {
            occupied.add(*wire, row, piece.column);
        }
    }
    let public_bindings = (0..network.publics)
        .map(|public| {
            let wire = public + 1;
            let (row, col) = occupied.first[wire as usize].expect("every public wire has a cell");
            Binding {
                wire,
                public,
                row,
                col,
            }
        })
        .collect();
    ensure_within_limit(
        bands.width <= MAX_WIDTH,
        format_args!(
            "the shapes need {} columns in {rows} rows, more than {MAX_WIDTH}; raise minRows",
            bands.width
        ),
    )?;
    Ok(Placed {
        width: bands.width.max(MIN_WIDTH),
        placements,
        public_bindings,
        used_rows: bands.used_rows.max(1),
        requires_copy: occupied.cells.iter().any(|count| *count > 1),
    })
}

/// An equation not placed yet: its index, shape group and wires.
struct Entry {
    index: usize,
    shape: usize,
    wires: Vec<u32>,
}
impl Entry {
    /// Put the entry's wires in `row` and return the placement.
    fn place(&self, placer: &mut Placer, row: usize) -> Placement {
        let columns = placer.assign(row, &self.wires);
        self.at(row, columns)
    }
    /// The placement of this entry at `row` and `columns`.
    fn at(&self, row: usize, columns: Vec<usize>) -> Placement {
        Placement {
            index: self.index,
            shape: self.shape,
            wires: self.wires.clone(),
            row,
            columns,
        }
    }
}

/// Canonical shape groups and one unplaced entry per equation, in equation order.
fn shape_entries(network: &Network, width: usize) -> Result<(Vec<Group>, Vec<Entry>)> {
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
        entries.push(Entry {
            index,
            shape: id,
            wires: shape.wires,
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
    entries: &[Entry],
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
            placements.push(entries[index].place(placer, row));
            handled.insert(index);
        }
    }
}

/// Place the remaining multi-wire equations, widest first.
fn place_multi_wire(
    placer: &mut Placer,
    entries: &[Entry],
    handled: &BTreeSet<usize>,
    strategy: Strategy,
    placements: &mut Vec<Placement>,
) {
    let mut rest: Vec<_> = entries
        .iter()
        .filter(|entry| !handled.contains(&entry.index) && entry.wires.len() > 1)
        .collect();
    rest.sort_by_key(|entry| {
        (
            std::cmp::Reverse(entry.wires.len()),
            entry.shape,
            entry.index,
        )
    });
    for entry in rest {
        let chosen = if strategy == Strategy::Mixed {
            None
        } else {
            local_row(placer, &entry.wires)
        };
        let row = chosen.unwrap_or_else(|| placer.best_fit(entry.wires.len()));
        placements.push(entry.place(placer, row));
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
