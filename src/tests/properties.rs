//! Property tests over random circuits: every compiled artifact passes its
//! checker, every witness of a satisfiable circuit projects, an honest trace
//! satisfies the AIR (completeness), every trace that satisfies the AIR encodes
//! a satisfying assignment (soundness), a changed artifact is accepted only if
//! its constraint polynomials did not change, and malformed input never
//! panics. Set `PROPTEST_CASES` to run more cases than the defaults below.
use super::{
    air::{self, Failure, Trace},
    changed_pilout, r1cs_with_terms,
};
use crate::{
    api, checker, compiler,
    field::{add, inverse, mul},
    layout::LayoutConfig,
    packed, packed_check, proto as pb, r1cs,
    rewrite::{Reuse, RewriteOptions},
    sha256_hex, P,
};
use proptest::prelude::*;
use proptest::sample::subsequence;

/// R1CS header counts and equations as `(wire, coefficient)` terms.
#[derive(Clone, Debug)]
struct Circuit {
    header: [u32; 4],
    equations: Vec<[Vec<(u32, u64)>; 3]>,
}
impl Circuit {
    fn bytes(&self) -> Vec<u8> {
        r1cs_with_terms(self.header, &self.equations)
    }
}

/// A nonzero coefficient, mixing small values, `P - 1` and large values.
fn coefficient() -> impl Strategy<Value = u64> {
    prop_oneof![1..5u64, Just(P - 1), 1..P]
}

/// A linear combination of 1 to 4 distinct wires below `wires`.
fn linear_combination(wires: u32) -> impl Strategy<Value = Vec<(u32, u64)>> {
    let count = 4.min(wires as usize);
    subsequence((0..wires).collect::<Vec<_>>(), 1..=count)
        .prop_flat_map(|chosen| {
            let n = chosen.len();
            (Just(chosen), proptest::collection::vec(coefficient(), n))
        })
        .prop_map(|(chosen, coefficients)| chosen.into_iter().zip(coefficients).collect())
}

/// A circuit with 4 to 24 wires, up to 3 public wires and 1 to 12 equations
/// with nonzero coefficients. Most such circuits have no satisfying assignment;
/// compiling and checking does not need one.
fn circuit() -> impl Strategy<Value = Circuit> {
    (4u32..=24, 0u32..=3, 1usize..=12)
        .prop_flat_map(|(wires, publics, equations)| {
            let publics = publics.min(wires - 2);
            let factor = || linear_combination(wires);
            let equation = (factor(), factor(), factor()).prop_map(|(a, b, c)| [a, b, c]);
            (
                Just(wires),
                0..=publics,
                Just(publics),
                proptest::collection::vec(equation, equations),
            )
        })
        .prop_map(|(wires, outputs, publics, equations)| Circuit {
            header: [wires, outputs, publics - outputs, wires - 1 - publics],
            equations,
        })
}

/// A circuit and a satisfying assignment: wires `1..=inputs` get random values,
/// and each later wire `r` is the result of one equation `A · B = c · w_r`,
/// with `A` and `B` over earlier wires.
fn satisfiable_circuit() -> impl Strategy<Value = (Circuit, Vec<u64>)> {
    (2u32..=6, 1u32..=10, 0u32..=2)
        .prop_flat_map(|(inputs, results, publics)| {
            let publics = publics.min(inputs);
            let values = proptest::collection::vec(0..P, inputs as usize);
            let equations = (0..results)
                .map(|i| {
                    let earlier = 1 + inputs + i;
                    (
                        linear_combination(earlier),
                        linear_combination(earlier),
                        coefficient(),
                    )
                })
                .collect::<Vec<_>>();
            (Just((inputs, results, publics)), values, equations)
        })
        .prop_map(|((inputs, results, publics), inputs_values, factors)| {
            let wires = 1 + inputs + results;
            let mut values = vec![1];
            values.extend(inputs_values);
            let mut equations = vec![];
            for (i, (a, b, c)) in factors.into_iter().enumerate() {
                let result = 1 + inputs + i as u32;
                let product = mul(linear(&a, &values), linear(&b, &values));
                values.push(mul(product, inverse(c)));
                equations.push([a, b, vec![(result, c)]]);
            }
            let header = [wires, 0, publics, wires - 1 - publics];
            (Circuit { header, equations }, values)
        })
}

