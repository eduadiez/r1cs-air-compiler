//! Placement check: verifies that a rewritten circuit and layout preserve the
//! source equations and public order, without calling the code that built them.
use crate::{
    certificate::{GateType, Layout, Placement, RewrittenCircuit},
    contract::{
        MAX_AUXILIARY_EXPANSION, MAX_CELLS, MAX_PRODUCT_TERMS, MAX_ROWS, MAX_WIDTH, MIN_ROWS,
        MIN_WIDTH,
    },
    ensure, ensure_within_limit,
    field::{add, mul, neg, FieldElement},
    polynomial::accumulate,
    r1cs::{Circuit, Equation},
    Result, P,
};
use std::collections::{BTreeMap, BTreeSet};

/// Linear combination over wires: wire to nonzero coefficient.
type Linear = BTreeMap<u32, u64>;
/// Quadratic polynomial over wires, as monomials in increasing order with
/// nonzero coefficients: `(i, j)` with `i <= j` is the monomial `w_i · w_j`;
/// wire 0 is the constant 1, so `(0, j)` is linear in `w_j`.
type Quadratic = Vec<((u32, u32), u64)>;
/// A gate activation: gate type, columns and row.
type Activation = (usize, Vec<usize>, usize);

/// Result of a successful placement check.
pub struct Checked {
    /// Wire in each cell, row by row; 0 marks an unused cell.
    pub map: Vec<u32>,
    /// Auxiliary equations, then rewritten equations, as numbered by
    /// [`Placement::index`].
    pub equations: Vec<Equation>,
    /// Cells `(row, column)` of each wire.
    pub occurrences: BTreeMap<u32, Vec<(usize, usize)>>,
}

/// Check a rewritten and layout against their source:
/// 1. each auxiliary wire references only earlier wires and has first coefficient 1;
/// 2. after replacing auxiliary wires by their definitions, every rewritten equation
///    is exactly its source equation;
/// 3. every auxiliary wire and rewritten equation is placed exactly once, on a row
///    where its gate is active, in a gate type that gives back the same equation,
///    and no cell holds two different wires;
/// 4. public values bind to wires `1..=publics` in order, and the row counts
///    and `requires_permutation` match the cells.
pub fn check(source: &Circuit, rewritten: &RewrittenCircuit, layout: &Layout) -> Result<Checked> {
    check_dimensions(source, rewritten, layout)?;
    // Every wire standing for itself, to compare equations without expanding auxiliary wires.
    let identity: Vec<Linear> = (0..rewritten.wires)
        .map(|w| BTreeMap::from([(w, 1)]))
        .collect();
    let (expanded, mut equations) = expand_auxiliaries(source, rewritten, &identity)?;
    for (index, (actual, expected)) in rewritten
        .equations
        .iter()
        .zip(&source.equations)
        .enumerate()
    {
        check_canonical(actual, rewritten.wires)
            .map_err(|e| format!("rewritten equation {index}: {e}"))?;
        let expected = expected
            .clone()
            .map(|lc| lc.into_iter().filter(|(_, k)| k.0 != 0).collect());
        ensure(
            residual(actual, &expanded)? == residual(&expected, &identity)?,
            format_args!("rewritten equation {index} differs from its source equation"),
        )?;
        equations.push(actual.clone());
    }
    let active = active_gates(layout)?;
    let mut cells = Cells::new(layout, rewritten.wires);
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
        layout.requires_permutation == cells.occurrences.values().any(|cells| cells.len() > 1),
        "requiresPermutation must be set exactly when some wire occupies several cells",
    )?;
    Ok(Checked {
        map: cells.map,
        equations,
        occurrences: cells.occurrences,
    })
}

/// Wire, public and equation counts against the source, and layout size limits.
fn check_dimensions(source: &Circuit, rewritten: &RewrittenCircuit, layout: &Layout) -> Result<()> {
    let wires = u32::try_from(rewritten.auxiliaries.len())
        .ok()
        .and_then(|auxiliaries| source.wires.checked_add(auxiliaries));
    ensure(
        rewritten.original_wires == source.wires
            && rewritten.publics == source.publics
            && Some(rewritten.wires) == wires,
        "rewritten wire or public counts differ from the source",
    )?;
    ensure(
        (MIN_WIDTH..=MAX_WIDTH).contains(&layout.width),
        format_args!("width must be in {MIN_WIDTH}..={MAX_WIDTH}"),
    )?;
    ensure(
        layout.min_rows >= MIN_ROWS && layout.min_rows.is_power_of_two(),
        format_args!("minRows must be a power of two, at least {MIN_ROWS}"),
    )?;
    ensure(
        layout.rows.is_power_of_two() && layout.rows <= MAX_ROWS && layout.rows >= layout.min_rows,
        format_args!("rows must be a power of two in minRows..={MAX_ROWS}"),
    )?;
    ensure_within_limit(
        layout.rows.saturating_mul(layout.width) <= MAX_CELLS,
        format_args!("layout exceeds {MAX_CELLS} cells"),
    )?;
    ensure(
        rewritten.equations.len() == source.equations.len(),
        "one rewritten equation per source equation",
    )
}

