//! Placement check: verifies that a helper network and layout preserve the
//! source equations and public order, without calling the code that built them.
use crate::{
    certificate::{Group, Layout, Network, Placement},
    contract::{
        MAX_CELLS, MAX_HELPER_EXPANSION, MAX_PRODUCT_TERMS, MAX_ROWS, MAX_WIDTH, MIN_ROWS,
        MIN_WIDTH,
    },
    ensure,
    field::{mul, neg, FieldElement},
    polynomial::accumulate,
    r1cs::{Circuit, Equation},
    Result, P,
};
use std::collections::{BTreeMap, BTreeSet};

/// Linear combination over wires: wire to nonzero coefficient.
type Linear = BTreeMap<u32, u64>;
/// Quadratic polynomial over wires: `(i, j)` with `i <= j` is the monomial
/// `w_i · w_j`; wire 0 is the constant 1, so `(0, j)` is linear in `w_j`.
type Quadratic = BTreeMap<(u32, u32), u64>;
/// A slot activation: shape, columns and row.
type Activation = (usize, Vec<usize>, usize);

/// Result of a successful placement check.
pub struct Checked {
    /// Wire in each cell, row by row; 0 marks an unused cell.
    pub map: Vec<u32>,
    /// Helper equations, then converted equations, as numbered by
    /// [`Placement::index`].
    pub equations: Vec<Equation>,
    /// Cells `(row, column)` of each wire.
    pub occurrences: BTreeMap<u32, Vec<(usize, usize)>>,
}

/// Check a network and layout against their source:
/// 1. each helper references only earlier wires and has first coefficient 1;
/// 2. after replacing helpers by their definitions, every converted equation
///    is exactly its source equation;
/// 3. every helper and converted equation is placed exactly once, on a row
///    where its slot is active, in a shape that gives back the same equation,
///    and no cell holds two different wires;
/// 4. public values bind to wires `1..=publics` in order, and the row counts
///    and `requires_copy` match the cells.
pub fn check(source: &Circuit, network: &Network, layout: &Layout) -> Result<Checked> {
    check_dimensions(source, network, layout)?;
    // Every wire standing for itself, to compare equations without expanding helpers.
    let identity: Vec<Linear> = (0..network.wires)
        .map(|w| BTreeMap::from([(w, 1)]))
        .collect();
    let (expanded, mut equations) = expand_helpers(source, network, &identity)?;
    for (index, (actual, expected)) in network.converted.iter().zip(&source.equations).enumerate() {
        check_canonical(actual, network.wires)
            .map_err(|e| format!("converted equation {index}: {e}"))?;
        let expected = expected
            .clone()
            .map(|lc| lc.into_iter().filter(|(_, k)| k.0 != 0).collect());
        ensure(
            residual(actual, &expanded)? == residual(&expected, &identity)?,
            format!("converted equation {index} differs from its source equation"),
        )?;
        equations.push(actual.clone());
    }
    let active = active_slots(layout)?;
    let mut cells = Cells::new(layout, network.wires);
    let arithmetic_rows = place_equations(layout, &equations, &identity, &active, &mut cells)?;
    ensure(
        layout.public_bindings.len() == source.publics as usize,
        "one public binding per public value",
    )?;
    for (index, binding) in layout.public_bindings.iter().enumerate() {
        ensure(
            binding.public as usize == index && binding.wire as usize == index + 1,
            "public binding i must bind public value i to wire i + 1",
        )?;
        cells.set(binding.row, binding.col, binding.wire)?;
    }
    let occupied_rows = cells
        .occurrences
        .values()
        .flatten()
        .map(|(row, _)| row + 1)
        .max()
        .unwrap_or(0);
    ensure(
        layout.used_rows == 1.max(arithmetic_rows).max(occupied_rows),
        "usedRows must count the rows up to the last one holding an equation or public cell",
    )?;
    ensure(
        layout.rows == layout.used_rows.next_power_of_two().max(layout.min_rows),
        "rows must be usedRows rounded up to a power of two, at least minRows",
    )?;
    ensure(
        layout.requires_copy == cells.occurrences.values().any(|cells| cells.len() > 1),
        "requiresCopy must be set exactly when some wire occupies several cells",
    )?;
    Ok(Checked {
        map: cells.map,
        equations,
        occurrences: cells.occurrences,
    })
}