/// Value of a linear combination for an assignment with `values[0] = 1`.
fn linear(lc: &[(u32, u64)], values: &[u64]) -> u64 {
    lc.iter().fold(0, |sum, (w, k)| {
        crate::field::add(sum, mul(values[*w as usize], *k))
    })
}

/// Whether every equation `A · B = C` of `circuit` holds for `values`.
fn satisfies(circuit: &Circuit, values: &[u64]) -> bool {
    values[0] == 1
        && circuit
            .equations
            .iter()
            .all(|[a, b, c]| mul(linear(a, values), linear(b, values)) == linear(c, values))
}

fn layout_config() -> impl Strategy<Value = LayoutConfig> {
    (3usize..=12, any::<bool>(), 0usize..3, any::<bool>())
        .prop_flat_map(|(width, fold_constants, reuse, wide)| {
            (Just((width, fold_constants, reuse, wide)), 2..width)
        })
        .prop_map(
            |((width, fold_constants, reuse, wide), auxiliary_terms)| LayoutConfig {
                options: RewriteOptions {
                    width,
                    auxiliary_terms,
                    reuse_auxiliaries: [Reuse::None, Reuse::Prefix, Reuse::Available][reuse],
                    fold_constants,
                },
                min_rows: 64,
                row_group_width: wide.then_some(2 * width),
            },
        )
}

/// A change that a dishonest prover might make to an honest stage-one trace.
#[derive(Clone, Debug)]
enum Tamper {
    /// Set the `n`-th cell to a value.
    Cell(usize, u64),
    /// Set every cell of the `n`-th placed wire (auxiliary wires included) to a value,
    /// which leaves the permutation argument satisfied.
    Wire(usize, u64),
    /// Set the `n`-th public value.
    Public(usize, u64),
}

fn tamper() -> impl Strategy<Value = Tamper> {
    prop_oneof![
        (any::<usize>(), 0..P).prop_map(|(n, v)| Tamper::Cell(n, v)),
        (any::<usize>(), 0..P).prop_map(|(n, v)| Tamper::Wire(n, v)),
        (any::<usize>(), 0..P).prop_map(|(n, v)| Tamper::Public(n, v)),
    ]
}

/// Apply `tamper` to a trace whose cell `i` holds wire `wires[i]` (0 for an
/// unused cell).
fn apply_tamper(tamper: &Tamper, trace: &mut [u64], publics: &mut [u64], wires: &[u32]) {
    match *tamper {
        Tamper::Cell(n, value) => trace[n % trace.len()] = value,
        Tamper::Wire(n, value) => {
            let mut placed: Vec<u32> = wires.iter().copied().filter(|w| *w != 0).collect();
            placed.sort_unstable();
            placed.dedup();
            if let Some(wire) = placed.get(n % placed.len().max(1)) {
                for (cell, w) in trace.iter_mut().zip(wires) {
                    if w == wire {
                        *cell = value;
                    }
                }
            }
        }
        Tamper::Public(n, value) => {
            if !publics.is_empty() {
                let count = publics.len();
                publics[n % count] = value;
            }
        }
    }
}

/// A change that a dishonest prover might make to the stage-two columns that
/// the hints compute from an honest trace.
#[derive(Clone, Debug)]
enum StageTwoChange {
    /// Set every value of the `n`-th column to 0.
    Zero(usize),
    /// Multiply every value of the `n`-th column by a factor other than 1.
    Scale(usize, u64),
    /// Set one value of the `n`-th column.
    Cell(usize, usize, u64),
}

