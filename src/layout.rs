//! Places equations into the cells of the trace. The row strategies fill rows
//! of `width` cells, breaking ties by the order in which rows last changed;
//! the columns strategy joins equations into row groups and packs the row groups of
//! each kind, as stacks, into columns. Placement is deterministic.
use crate::{
    certificate::{Binding, Gate, GateType, Layout, Placement, RewrittenCircuit, Selector},
    contract::{MAX_CELLS, MAX_ROWS, MAX_WIDTH, MIN_ROWS, MIN_WIDTH},
    ensure, ensure_within_limit, rewrite, Result,
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
    /// Join equations that share wires into row groups, then place the row groups of
    /// each kind as a stack, one row group per row in the same columns, so that a
    /// stack needs few gates and selectors. The trace has exactly `minRows` rows
    /// and as many columns as the stacks need, at most `MAX_WIDTH`;
    /// `options.width` only bounds each equation, and `rowGroupWidth` each
    /// row group. Wires get more cells than with the row strategies.
    Columns,
}
/// Settings for building a packed layout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayoutConfig {
    /// How source equations are rewritten with auxiliary wires.
    pub options: rewrite::RewriteOptions,
    /// How equations are assigned to rows.
    pub strategy: Strategy,
    /// Minimum row count; a power of two, at least 64.
    pub min_rows: usize,
    /// Most wires of a row group of equations with `Strategy::Columns`, from
    /// `options.width` (the default) to `MAX_WIDTH`. Wider row groups let more
    /// equations share cells but may leave more of the trace empty; which is
    /// faster to prove depends on the circuit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_group_width: Option<usize>,
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
/// Place every equation of `rewritten` and bind the public values.
///
/// Equations are grouped by canonical gate type. `Strategy::Columns` places them
/// in stacks of row groups (see [`place_in_stacks`]); the other strategies fill rows
/// of `options.width` cells (see [`place_in_rows`]).
pub fn pack(rewritten: &RewrittenCircuit, config: &LayoutConfig) -> Result<Layout> {
    let (width, strategy, min_rows) = (config.options.width, config.strategy, config.min_rows);
    ensure(
        min_rows >= MIN_ROWS && min_rows.is_power_of_two(),
        format_args!("minRows must be a power of two, at least {MIN_ROWS}"),
    )?;
    ensure_within_limit(
        min_rows <= MAX_ROWS,
        format_args!("minRows exceeds {MAX_ROWS}"),
    )?;
    ensure(
        strategy == Strategy::Columns || config.row_group_width.is_none(),
        "rowGroupWidth applies only to the columns strategy",
    )?;
    let (gate_types, entries) = gate_entries(rewritten, width)?;
    let placed = if strategy == Strategy::Columns {
        let row_group_width = config.row_group_width.unwrap_or(width);
        ensure(
            (width..=MAX_WIDTH).contains(&row_group_width),
            format_args!("rowGroupWidth must be in options.width..={MAX_WIDTH}"),
        )?;
        place_in_stacks(&entries, rewritten, min_rows, row_group_width)?
    } else {
        place_in_rows(&entries, rewritten, width, strategy)
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
    let (gates, selectors) = gates_and_selectors(&placed.placements);
    Ok(Layout {
        min_rows,
        width: placed.width,
        rows,
        used_rows: placed.used_rows,
        arithmetic_rows,
        gate_types,
        placements: placed.placements,
        gates,
        selectors,
        public_bindings: placed.public_bindings,
        requires_permutation: placed.requires_permutation,
    })
}

/// Equations and public values placed in a trace of `width` columns.
struct Placed {
    width: usize,
    /// Columns in the permutation argument (SPEC.md, "Permutation argument"). Only the
    /// columns strategy counts them, to compare its packings; the row
    /// strategies set it to `width`.
    permutation_columns: usize,
    placements: Vec<Placement>,
    public_bindings: Vec<Binding>,
    /// Rows up to the last one holding an equation or a public cell, at least 1.
    used_rows: usize,
    requires_permutation: bool,
}

/// Fill rows of `width` cells. With `Strategy::Components`, each connected
/// group of wires that fits in one row is placed first, together with all its
/// equations. Remaining equations with several wires go widest first: `Local`
/// and `Components` prefer a row already holding their wires, `Mixed` takes
/// the fullest row with room. Single-wire equations join the row where their
/// wire first appears; public wires that appear in no equation get a cell of
/// their own.
fn place_in_rows(
    entries: &[Entry],
    rewritten: &RewrittenCircuit,
    width: usize,
    strategy: Strategy,
) -> Placed {
    let mut placer = Placer::new(width, rewritten.wires);
    let mut placements = vec![];
    let mut handled = BTreeSet::new();
    if strategy == Strategy::Components {
        place_components(
            &mut placer,
            entries,
            rewritten.wires,
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
    let public_bindings = bind_publics(&mut placer, rewritten.publics);
    Placed {
        width,
        permutation_columns: width,
        placements,
        public_bindings,
        used_rows: placer.rows.len().max(1),
        requires_permutation: placer.locations.iter().any(|r| r.len() > 1),
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

/// Consecutive rows `top..top + count` of `columns`.
struct Segment {
    columns: Vec<usize>,
    top: usize,
    count: usize,
}

/// A way of dividing a trace of a fixed number of rows among stacks of row groups.
trait Packing {
    /// Take `count` rows of one column per entry of `copies`, in one or more
    /// segments. An entry is true for a position that may hold a wire with
    /// several cells; a packing may keep the others in columns of their own,
    /// which can then stay out of the permutation argument.
    fn take(&mut self, copies: &[bool], count: usize) -> Vec<Segment>;
    /// Columns used so far.
    fn width(&self) -> usize;
    /// Rows used so far in any column.
    fn used_rows(&self) -> usize;
}

/// The trace, `rows` rows high, divided into bands of adjacent columns as
/// stacks are placed. Free space is kept as rectangles cut from the bands.
struct Guillotine {
    rows: usize,
    free: Vec<Area>,
    /// Columns taken by all bands.
    width: usize,
    /// Rows taken in any band.
    used_rows: usize,
}
impl Guillotine {
    fn new(rows: usize) -> Self {
        Guillotine {
            rows,
            free: vec![],
            width: 0,
            used_rows: 0,
        }
    }
}
impl Packing for Guillotine {
    fn width(&self) -> usize {
        self.width
    }
    fn used_rows(&self) -> usize {
        self.used_rows
    }
    /// Take `count` rows of `copies.len()` columns, in segments; which positions
    /// may have copies is ignored. Each segment goes in the narrowest free
    /// rectangle that is wide enough, preferring one tall enough for all the
    /// remaining rows, then the earliest in the free list; a rectangle too
    /// short takes what fits and the rest goes elsewhere. The columns to the
    /// right of the segment and the rows below it stay free. Without a wide
    /// enough rectangle, a new band is added on the right.
    fn take(&mut self, copies: &[bool], mut count: usize) -> Vec<Segment> {
        let width = copies.len();
        let mut segments = vec![];
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
            segments.push(Segment {
                columns: (area.column..area.column + width).collect(),
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
        segments
    }
}

/// Open columns of one kind in [`Skyline`], fullest first.
struct Pool {
    columns: Vec<usize>,
    /// `sums[i]` is the sum of the levels of the first `i` columns.
    sums: Vec<usize>,
}

/// A new column used by [`Skyline`] counts as `rows / NEW_COLUMN_PENALTY`
/// empty cells on top of those above its segment, so that partly filled columns
/// are used before new ones are added.
const NEW_COLUMN_PENALTY: usize = 8;

/// The trace, `rows` rows high, as columns filled from the top down, each to
/// its own level. A segment may use any columns, adjacent or not. Columns that
/// hold positions which may have copies are kept apart from the others.
struct Skyline {
    rows: usize,
    /// Rows taken from the top of each column.
    levels: Vec<usize>,
    /// Whether each column holds positions that may have copies.
    copies: Vec<bool>,
    used_rows: usize,
}
impl Skyline {
    fn new(rows: usize) -> Self {
        Skyline {
            rows,
            levels: vec![],
            copies: vec![],
            used_rows: 0,
        }
    }
    /// The open columns of one kind, fullest first, with prefix sums of their
    /// levels.
    fn pool(&self, copies: bool) -> Pool {
        let mut columns: Vec<usize> = (0..self.levels.len())
            .filter(|c| self.copies[*c] == copies && self.levels[*c] < self.rows)
            .collect();
        columns.sort_by_key(|c| (std::cmp::Reverse(self.levels[*c]), *c));
        let mut sums = vec![0];
        for c in &columns {
            sums.push(sums.last().expect("starts at 0") + self.levels[*c]);
        }
        Pool { columns, sums }
    }
    /// The index in `pool` of its first column at or below level `top`, and
    /// how many cells the `k` columns from there leave empty above `top`.
    /// When the pool stacks out, new columns are added; each counts as its `top`
    /// empty cells plus a penalty of `rows / NEW_COLUMN_PENALTY` cells.
    fn choose(&self, pool: &Pool, top: usize, k: usize) -> (usize, usize) {
        let Pool { columns, sums } = pool;
        let first = columns.partition_point(|c| self.levels[*c] > top);
        let existing = k.min(columns.len() - first);
        let filled = sums[first + existing] - sums[first];
        let new = k - existing;
        (
            first,
            existing * top - filled + new * (top + self.rows / NEW_COLUMN_PENALTY),
        )
    }
}
impl Packing for Skyline {
    fn width(&self) -> usize {
        self.levels.len()
    }
    fn used_rows(&self) -> usize {
        self.used_rows
    }
    /// Take `count` rows of one column per entry of `copies`, in segments, with
    /// copy positions and the others in separate columns. A segment starts at a
    /// row `top` that is at or past the level of each of its columns; the
    /// cells between a column's level and `top` stay empty. Each candidate
    /// `top` (the level of an open column, or 0) gives each kind its fullest
    /// columns with a level up to `top`. The chosen `top` has room for all the
    /// remaining rows if any has, then leaves the fewest cells empty (counting
    /// `rows / NEW_COLUMN_PENALTY` more for each new column), then is the
    /// largest.
    fn take(&mut self, copies: &[bool], mut count: usize) -> Vec<Segment> {
        let wanted = |kind: bool| copies.iter().filter(|c| **c == kind).count();
        let (with_copies, without) = (wanted(true), wanted(false));
        let mut segments = vec![];
        while count > 0 {
            let pools = [self.pool(true), self.pool(false)];
            let mut tops: Vec<usize> = pools
                .iter()
                .flat_map(|pool| pool.columns.iter().map(|c| self.levels[*c]))
                .chain([0])
                .collect();
            tops.sort_unstable();
            tops.dedup();
            let top = *tops
                .iter()
                .min_by_key(|top| {
                    let empty = self.choose(&pools[0], **top, with_copies).1
                        + self.choose(&pools[1], **top, without).1;
                    (self.rows - **top < count, empty, std::cmp::Reverse(**top))
                })
                .expect("0 is a possible start");
            let taken = count.min(self.rows - top);
            let mut chosen = [vec![], vec![]];
            for (kind, (pool, k)) in pools.iter().zip([with_copies, without]).enumerate() {
                let (first, _) = self.choose(pool, top, k);
                let mut columns: Vec<usize> =
                    pool.columns[first..].iter().copied().take(k).collect();
                while columns.len() < k {
                    self.levels.push(0);
                    self.copies.push(kind == 0);
                    columns.push(self.levels.len() - 1);
                }
                for column in &columns {
                    self.levels[*column] = top + taken;
                }
                columns.sort_unstable();
                chosen[kind] = columns;
            }
            let [mut with_copies_left, mut without_left] = chosen.map(Vec::into_iter);
            let columns = copies
                .iter()
                .map(|copy| {
                    if *copy {
                        with_copies_left.next()
                    } else {
                        without_left.next()
                    }
                    .expect("one column per position")
                })
                .collect();
            self.used_rows = self.used_rows.max(top + taken);
            segments.push(Segment {
                columns,
                top,
                count: taken,
            });
            count -= taken;
        }
        segments
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
struct RowGroup<'a> {
    members: Vec<&'a Entry>,
    /// Distinct wires in order of first appearance; wire `i` goes to the
    /// `i`-th column of the segment that holds the row group.
    wires: Vec<u32>,
    /// Gate type and wire positions of each member. Row groups with equal keys
    /// differ only in which wires they use, so they can share gates.
    key: Vec<(usize, Vec<u32>)>,
}
impl<'a> RowGroup<'a> {
    fn single(entry: &'a Entry) -> Self {
        RowGroup {
            members: vec![entry],
            wires: entry.wires.clone(),
            key: vec![(entry.gate_type, (0..entry.wires.len() as u32).collect())],
        }
    }
    /// The row group's wires with their positions, in increasing wire order.
    fn sorted_wires(&self) -> Vec<(u32, u32)> {
        let mut wires: Vec<(u32, u32)> = self.wires.iter().copied().zip(0..).collect();
        wires.sort_unstable();
        wires
    }
    /// `self` followed by `other`, sharing the cells of common wires.
    fn join(mut self, other: RowGroup<'a>) -> Self {
        let mut positions: BTreeMap<u32, u32> = self.wires.iter().copied().zip(0..).collect();
        for wire in &other.wires {
            if !positions.contains_key(wire) {
                positions.insert(*wire, self.wires.len() as u32);
                self.wires.push(*wire);
            }
        }
        for (gate_type, members) in other.key {
            let moved = members
                .iter()
                .map(|p| positions[&other.wires[*p as usize]])
                .collect();
            self.key.push((gate_type, moved));
        }
        self.members.extend(other.members);
        self
    }
}

/// Most rounds of joining row groups that share wires; joining stops earlier when
/// a round joins nothing.
const JOIN_ROUNDS: usize = 64;
/// Wires held by more row groups than this are not used to find pairs to join.
const MAX_HOLDERS: usize = 16;
/// Relative proving costs used to decide joins, in gates: a column of the
/// trace (with its share of the permutation argument) costs about `GATES_PER_COLUMN`
/// gate constraints, and a selector column about `GATES_PER_SELECTOR`. The
/// values come from timing a STARK prover; see `docs/SCALE.md`.
const GATES_PER_COLUMN: f64 = 22.0;
const GATES_PER_SELECTOR: f64 = 0.5;

/// Relative cost of a column outside and inside the permutation argument, used to
/// choose between the two packings of the columns strategy. A permutation column
/// also brings about 1.5 committed stage-two columns and a fixed column of
/// labels, so it costs about 2.6 times a column without copies.
const COLUMN_COST: usize = 5;
const PERMUTATION_COLUMN_COST: usize = 13;

/// Cost, in gates, of `count` row groups of one key with `members` equations
/// each: one gate per member for every segment of at most `rows` row groups, plus
/// `GATES_PER_SELECTOR` when the last segment is shorter than `rows`.
fn stack_cost(count: usize, members: usize, rows: usize) -> f64 {
    let runs = count.div_ceil(rows);
    let partial = usize::from(count % rows != 0);
    (runs * members) as f64 + partial as f64 * GATES_PER_SELECTOR
}

/// The positions of the wires two row groups share, as pairs of positions in the
/// first and in the second, in order of position in the first. Each argument
/// is a row group's wires with their positions, in increasing wire order.
fn shared(a: &[(u32, u32)], b: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let (mut i, mut j, mut pairs) = (0, 0, vec![]);
    while i < a.len() && j < b.len() {
        match a[i].0.cmp(&b[j].0) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                pairs.push((a[i].1, b[j].1));
                i += 1;
                j += 1;
            }
        }
    }
    pairs.sort_unstable();
    pairs
}

/// Group equations into row groups of at most `max_width` wires. Each round
/// collects, for every pair of row groups that share a wire, the pattern
/// `(key, key, shared positions)` (see [`patterns`]). Going from the patterns
/// that save the most cells to the least, it joins the disjoint pairs of a
/// pattern when that lowers the weighted cost of the stacks involved (see
/// [`choose_joins`]), and stops after a round that joins nothing.
fn row_groups<'a>(
    entries: &[&'a Entry],
    wires: u32,
    max_width: usize,
    rows: usize,
) -> Vec<RowGroup<'a>> {
    let mut row_groups: Vec<RowGroup> = entries
        .iter()
        .map(|entry| RowGroup::single(entry))
        .collect();
    for _ in 0..JOIN_ROUNDS {
        let (keys, key_count) = key_ids(&row_groups);
        let patterns = patterns(&row_groups, &keys, wires, max_width);
        let joins = choose_joins(&row_groups, &keys, key_count, &patterns, max_width, rows);
        if joins.partners.iter().all(Vec::is_empty) {
            break;
        }
        row_groups = apply_joins(row_groups, &joins);
    }
    row_groups
}

/// A number for each row group's key, in order of first appearance, and the
/// number of distinct keys.
fn key_ids(row_groups: &[RowGroup]) -> (Vec<usize>, usize) {
    let mut ids = BTreeMap::new();
    let keys = row_groups
        .iter()
        .map(|row_group| {
            let next = ids.len();
            *ids.entry(&row_group.key).or_insert(next)
        })
        .collect();
    (keys, ids.len())
}

/// Pairs of row groups that share wires and could be joined, grouped by the
/// keys of both row groups and the positions they share.
struct Pattern {
    /// Key of the second row group of every pair.
    partner_key: usize,
    /// Positions of the shared wires, in the first row group and in the second.
    shared: Vec<(u32, u32)>,
    /// The pairs, each oriented so that its first row group has the smaller key.
    pairs: Vec<(usize, usize)>,
}

/// The patterns among pairs of row groups that share a wire held by at most
/// `MAX_HOLDERS` row groups and that, joined, fit in `max_width` wires; the ones
/// that save the most cells (pairs times shared wires) come first.
fn patterns(row_groups: &[RowGroup], keys: &[usize], wires: u32, max_width: usize) -> Vec<Pattern> {
    let mut holders = vec![vec![]; wires as usize];
    for (index, row_group) in row_groups.iter().enumerate() {
        for wire in &row_group.wires {
            holders[*wire as usize].push(index);
        }
    }
    let sorted: Vec<_> = row_groups.iter().map(RowGroup::sorted_wires).collect();
    let mut pairs = vec![];
    for held in holders
        .iter()
        .filter(|held| (2..=MAX_HOLDERS).contains(&held.len()))
    {
        for (i, a) in held.iter().enumerate() {
            for b in &held[i + 1..] {
                // Orient each pair by key so that equal patterns compare equal.
                pairs.push(if (keys[*a], *a) <= (keys[*b], *b) {
                    (*a, *b)
                } else {
                    (*b, *a)
                });
            }
        }
    }
    pairs.sort_unstable();
    pairs.dedup();
    let mut patterns: BTreeMap<_, Vec<(usize, usize)>> = BTreeMap::new();
    for (a, b) in pairs {
        let shared = shared(&sorted[a], &sorted[b]);
        if row_groups[a].wires.len() + row_groups[b].wires.len() - shared.len() <= max_width {
            patterns
                .entry((keys[a], keys[b], shared))
                .or_default()
                .push((a, b));
        }
    }
    let mut patterns: Vec<_> = patterns
        .into_iter()
        .map(|((_, partner_key, shared), pairs)| Pattern {
            partner_key,
            shared,
            pairs,
        })
        .collect();
    patterns.sort_by_key(|pattern| std::cmp::Reverse(pattern.pairs.len() * pattern.shared.len()));
    patterns
}

/// The joins of one round: the partners each row group takes, in order, and
/// whether each row group joins another.
struct Joins {
    partners: Vec<Vec<usize>>,
    absorbed: Vec<bool>,
}

/// Choose the joins of one round. Patterns are visited in order, and within
/// a pattern pairs `(a, b)` are taken greedily: neither row group may be absorbed
/// already, `b` may have no partners, the joined width must fit in
/// `max_width`, and each row group is used at most once per pattern (if `b` was
/// used, `a` still counts as used). So a row group takes at most one partner per
/// pattern, and each partner joins one row group.
///
/// Costs are tracked per group: a key followed by the patterns taken so far.
/// This assumes the row groups of a group end up with the same key; they usually
/// do, and when they do not (a later partner shares wires with an earlier
/// one) only the estimate is off, not the width. A pattern's joins are made
/// only if the change in [`stack_cost`] of the stacks involved is less than the
/// cells saved, in columns, times `GATES_PER_COLUMN`.
fn choose_joins(
    row_groups: &[RowGroup],
    keys: &[usize],
    key_count: usize,
    patterns: &[Pattern],
    max_width: usize,
    rows: usize,
) -> Joins {
    let mut group: Vec<usize> = keys.to_vec();
    let mut count = vec![0usize; key_count];
    let mut members = vec![0usize; key_count];
    for (row_group, key) in row_groups.iter().zip(keys) {
        count[*key] += 1;
        members[*key] = row_group.key.len();
    }
    let mut next_groups = BTreeMap::new();
    let mut width: Vec<usize> = row_groups
        .iter()
        .map(|row_group| row_group.wires.len())
        .collect();
    let mut joins = Joins {
        partners: vec![vec![]; row_groups.len()],
        absorbed: vec![false; row_groups.len()],
    };
    for (index, pattern) in patterns.iter().enumerate() {
        let (b_key, shared) = (pattern.partner_key, &pattern.shared);
        let mut taken = BTreeSet::new();
        let chosen: Vec<_> = pattern
            .pairs
            .iter()
            .copied()
            .filter(|&(a, b)| {
                !joins.absorbed[a]
                    && !joins.absorbed[b]
                    && joins.partners[b].is_empty()
                    && width[a] + row_groups[b].wires.len() - shared.len() <= max_width
                    && taken.insert(a)
                    && taken.insert(b)
            })
            .collect();
        if chosen.is_empty() {
            continue;
        }
        // Row group counts per key after the joins: each host moves to its group
        // followed by this pattern, and each partner leaves its key.
        let mut after: BTreeMap<usize, usize> = BTreeMap::new();
        let mut moved: BTreeMap<usize, usize> = BTreeMap::new();
        for &(a, _) in &chosen {
            *after.entry(group[a]).or_insert(count[group[a]]) -= 1;
            *moved.entry(group[a]).or_default() += 1;
            *after.entry(b_key).or_insert(count[b_key]) -= 1;
        }
        let before: f64 = after
            .keys()
            .map(|g| stack_cost(count[*g], members[*g], rows))
            .sum();
        let remaining: f64 = after
            .iter()
            .map(|(g, n)| stack_cost(*n, members[*g], rows))
            .sum();
        let joined: f64 = moved
            .iter()
            .map(|(g, n)| stack_cost(*n, members[*g] + members[b_key], rows))
            .sum();
        let saved_columns = (chosen.len() * shared.len()) as f64 / rows as f64;
        if remaining + joined - before - saved_columns * GATES_PER_COLUMN >= 0.0 {
            continue;
        }
        for (g, n) in after {
            count[g] = n;
        }
        for &(a, b) in &chosen {
            let from = group[a];
            let to = *next_groups.entry((from, index)).or_insert_with(|| {
                count.push(0);
                members.push(members[from] + members[b_key]);
                count.len() - 1
            });
            count[to] += 1;
            group[a] = to;
            width[a] += row_groups[b].wires.len() - shared.len();
            joins.partners[a].push(b);
            joins.absorbed[b] = true;
        }
    }
    joins
}

/// The row groups after `joins`: each row group that takes partners followed by
/// them in order, and every other row group that joins no one, as it was.
fn apply_joins<'a>(row_groups: Vec<RowGroup<'a>>, joins: &Joins) -> Vec<RowGroup<'a>> {
    let mut pending: Vec<Option<RowGroup>> = row_groups.into_iter().map(Some).collect();
    let mut next = vec![];
    for index in 0..pending.len() {
        if joins.absorbed[index] {
            continue;
        }
        let mut row_group = pending[index].take().expect("each row group is taken once");
        for partner in &joins.partners[index] {
            row_group = row_group.join(pending[*partner].take().expect("partner joins once"));
        }
        next.push(row_group);
    }
    next
}

/// Place the equations in row groups (see [`row_groups`]), one row group per row in
/// as many columns as the row group has wires. The row groups of one key form a
/// stack, placed top down in one or more segments of the trace; in each segment,
/// the `i`-th members of all row groups share one gate. Stacks go widest first,
/// then longest first, and are packed both as [`Guillotine`] and as
/// [`Skyline`], keeping the cheaper one that fits in `MAX_WIDTH` columns (see
/// `COLUMN_COST`). The width is at least `MIN_WIDTH`.
fn place_in_stacks(
    entries: &[Entry],
    rewritten: &RewrittenCircuit,
    rows: usize,
    max_width: usize,
) -> Result<Placed> {
    let with_wires: Vec<&Entry> = entries
        .iter()
        .filter(|entry| !entry.wires.is_empty())
        .collect();
    let mut by_key: BTreeMap<Vec<(usize, Vec<u32>)>, Vec<RowGroup>> = BTreeMap::new();
    for row_group in row_groups(&with_wires, rewritten.wires, max_width, rows) {
        by_key
            .entry(row_group.key.clone())
            .or_default()
            .push(row_group);
    }
    let mut runs: Vec<_> = by_key.into_values().collect();
    runs.sort_by_key(|run| {
        (
            std::cmp::Reverse(run[0].wires.len()),
            std::cmp::Reverse(run.len()),
            run[0].members[0].index,
        )
    });
    // Pack with both methods and keep the cheaper trace that fits.
    let rectangles = arrange(entries, rewritten, &runs, Guillotine::new(rows));
    let levels = arrange(entries, rewritten, &runs, Skyline::new(rows));
    let narrowest = rectangles.width.min(levels.width);
    ensure_within_limit(
        narrowest <= MAX_WIDTH,
        format_args!(
            "the gate types need {narrowest} columns in {rows} rows, more than {MAX_WIDTH}; raise minRows"
        ),
    )?;
    let placed = [rectangles, levels]
        .into_iter()
        .filter(|placed| placed.width <= MAX_WIDTH)
        .min_by_key(|placed| {
            COLUMN_COST * (placed.width - placed.permutation_columns)
                + PERMUTATION_COLUMN_COST * placed.permutation_columns
        })
        .expect("the narrowest packing fits");
    Ok(Placed {
        width: placed.width.max(MIN_WIDTH),
        ..placed
    })
}

/// Place `runs` with `packing`, then the public wires that appear in no
/// equation, one cell each; each public value binds to the first cell its
/// wire was given. The width is the packing's, possibly beyond `MAX_WIDTH`.
fn arrange(
    entries: &[Entry],
    rewritten: &RewrittenCircuit,
    runs: &[Vec<RowGroup>],
    mut packing: impl Packing,
) -> Placed {
    let mut occupied = Occupied {
        first: vec![None; rewritten.wires as usize],
        cells: vec![0; rewritten.wires as usize],
    };
    // Cells per wire: one in each row group that holds it.
    let mut cells_per_wire = vec![0usize; rewritten.wires as usize];
    for wire in runs.iter().flatten().flat_map(|row_group| &row_group.wires) {
        cells_per_wire[*wire as usize] += 1;
    }
    let mut placements = vec![];
    let mut cells = vec![];
    for run in runs {
        // A position may have copies if, in some row group of the stack, its wire
        // is in more than one row group.
        let copies: Vec<bool> = (0..run[0].wires.len())
            .map(|p| {
                run.iter()
                    .any(|row_group| cells_per_wire[row_group.wires[p] as usize] > 1)
            })
            .collect();
        let mut row_groups = run.iter();
        for segment in packing.take(&copies, run.len()) {
            for (row, row_group) in
                (segment.top..segment.top + segment.count).zip(row_groups.by_ref())
            {
                for (position, wire) in row_group.wires.iter().enumerate() {
                    occupied.add(*wire, row, segment.columns[position]);
                    cells.push((*wire, segment.columns[position]));
                }
                for (member, (_, positions)) in row_group.members.iter().zip(&row_group.key) {
                    let columns = positions
                        .iter()
                        .map(|p| segment.columns[*p as usize])
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
    let unplaced: Vec<u32> = (1..=rewritten.publics)
        .filter(|wire| occupied.first[*wire as usize].is_none())
        .collect();
    let mut wires = unplaced.iter();
    for segment in packing.take(&[false], unplaced.len()) {
        for (row, wire) in (segment.top..segment.top + segment.count).zip(wires.by_ref()) {
            occupied.add(*wire, row, segment.columns[0]);
        }
    }
    let public_bindings = (0..rewritten.publics)
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
    let permutation_columns = cells
        .iter()
        .filter(|(wire, _)| occupied.cells[*wire as usize] > 1)
        .map(|(_, column)| *column)
        .collect::<BTreeSet<_>>()
        .len();
    Placed {
        width: packing.width(),
        permutation_columns,
        placements,
        public_bindings,
        used_rows: packing.used_rows().max(1),
        requires_permutation: occupied.cells.iter().any(|count| *count > 1),
    }
}

/// An equation not placed yet: its index, gate type group and wires.
struct Entry {
    index: usize,
    gate_type: usize,
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
            gate_type: self.gate_type,
            wires: self.wires.clone(),
            row,
            columns,
        }
    }
}

/// The distinct gate types and one unplaced entry per equation, in equation order.
fn gate_entries(rewritten: &RewrittenCircuit, width: usize) -> Result<(Vec<GateType>, Vec<Entry>)> {
    let mut gate_types = vec![];
    let mut ids = BTreeMap::new();
    let mut entries = vec![];
    for (index, equation) in rewrite::equations(rewritten).iter().enumerate() {
        let gate = rewrite::canonical_gate(equation);
        ensure(gate.wires.len() <= width, "equation exceeds width")?;
        let id = *ids.entry(gate.key).or_insert_with(|| {
            let id = gate_types.len();
            gate_types.push(GateType {
                id,
                width: gate.wires.len(),
                terms: gate.terms,
            });
            id
        });
        entries.push(Entry {
            index,
            gate_type: id,
            wires: gate.wires,
        });
    }
    Ok((gate_types, entries))
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
            entry.gate_type,
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

/// Among the first `LOCAL_CANDIDATES` rows found that hold some of `wires`,
/// the one with room that shares the most wires; ties go to the row left with
/// the fewest free cells.
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

/// One gate per distinct `(gate type, columns)` pair, active on the rows that use
/// it, and one selector per distinct set of active rows.
fn gates_and_selectors(placements: &[Placement]) -> (Vec<Gate>, Vec<Selector>) {
    let mut gates: Vec<Gate> = vec![];
    let mut keys = BTreeMap::new();
    for placement in placements {
        let key = (placement.gate_type, placement.columns.clone());
        let id = *keys.entry(key).or_insert_with(|| {
            let index = gates.len();
            gates.push(Gate {
                gate_type: placement.gate_type,
                columns: placement.columns.clone(),
                rows: vec![],
                selector: 0,
            });
            index
        });
        gates[id].rows.push(placement.row);
    }
    let mut selectors = vec![];
    let mut masks = BTreeMap::new();
    for gate in &mut gates {
        gate.rows.sort_unstable();
        gate.rows.dedup();
        let id = *masks.entry(gate.rows.clone()).or_insert_with(|| {
            let id = selectors.len();
            selectors.push(Selector {
                id,
                rows: gate.rows.clone(),
            });
            id
        });
        gate.selector = id;
    }
    (gates, selectors)
}