/// Wire, public and equation counts against the source, and layout size limits.
fn check_dimensions(source: &Circuit, network: &Network, layout: &Layout) -> Result<()> {
    let wires = u32::try_from(network.helpers.len())
        .ok()
        .and_then(|helpers| source.wires.checked_add(helpers));
    ensure(
        network.original_wires == source.wires
            && network.publics == source.publics
            && Some(network.wires) == wires,
        "network wire or public counts differ from the source",
    )?;
    ensure(
        (MIN_WIDTH..=MAX_WIDTH).contains(&layout.width),
        format!("width must be in {MIN_WIDTH}..={MAX_WIDTH}"),
    )?;
    ensure(
        layout.min_rows >= MIN_ROWS && layout.min_rows.is_power_of_two(),
        format!("minRows must be a power of two, at least {MIN_ROWS}"),
    )?;
    ensure(
        layout.rows.is_power_of_two() && layout.rows <= MAX_ROWS && layout.rows >= layout.min_rows,
        format!("rows must be a power of two in minRows..={MAX_ROWS}"),
    )?;
    ensure(
        layout.rows.saturating_mul(layout.width) <= MAX_CELLS,
        format!("layout exceeds {MAX_CELLS} cells"),
    )?;
    ensure(
        network.converted.len() == source.equations.len(),
        "one converted equation per source equation",
    )
}

/// Each side lists wires below `wires` in increasing order, with nonzero
/// coefficients below `P`.
fn check_canonical(equation: &Equation, wires: u32) -> Result<()> {
    for lc in equation {
        let mut last = None;
        for (wire, k) in lc {
            ensure(*wire < wires, format!("wire {wire} out of range"))?;
            ensure(
                k.0 > 0 && k.0 < P,
                format!("coefficient of wire {wire} must be in 1..P"),
            )?;
            ensure(
                last.is_none_or(|previous| *wire > previous),
                "wires must be in increasing order",
            )?;
            last = Some(*wire);
        }
    }
    Ok(())
}

/// Check helper ids and normalization; return each wire's expansion over
/// source wires and the defining equation `0 · 0 = terms - helper` of each helper.
fn expand_helpers(
    source: &Circuit,
    network: &Network,
    identity: &[Linear],
) -> Result<(Vec<Linear>, Vec<Equation>)> {
    let mut expanded = identity[..source.wires as usize].to_vec();
    let mut equations = vec![];
    for (index, helper) in network.helpers.iter().enumerate() {
        ensure(
            helper
                .id
                .checked_sub(source.wires)
                .map(|offset| offset as usize)
                == Some(index),
            format!("helper {index}: ids must follow the source wires in order"),
        )?;
        ensure(
            helper.terms.len() >= 2,
            format!("helper {index}: needs at least two terms"),
        )?;
        // Terms are canonical and reference only earlier wires, so helpers are acyclic.
        check_canonical(&[helper.terms.clone(), vec![], vec![]], helper.id)
            .map_err(|e| format!("helper {index}: {e}"))?;
        ensure(
            helper.terms.iter().all(|(wire, _)| *wire > 0),
            format!("helper {index}: terms must not use wire 0"),
        )?;
        ensure(
            helper.terms[0].1 .0 == 1,
            format!("helper {index}: first coefficient must be 1"),
        )?;
        let mut values = BTreeMap::new();
        for (wire, k) in &helper.terms {
            for (source_wire, c) in &expanded[*wire as usize] {
                accumulate(&mut values, *source_wire, mul(k.0, *c));
            }
        }
        ensure(
            values.len() <= MAX_HELPER_EXPANSION,
            format!("helper {index}: expansion exceeds the size limit"),
        )?;
        expanded.push(values);
        let mut terms = helper.terms.clone();
        terms.push((helper.id, FieldElement(P - 1)));
        equations.push([vec![], vec![], terms]);
    }
    Ok((expanded, equations))
}