fn stage_two_change() -> impl Strategy<Value = StageTwoChange> {
    prop_oneof![
        any::<usize>().prop_map(StageTwoChange::Zero),
        (any::<usize>(), 2..P).prop_map(|(n, k)| StageTwoChange::Scale(n, k)),
        (any::<usize>(), any::<usize>(), 0..P).prop_map(|(n, r, v)| StageTwoChange::Cell(n, r, v)),
    ]
}

/// Apply `change` to the stage-two columns. A change that leaves them as they
/// were (zeroing or scaling a zero column) adds 1 to the column's first value
/// instead.
fn apply_stage_two_change(change: &StageTwoChange, columns: &mut [Vec<u64>]) {
    let count = columns.len();
    let before = columns.to_vec();
    match *change {
        StageTwoChange::Zero(n) => columns[n % count].fill(0),
        StageTwoChange::Scale(n, k) => {
            for value in &mut columns[n % count] {
                *value = mul(*value, k);
            }
        }
        StageTwoChange::Cell(n, row, value) => {
            let column = &mut columns[n % count];
            let rows = column.len();
            column[row % rows] = value;
        }
    }
    if columns == before.as_slice() {
        let n = match *change {
            StageTwoChange::Zero(n)
            | StageTwoChange::Scale(n, _)
            | StageTwoChange::Cell(n, _, _) => n,
        };
        let column = &mut columns[n % count];
        column[0] = add(column[0], 1);
    }
}

/// Two random stage-two challenges.
fn challenges() -> impl Strategy<Value = [u64; 2]> {
    (0..P, 0..P).prop_map(|(alpha, gamma)| [alpha, gamma])
}

/// The honest reference trace: every row holds the whole assignment.
fn reference_trace(columns: &[Option<u32>], width: usize, rows: usize, values: &[u64]) -> Vec<u64> {
    let mut row = vec![0; width];
    for (wire, column) in columns.iter().enumerate() {
        if let Some(column) = column {
            row[*column as usize] = values[wire];
        }
    }
    row.repeat(rows)
}

/// For a stage-one trace that satisfies a packed AIR: read every source wire
/// from its cells, which must agree, and require the assignment to satisfy
/// the circuit with the trace's public values.
fn check_packed_read_back(circuit: &Circuit, map: &[u32], trace: &[u64], publics: &[u64]) {
    let wires = circuit.header[0] as usize;
    let mut values: Vec<Option<u64>> = vec![None; wires];
    values[0] = Some(1);
    for (wire, value) in map.iter().zip(trace) {
        let wire = *wire as usize;
        if wire == 0 || wire >= wires {
            continue;
        }
        let previous = values[wire].replace(*value);
        assert!(
            previous.is_none_or(|p| p == *value),
            "cells of wire {wire} disagree in a trace that satisfies the AIR"
        );
    }
    // A wire without a cell appears in no equation, so any value will do;
    // a public one takes its public value.
    for (index, public) in publics.iter().enumerate() {
        values[1 + index].get_or_insert(*public);
    }
    let values: Vec<u64> = values.into_iter().map(|v| v.unwrap_or(0)).collect();
    assert!(
        satisfies(circuit, &values),
        "the read-back assignment fails the circuit"
    );
    assert_eq!(&values[1..=publics.len()], publics, "public values differ");
}

/// One change to a PILout: a constant, a column reference or a fixed value.
#[derive(Clone, Debug)]
enum Change {
    /// Replace the value of the `n`-th constant operand.
    Constant(usize),
    /// Point the `n`-th stage-one operand at another column.
    TraceColumn(usize),
    /// Change one value of the `n`-th fixed column.
    FixedValue(usize, usize),
}

fn change() -> impl Strategy<Value = Change> {
    prop_oneof![
        any::<usize>().prop_map(Change::Constant),
        any::<usize>().prop_map(Change::TraceColumn),
        (any::<usize>(), any::<usize>()).prop_map(|(c, r)| Change::FixedValue(c, r)),
    ]
}