/// Each side lists wires below `wires` in increasing order, with nonzero
/// coefficients below `P`.
fn check_canonical(equation: &Equation, wires: u32) -> Result<()> {
    for lc in equation {
        let mut last = None;
        for (wire, k) in lc {
            ensure(*wire < wires, format_args!("wire {wire} out of range"))?;
            ensure(
                k.0 > 0 && k.0 < P,
                format_args!("coefficient of wire {wire} must be in 1..P"),
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

/// Check auxiliary ids and normalization; return each wire's expansion over
/// source wires and the defining equation `0 · 0 = terms - auxiliary` of each auxiliary wire.
fn expand_auxiliaries(
    source: &Circuit,
    rewritten: &RewrittenCircuit,
    identity: &[Linear],
) -> Result<(Vec<Linear>, Vec<Equation>)> {
    let mut expanded = identity[..source.wires as usize].to_vec();
    let mut equations = vec![];
    for (index, auxiliary) in rewritten.auxiliaries.iter().enumerate() {
        ensure(
            auxiliary
                .id
                .checked_sub(source.wires)
                .map(|offset| offset as usize)
                == Some(index),
            format_args!("auxiliary wire {index}: ids must follow the source wires in order"),
        )?;
        ensure(
            auxiliary.terms.len() >= 2,
            format_args!("auxiliary wire {index}: needs at least two terms"),
        )?;
        // Terms are canonical and reference only earlier wires, so auxiliary wires are acyclic.
        check_canonical(&[auxiliary.terms.clone(), vec![], vec![]], auxiliary.id)
            .map_err(|e| format!("auxiliary wire {index}: {e}"))?;
        ensure(
            auxiliary.terms.iter().all(|(wire, _)| *wire > 0),
            format_args!("auxiliary wire {index}: terms must not use wire 0"),
        )?;
        ensure(
            auxiliary.terms[0].1 .0 == 1,
            format_args!("auxiliary wire {index}: first coefficient must be 1"),
        )?;
        let mut values = BTreeMap::new();
        for (wire, k) in &auxiliary.terms {
            for (source_wire, c) in &expanded[*wire as usize] {
                accumulate(&mut values, *source_wire, mul(k.0, *c));
            }
        }
        ensure_within_limit(
            values.len() <= MAX_AUXILIARY_EXPANSION,
            format_args!("auxiliary wire {index}: expansion exceeds the size limit"),
        )?;
        expanded.push(values);
        let mut terms = auxiliary.terms.clone();
        terms.push((auxiliary.id, FieldElement(P - 1)));
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
    ensure_within_limit(
        a.len().saturating_mul(b.len()) <= MAX_PRODUCT_TERMS,
        "placement check polynomial size limit",
    )?;
    // Collect every product term, then sort and merge equal monomials once:
    // much faster than accumulating into a map when `a` and `b` are long.
    let mut terms = Vec::with_capacity(a.len() * b.len() + c.len());
    for (i, x) in a {
        for (j, y) in b {
            terms.push((((*i).min(*j), (*i).max(*j)), mul(*x, *y)));
        }
    }
    terms.extend(c.iter().map(|(i, z)| ((0, *i), neg(*z))));
    terms.sort_unstable_by_key(|(monomial, _)| *monomial);
    let mut result: Quadratic = Vec::with_capacity(terms.len());
    for (monomial, k) in terms {
        match result.last_mut() {
            Some((last, sum)) if *last == monomial => *sum = add(*sum, k),
            _ => result.push((monomial, k)),
        }
    }
    result.retain(|(_, sum)| *sum != 0);
    Ok(result)
}

/// Check gate type groups, selectors and gates; return every activation of a gate.
fn active_gates(layout: &Layout) -> Result<BTreeSet<Activation>> {
    for (id, group) in layout.gate_types.iter().enumerate() {
        check_gate_type(id, group, layout.width)?;
    }
    for (id, selector) in layout.selectors.iter().enumerate() {
        ensure(
            selector.id == id,
            format_args!("selector {id}: id out of order"),
        )?;
        ensure(
            !selector.rows.is_empty(),
            format_args!("selector {id}: no active rows"),
        )?;
        ensure(
            selector.rows.windows(2).all(|pair| pair[0] < pair[1]),
            format_args!("selector {id}: rows must be in increasing order"),
        )?;
        ensure(
            selector
                .rows
                .iter()
                .all(|row| *row < layout.arithmetic_rows),
            format_args!("selector {id}: row beyond arithmeticRows"),
        )?;
    }
    let mut active = BTreeSet::new();
    let mut gate_keys = BTreeSet::new();
    for (id, gate) in layout.gates.iter().enumerate() {
        ensure(
            gate.gate_type < layout.gate_types.len(),
            format_args!("gate {id}: unknown gate type"),
        )?;
        ensure(
            gate.selector < layout.selectors.len(),
            format_args!("gate {id}: unknown selector"),
        )?;
        ensure(
            gate.rows == layout.selectors[gate.selector].rows,
            format_args!("gate {id}: rows differ from its selector's rows"),
        )?;
        ensure(
            gate.columns.len() == layout.gate_types[gate.gate_type].width,
            format_args!("gate {id}: column count differs from its gate type width"),
        )?;
        ensure(
            gate.columns.iter().all(|column| *column < layout.width),
            format_args!("gate {id}: column out of range"),
        )?;
        ensure(
            gate.columns.iter().collect::<BTreeSet<_>>().len() == gate.columns.len(),
            format_args!("gate {id}: repeated column"),
        )?;
        ensure(
            gate_keys.insert((gate.gate_type, gate.columns.clone())),
            format_args!("gate {id}: duplicate of an earlier gate"),
        )?;
        for row in &gate.rows {
            active.insert((gate.gate_type, gate.columns.clone(), *row));
        }
    }
    for id in 0..layout.selectors.len() {
        ensure(
            layout.gates.iter().any(|gate| gate.selector == id),
            format_args!("selector {id}: no gate uses it"),
        )?;
    }
    Ok(active)
}

/// GateType ids match their positions; every term refers to the constant (position -1) or
/// a position of the gate type, in increasing order, with a coefficient in `1..P`.
fn check_gate_type(id: usize, group: &GateType, width: usize) -> Result<()> {
    ensure(
        group.id == id,
        format_args!("gate type {id}: id out of order"),
    )?;
    ensure(
        group.width <= width,
        format_args!("gate type {id}: wider than the layout"),
    )?;
    for lc in &group.terms {
        let mut last = None;
        for (position, k) in lc {
            ensure(
                *position == -1 || usize::try_from(*position).is_ok_and(|p| p < group.width),
                format_args!("gate type {id}: position out of range"),
            )?;
            ensure(
                k.0 > 0 && k.0 < P,
                format_args!("gate type {id}: coefficient must be in 1..P"),
            )?;
            ensure(
                last.is_none_or(|previous| *position > previous),
                format_args!("gate type {id}: positions must be in increasing order"),
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
            format_args!("cell ({row}, {col}) out of range"),
        )?;
        ensure(
            wire > 0 && wire < self.wires,
            format_args!("wire {wire} cannot occupy a cell"),
        )?;
        let old = &mut self.map[row * self.width + col];
        ensure(
            *old == 0 || *old == wire,
            format_args!("cell ({row}, {col}) would hold two different wires"),
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
/// and returns the last row holding an equation, plus one.
fn place_equations(
    layout: &Layout,
    equations: &[Equation],
    identity: &[Linear],
    active: &BTreeSet<Activation>,
    cells: &mut Cells,
) -> Result<usize> {
    ensure(
        layout.placements.len() == equations.len(),
        "one placement per auxiliary wire and rewritten equation",
    )?;
    let mut seen = BTreeSet::new();
    let mut used = BTreeSet::new();
    let mut arithmetic_rows = 0;
    for placement in &layout.placements {
        check_placement(layout, placement, equations, identity, &mut seen)?;
        let key = (
            placement.gate_type,
            placement.columns.clone(),
            placement.row,
        );
        ensure(
            active.contains(&key),
            format_args!(
                "equation {} is placed on a row where its gate is not active",
                placement.index
            ),
        )?;
        used.insert(key);
        for (wire, column) in placement.wires.iter().zip(&placement.columns) {
            cells.set(placement.row, *column, *wire)?;
        }
        arithmetic_rows = arithmetic_rows.max(placement.row + 1);
    }
    // The counts match and no index repeats, so every equation is placed;
    // now every active gate row must hold one.
    ensure(
        *active == used,
        "placements must match the active gate rows exactly",
    )?;
    for id in 0..layout.gate_types.len() {
        ensure(
            layout
                .placements
                .iter()
                .any(|placement| placement.gate_type == id),
            format_args!("gate type {id}: no equation uses it"),
        )?;
    }
    ensure(
        arithmetic_rows == layout.arithmetic_rows,
        "arithmeticRows must be the last row holding an equation, plus one",
    )?;
    Ok(arithmetic_rows)
}

/// One placement: a new equation index, a known gate type and row, consistent
/// sizes, and a gate type that gives back the placed equation.
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
        format_args!("placement of unknown equation {index}"),
    )?;
    ensure(
        seen.insert(index),
        format_args!("equation {index} is placed twice"),
    )?;
    ensure(
        placement.gate_type < layout.gate_types.len(),
        format_args!("equation {index}: unknown gate type"),
    )?;
    ensure(
        placement.row < layout.arithmetic_rows,
        format_args!("equation {index}: row beyond arithmeticRows"),
    )?;
    let group = &layout.gate_types[placement.gate_type];
    ensure(
        placement.wires.len() == group.width && placement.columns.len() == group.width,
        format_args!("equation {index}: wire or column count differs from its gate type width"),
    )?;
    ensure(
        placement.columns.iter().collect::<BTreeSet<_>>().len() == group.width,
        format_args!("equation {index}: repeated column"),
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
        format_args!("equation {index}: its gate type and wires give a different equation"),
    )
}