/// `A · B - C` for `equation`, after replacing each wire by its expansion `expanded[w]`.
fn residual(equation: &Equation, expanded: &[Linear]) -> Result<Quadratic> {
    let mut sides: [Linear; 3] = Default::default();
    for (side, terms) in equation.iter().enumerate() {
        for (wire, k) in terms {
            let expansion = expanded
                .get(*wire as usize)
                .ok_or("equation refers to an unknown wire")?;
            for (source_wire, c) in expansion {
                accumulate(&mut sides[side], *source_wire, mul(k.0, *c));
            }
        }
    }
    let [a, b, c] = &sides;
    ensure(
        a.len().saturating_mul(b.len()) <= MAX_PRODUCT_TERMS,
        "placement check polynomial size limit",
    )?;
    let mut result = BTreeMap::new();
    for (i, x) in a {
        for (j, y) in b {
            accumulate(&mut result, ((*i).min(*j), (*i).max(*j)), mul(*x, *y));
        }
    }
    for (i, z) in c {
        accumulate(&mut result, (0, *i), neg(*z));
    }
    Ok(result)
}

/// Check shape groups, selectors and slots; return every activation of a slot.
fn active_slots(layout: &Layout) -> Result<BTreeSet<Activation>> {
    for (id, group) in layout.groups.iter().enumerate() {
        check_group(id, group, layout.width)?;
    }
    for (id, selector) in layout.selectors.iter().enumerate() {
        ensure(selector.id == id, format!("selector {id}: id out of order"))?;
        ensure(
            !selector.rows.is_empty(),
            format!("selector {id}: no active rows"),
        )?;
        ensure(
            selector.rows.windows(2).all(|pair| pair[0] < pair[1]),
            format!("selector {id}: rows must be in increasing order"),
        )?;
        ensure(
            selector
                .rows
                .iter()
                .all(|row| *row < layout.arithmetic_rows),
            format!("selector {id}: row beyond arithmeticRows"),
        )?;
    }
    let mut active = BTreeSet::new();
    let mut slot_keys = BTreeSet::new();
    for (id, slot) in layout.slots.iter().enumerate() {
        ensure(
            slot.shape < layout.groups.len(),
            format!("slot {id}: unknown shape"),
        )?;
        ensure(
            slot.selector < layout.selectors.len(),
            format!("slot {id}: unknown selector"),
        )?;
        ensure(
            slot.rows == layout.selectors[slot.selector].rows,
            format!("slot {id}: rows differ from its selector's rows"),
        )?;
        ensure(
            slot.columns.len() == layout.groups[slot.shape].width,
            format!("slot {id}: column count differs from its shape width"),
        )?;
        ensure(
            slot.columns.iter().all(|column| *column < layout.width),
            format!("slot {id}: column out of range"),
        )?;
        ensure(
            slot.columns.iter().collect::<BTreeSet<_>>().len() == slot.columns.len(),
            format!("slot {id}: repeated column"),
        )?;
        ensure(
            slot_keys.insert((slot.shape, slot.columns.clone())),
            format!("slot {id}: duplicate of an earlier slot"),
        )?;
        for row in &slot.rows {
            active.insert((slot.shape, slot.columns.clone(), *row));
        }
    }
    for id in 0..layout.selectors.len() {
        ensure(
            layout.slots.iter().any(|slot| slot.selector == id),
            format!("selector {id}: no slot uses it"),
        )?;
    }
    Ok(active)
}

/// Group ids are positions; every term refers to the constant (position -1) or
/// a position of the shape, in increasing order, with a coefficient in `1..P`.
fn check_group(id: usize, group: &Group, width: usize) -> Result<()> {
    ensure(group.id == id, format!("shape {id}: id out of order"))?;
    ensure(
        group.width <= width,
        format!("shape {id}: wider than the layout"),
    )?;
    for lc in &group.terms {
        let mut last = None;
        for (position, k) in lc {
            ensure(
                *position == -1 || usize::try_from(*position).is_ok_and(|p| p < group.width),
                format!("shape {id}: position out of range"),
            )?;
            ensure(
                k.0 > 0 && k.0 < P,
                format!("shape {id}: coefficient must be in 1..P"),
            )?;
            ensure(
                last.is_none_or(|previous| *position > previous),
                format!("shape {id}: positions must be in increasing order"),
            )?;
            last = Some(*position);
        }
    }
    Ok(())
}