/// Every operand of every expression, in order.
fn operands(pilout: &mut pb::PilOut) -> Vec<&mut pb::Operand> {
    use pb::expression::Operation;
    let mut result = vec![];
    for expression in &mut pilout.air_groups[0].airs[0].expressions {
        match &mut expression.operation {
            Some(Operation::Add(x)) => result.extend([x.lhs.as_mut(), x.rhs.as_mut()]),
            Some(Operation::Sub(x)) => result.extend([x.lhs.as_mut(), x.rhs.as_mut()]),
            Some(Operation::Mul(x)) => result.extend([x.lhs.as_mut(), x.rhs.as_mut()]),
            Some(Operation::Neg(x)) => result.push(x.value.as_mut()),
            None => {}
        }
    }
    result.into_iter().flatten().collect()
}

/// Apply `change` to `pilout`. A PILout without stage-one operands or fixed
/// columns gets a constant changed instead; returns false only if it has no
/// constants either.
fn apply(change: &Change, pilout: &mut pb::PilOut, width: u32) -> bool {
    use pb::operand::Operand;
    match *change {
        Change::Constant(n) => {
            let mut constants: Vec<_> = operands(pilout)
                .into_iter()
                .filter_map(|o| match &mut o.operand {
                    Some(Operand::Constant(c)) => Some(c),
                    _ => None,
                })
                .collect();
            if constants.is_empty() {
                return false;
            }
            let count = constants.len();
            let constant = &mut constants[n % count];
            let value = constant
                .value
                .iter()
                .fold(0u64, |v, b| (v << 8) | u64::from(*b));
            let changed = (value + 1) % P;
            constant.value = crate::dag::field_bytes(changed);
            true
        }
        Change::TraceColumn(n) => {
            let mut columns: Vec<_> = operands(pilout)
                .into_iter()
                .filter_map(|o| match &mut o.operand {
                    Some(Operand::WitnessCol(w)) if w.stage == 1 => Some(w),
                    _ => None,
                })
                .collect();
            if columns.is_empty() || width < 2 {
                return apply(&Change::Constant(n), pilout, width);
            }
            let count = columns.len();
            let column = &mut columns[n % count];
            column.col_idx = (column.col_idx + 1 + (n as u32 % (width - 1))) % width;
            true
        }
        Change::FixedValue(column, row) => {
            if pilout.air_groups[0].airs[0].fixed_cols.is_empty() {
                return apply(&Change::Constant(column), pilout, width);
            }
            let fixed = &mut pilout.air_groups[0].airs[0].fixed_cols;
            let count = fixed.len();
            let values = &mut fixed[column % count].values;
            let rows = values.len();
            let value = &mut values[row % rows];
            let old = value.iter().fold(0u64, |v, b| (v << 8) | u64::from(*b));
            *value = crate::dag::field_bytes((old + 1) % P);
            true
        }
    }
}

fn decode(pilout: &[u8]) -> pb::PilOut {
    <pb::PilOut as prost::Message>::decode(pilout).unwrap()
}

fn constraint_count(pilout: &[u8]) -> usize {
    decode(pilout).air_groups[0].airs[0].constraints.len()
}

/// Number of constraints before the permutation argument's, which come last and
/// are labeled `copy ...`.
fn gate_and_public_constraints(pilout: &[u8]) -> usize {
    decode(pilout).air_groups[0].airs[0]
        .constraints
        .iter()
        .filter(|c| match &c.constraint {
            Some(pb::constraint::Constraint::EveryRow(row)) => {
                !row.debug_line.as_deref().unwrap_or("").starts_with("copy ")
            }
            _ => true,
        })
        .count()
}