/// The wire in each cell, row by row; each cell holds at most one wire.
struct Cells {
    rows: usize,
    width: usize,
    wires: u32,
    map: Vec<u32>,
    occurrences: BTreeMap<u32, Vec<(usize, usize)>>,
}
impl Cells {
    fn new(layout: &Layout, wires: u32) -> Self {
        Self {
            rows: layout.rows,
            width: layout.width,
            wires,
            map: vec![0; layout.rows * layout.width],
            occurrences: BTreeMap::new(),
        }
    }
    /// Put `wire` in a cell, failing if the cell holds a different wire.
    fn set(&mut self, row: usize, col: usize, wire: u32) -> Result<()> {
        ensure(
            row < self.rows && col < self.width,
            format!("cell ({row}, {col}) out of range"),
        )?;
        ensure(
            wire > 0 && wire < self.wires,
            format!("wire {wire} cannot occupy a cell"),
        )?;
        let old = &mut self.map[row * self.width + col];
        ensure(
            *old == 0 || *old == wire,
            format!("cell ({row}, {col}) would hold two different wires"),
        )?;
        if *old == 0 {
            *old = wire;
            self.occurrences.entry(wire).or_default().push((row, col));
        }
        Ok(())
    }
}

/// Check that every equation is placed exactly once and that the placed
/// `(shape, columns, row)` triples are exactly the active ones. Fills `cells`
/// and returns the number of rows holding equations.
fn place_equations(
    layout: &Layout,
    equations: &[Equation],
    identity: &[Linear],
    active: &BTreeSet<Activation>,
    cells: &mut Cells,
) -> Result<usize> {
    ensure(
        layout.placements.len() == equations.len(),
        "one placement per helper and converted equation",
    )?;
    let mut seen = BTreeSet::new();
    let mut used = BTreeSet::new();
    let mut arithmetic_rows = 0;
    for placement in &layout.placements {
        check_placement(layout, placement, equations, identity, &mut seen)?;
        let key = (placement.shape, placement.columns.clone(), placement.row);
        ensure(
            active.contains(&key),
            format!(
                "equation {} is placed on a row where its slot is not active",
                placement.index
            ),
        )?;
        used.insert(key);
        for (wire, column) in placement.wires.iter().zip(&placement.columns) {
            cells.set(placement.row, *column, *wire)?;
        }
        arithmetic_rows = arithmetic_rows.max(placement.row + 1);
    }
    // Every equation is placed: the counts match and no index repeats.
    ensure(
        *active == used,
        "placements must match the active slot rows exactly",
    )?;
    for id in 0..layout.groups.len() {
        ensure(
            layout
                .placements
                .iter()
                .any(|placement| placement.shape == id),
            format!("shape {id}: no equation uses it"),
        )?;
    }
    ensure(
        arithmetic_rows == layout.arithmetic_rows,
        "arithmeticRows must be the last row holding an equation, plus one",
    )?;
    Ok(arithmetic_rows)
}

/// One placement: a new equation index, a known shape and row, consistent
/// sizes, and a shape that gives back the placed equation.
fn check_placement(
    layout: &Layout,
    placement: &Placement,
    equations: &[Equation],
    identity: &[Linear],
    seen: &mut BTreeSet<usize>,
) -> Result<()> {
    let index = placement.index;
    ensure(
        index < equations.len(),
        format!("placement of unknown equation {index}"),
    )?;
    ensure(
        seen.insert(index),
        format!("equation {index} is placed twice"),
    )?;
    ensure(
        placement.shape < layout.groups.len(),
        format!("equation {index}: unknown shape"),
    )?;
    ensure(
        placement.row < layout.arithmetic_rows,
        format!("equation {index}: row beyond arithmeticRows"),
    )?;
    let group = &layout.groups[placement.shape];
    ensure(
        placement.wires.len() == group.width && placement.columns.len() == group.width,
        format!("equation {index}: wire or column count differs from its shape width"),
    )?;
    ensure(
        placement.columns.iter().collect::<BTreeSet<_>>().len() == group.width,
        format!("equation {index}: repeated column"),
    )?;
    let instantiated = group.terms.clone().map(|lc| {
        lc.into_iter()
            .map(|(position, k)| {
                // Position -1 is the constant wire 0.
                let wire = usize::try_from(position).map_or(0, |p| placement.wires[p]);
                (wire, k)
            })
            .collect()
    });
    ensure(
        residual(&instantiated, identity)? == residual(&equations[index], identity)?,
        format!("equation {index}: its shape and wires give a different equation"),
    )
}