/// Polynomials of the first `count` constraints, which must not use row offsets.
fn constraint_polynomials(
    pilout: &[u8],
    width: u32,
    publics: u32,
    count: usize,
) -> Vec<crate::polynomial::Polynomial> {
    let pilout = decode(pilout);
    let air = &pilout.air_groups[0].airs[0];
    let roots: Vec<usize> = air.constraints[..count]
        .iter()
        .map(|c| match &c.constraint {
            Some(pb::constraint::Constraint::EveryRow(row)) => {
                row.expression_idx.unwrap().idx as usize
            }
            _ => unreachable!("compiled constraints apply to every row"),
        })
        .collect();
    let end = roots.iter().max().map_or(0, |root| root + 1);
    let mut interpreter = crate::polynomial::Interpreter::new(air, width, publics);
    interpreter.evaluate_until(end).unwrap();
    roots
        .iter()
        .map(|root| interpreter.values[*root].clone())
        .collect()
}

/// `default` cases, or the number in the `PROPTEST_CASES` environment variable.
fn cases(default: u32) -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default);
    ProptestConfig::with_cases(cases)
}

proptest! {
    #![proptest_config(cases(64))]

    #[test]
    fn reference_artifacts_of_random_circuits_are_accepted(circuit in circuit(), optimize: bool) {
        let source = circuit.bytes();
        let parsed = r1cs::read(&source).unwrap();
        let (pilout, certificate) = compiler::compile(&source, &parsed, 64, optimize).unwrap();
        prop_assert!(checker::check(&source, &pilout, &certificate).is_ok());
    }

    #[test]
    fn packed_artifacts_of_random_circuits_are_accepted(
        circuit in circuit(),
        settings in layout_config(),
    ) {
        // compile_packed runs the packed checker before returning.
        let result = api::compile_packed(&circuit.bytes(), &settings);
        prop_assert!(result.is_ok(), "{:#}", result.unwrap_err());
    }

    #[test]
    fn witnesses_of_satisfiable_circuits_project(
        (circuit, values) in satisfiable_circuit(),
        settings in layout_config(),
    ) {
        let source = circuit.bytes();
        let artifact = api::compile_packed(&source, &settings).unwrap();
        let wtns = super::wtns_bytes(&values);
        let witness =
            api::project_witness(&source, &artifact.pilout, &artifact.certificate, &wtns).unwrap();
        // Cells of source wires hold their values; auxiliary wire values are computed
        // by the projection, which checks every auxiliary equation.
        for (wire, value) in artifact.map.iter().zip(&witness.trace) {
            let wire = *wire as usize;
            if wire == 0 {
                prop_assert_eq!(*value, 0);
            } else if wire < values.len() {
                prop_assert_eq!(*value, values[wire]);
            }
        }
        let publics = circuit.header[1] + circuit.header[2];
        prop_assert_eq!(&witness.publics[..], &values[1..=publics as usize]);
    }

    #[test]
    fn changed_reference_artifacts_are_rejected_unless_equivalent(
        circuit in circuit(),
        optimize: bool,
        change in change(),
    ) {
        let source = circuit.bytes();
        let parsed = r1cs::read(&source).unwrap();
        let (pilout, mut certificate) = compiler::compile(&source, &parsed, 64, optimize).unwrap();
        let mut applied = false;
        let changed = changed_pilout(&pilout, |p| applied = apply(&change, p, certificate.width));
        prop_assume!(applied);
        certificate.pilout_sha256 = sha256_hex(&changed);
        if checker::check(&source, &changed, &certificate).is_ok() {
            // Accepted only if the change left every constraint polynomial as it
            // was, for example a column whose terms cancel in `A · B - C`.
            let (width, publics) = (certificate.width, parsed.publics);
            let count = constraint_count(&pilout);
            prop_assert_eq!(
                constraint_polynomials(&pilout, width, publics, count),
                constraint_polynomials(&changed, width, publics, count)
            );
        }
    }

    #[test]
    fn changed_packed_artifacts_are_rejected_unless_equivalent(
        circuit in circuit(),
        settings in layout_config(),
        change in change(),
    ) {
        let source = circuit.bytes();
        let mut artifact = packed::compile(&source, &settings).unwrap();
        let width = artifact.certificate.layout.width as u32;
        let mut applied = false;
        let changed = changed_pilout(&artifact.pilout, |p| applied = apply(&change, p, width));
        prop_assume!(applied);
        artifact.certificate.pilout_sha256 = sha256_hex(&changed);
        if packed_check::check(&source, &changed, &artifact.certificate).is_ok() {
            // The checker compares the permutation argument operation by operation, so
            // an accepted change can only have touched the gate and public
            // constraints, and must have left their polynomials as they were.
            let main = gate_and_public_constraints(&artifact.pilout);
            let publics = artifact.certificate.rewritten_circuit.publics;
            prop_assert_eq!(
                constraint_polynomials(&artifact.pilout, width, publics, main),
                constraint_polynomials(&changed, width, publics, main)
            );
        }
    }
}

proptest! {
    #![proptest_config(cases(64))]

    #[test]
    fn honest_reference_traces_satisfy_the_air(
        (circuit, values) in satisfiable_circuit(),
        optimize: bool,
        challenges in challenges(),
    ) {
        let source = circuit.bytes();
        let (pilout, certificate) =
            compiler::compile(&source, &r1cs::read(&source).unwrap(), 64, optimize).unwrap();
        let (rows, width) = (certificate.rows(), certificate.width());
        let stage_one = reference_trace(certificate.wire_columns(), width, rows, &values);
        let publics = &values[1..=circuit.header[2] as usize];
        let trace = Trace { stage_one: &stage_one, publics, challenges };
        prop_assert_eq!(air::evaluate(&decode(&pilout), &trace), Ok(()));
    }

    #[test]
    fn honest_packed_traces_satisfy_the_air(
        (circuit, values) in satisfiable_circuit(),
        settings in layout_config(),
        challenges in challenges(),
    ) {
        let source = circuit.bytes();
        let artifact = api::compile_packed(&source, &settings).unwrap();
        let wtns = super::wtns_bytes(&values);
        let witness =
            api::project_witness(&source, &artifact.pilout, &artifact.certificate, &wtns).unwrap();
        let trace = Trace { stage_one: &witness.trace, publics: &witness.publics, challenges };
        let result = air::evaluate(&decode(&artifact.pilout), &trace);
        prop_assume!(result != Err(Failure::ZeroDenominator));
        prop_assert_eq!(result, Ok(()));
    }

    #[test]
    fn changed_stage_two_columns_violate_the_air(
        (circuit, values) in satisfiable_circuit(),
        settings in layout_config(),
        packed: bool,
        change in stage_two_change(),
        challenges in challenges(),
    ) {
        // Filling stage two from the hints covers every prover only if no
        // other stage-two columns satisfy the AIR; see `air.rs`.
        let source = circuit.bytes();
        let (pilout, stage_one, publics) = if packed {
            let artifact = api::compile_packed(&source, &settings).unwrap();
            let wtns = super::wtns_bytes(&values);
            let witness = api::project_witness(&source, &artifact.pilout, &artifact.certificate, &wtns)
                .unwrap();
            (artifact.pilout, witness.trace, witness.publics)
        } else {
            let (pilout, certificate) =
                compiler::compile(&source, &r1cs::read(&source).unwrap(), 64, false).unwrap();
            let (rows, width) = (certificate.rows(), certificate.width());
            let stage_one = reference_trace(certificate.wire_columns(), width, rows, &values);
            (pilout, stage_one, values[1..=circuit.header[2] as usize].to_vec())
        };
        let trace = Trace { stage_one: &stage_one, publics: &publics, challenges };
        let pilout = decode(&pilout);
        let result =
            air::evaluate_changed(&pilout, &trace, |columns| apply_stage_two_change(&change, columns));
        prop_assume!(result != Err(Failure::ZeroDenominator));
        prop_assert!(matches!(result, Err(Failure::Violated { .. })), "{result:?}");
    }

    #[test]
    fn reference_traces_that_satisfy_the_air_encode_satisfying_assignments(
        (circuit, values) in satisfiable_circuit(),
        optimize: bool,
        tamper in tamper(),
        challenges in challenges(),
    ) {
        let source = circuit.bytes();
        let (pilout, certificate) =
            compiler::compile(&source, &r1cs::read(&source).unwrap(), 64, optimize).unwrap();
        let (rows, width) = (certificate.rows(), certificate.width());
        let columns = certificate.wire_columns();
        let mut stage_one = reference_trace(columns, width, rows, &values);
        let mut publics = values[1..=circuit.header[2] as usize].to_vec();
        // The wire in each cell, row by row.
        let mut wire_of_column = vec![0; width];
        for (wire, column) in columns.iter().enumerate() {
            if let Some(column) = column {
                wire_of_column[*column as usize] = wire as u32;
            }
        }
        let wires = wire_of_column.repeat(rows);
        apply_tamper(&tamper, &mut stage_one, &mut publics, &wires);
        let trace = Trace { stage_one: &stage_one, publics: &publics, challenges };
        if air::evaluate(&decode(&pilout), &trace).is_ok() {
            // Every row's assignment satisfies the circuit; row 0 carries the
            // public values.
            for row in stage_one.chunks(width) {
                let mut assignment = vec![1; columns.len()];
                for (wire, column) in columns.iter().enumerate().skip(1) {
                    assignment[wire] = column.map_or(0, |c| row[c as usize]);
                }
                prop_assert!(satisfies(&circuit, &assignment));
            }
            let first: Vec<u64> = (1..=publics.len())
                .map(|wire| columns[wire].map_or(0, |c| stage_one[c as usize]))
                .collect();
            prop_assert_eq!(first, publics);
        }
    }

    #[test]
    fn packed_traces_that_satisfy_the_air_encode_satisfying_assignments(
        (circuit, values) in satisfiable_circuit(),
        settings in layout_config(),
        tamper in tamper(),
        challenges in challenges(),
    ) {
        let source = circuit.bytes();
        let artifact = api::compile_packed(&source, &settings).unwrap();
        let wtns = super::wtns_bytes(&values);
        let witness =
            api::project_witness(&source, &artifact.pilout, &artifact.certificate, &wtns).unwrap();
        let (mut stage_one, mut publics) = (witness.trace, witness.publics);
        apply_tamper(&tamper, &mut stage_one, &mut publics, &artifact.map);
        let trace = Trace { stage_one: &stage_one, publics: &publics, challenges };
        let result = air::evaluate(&decode(&artifact.pilout), &trace);
        prop_assume!(result != Err(Failure::ZeroDenominator));
        if result.is_ok() {
            check_packed_read_back(&circuit, &artifact.map, &stage_one, &publics);
        }
    }
}

proptest! {
    #![proptest_config(cases(256))]

    #[test]
    fn arbitrary_bytes_never_panic_the_parsers(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = r1cs::read(&bytes);
        let _ = r1cs::read_witness(&bytes, 4);
        let _ = crate::pilout_check::decode(&bytes);
    }

    #[test]
    fn changed_bytes_never_panic_the_checkers(
        circuit in circuit(),
        settings in layout_config(),
        position: usize,
        byte: u8,
        target in 0usize..4,
    ) {
        let source = circuit.bytes();
        let artifact = packed::compile(&source, &settings).unwrap();
        let mut certificate_json = serde_json::to_vec(&artifact.certificate).unwrap();
        let reference = compiler::compile(&source, &r1cs::read(&source).unwrap(), 64, false).unwrap();
        let mut reference_pilout = reference.0;
        let mut packed_pilout = artifact.pilout.clone();
        let mut changed_source = source.clone();
        let file = match target {
            0 => &mut changed_source,
            1 => &mut reference_pilout,
            2 => &mut packed_pilout,
            _ => &mut certificate_json,
        };
        let index = position % file.len();
        file[index] = byte;
        let _ = checker::check(&changed_source, &reference_pilout, &reference.1);
        if let Ok(certificate) = serde_json::from_slice(&certificate_json) {
            let _ = packed_check::check(&changed_source, &packed_pilout, &certificate);
            let _ = crate::witness::project(&changed_source, &certificate, &[]);
        }
    }
}
