//! Property tests over random circuits: every compiled artifact passes its
//! checker, every witness of a satisfiable circuit projects, a changed artifact
//! is accepted only if its constraint polynomials did not change, and malformed
//! input never panics. Set
//! `PROPTEST_CASES` to run more cases than the defaults below.
use super::{changed_pilout, r1cs_with_terms};
use crate::{
    api, checker, compiler,
    field::{inverse, mul},
    layout::{LayoutConfig, Strategy as Placement},
    network::{ConversionOptions, Reuse},
    packed, packed_check, proto as pb, r1cs, sha256_hex, P,
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
            let evaluate = |lc: &[(u32, u64)], values: &[u64]| {
                lc.iter().fold(0, |sum, (w, k)| {
                    crate::field::add(sum, mul(values[*w as usize], *k))
                })
            };
            let mut equations = vec![];
            for (i, (a, b, c)) in factors.into_iter().enumerate() {
                let result = 1 + inputs + i as u32;
                let product = mul(evaluate(&a, &values), evaluate(&b, &values));
                values.push(mul(product, inverse(c)));
                equations.push([a, b, vec![(result, c)]]);
            }
            let header = [wires, 0, publics, wires - 1 - publics];
            (Circuit { header, equations }, values)
        })
}

fn layout_config() -> impl Strategy<Value = LayoutConfig> {
    (3usize..=12, any::<bool>(), 0usize..3, 0usize..3)
        .prop_flat_map(|(width, fold, reuse, strategy)| {
            (Just((width, fold, reuse, strategy)), 2..width)
        })
        .prop_map(|((width, fold, reuse, strategy), chunk)| LayoutConfig {
            options: ConversionOptions {
                width,
                chunk,
                reuse: [Reuse::None, Reuse::Prefix, Reuse::Available][reuse],
                fold,
            },
            strategy: [Placement::Mixed, Placement::Local, Placement::Components][strategy],
            min_rows: 64,
        })
}

/// One change to a PILout that must change what it encodes.
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
        // Cells of source wires hold their values; helper values are computed
        // by the projection, which checks every helper equation.
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
            // The checker compares the copy argument operation by operation, so
            // an accepted change can only have touched the slot and public
            // constraints, and must have left their polynomials as they were.
            let layout = &artifact.certificate.layout;
            let ratios = if layout.width > 3 { ((layout.width - 1) / 4).max(1) } else { 0 };
            let copy = if layout.requires_copy { ratios + 2 } else { 0 };
            let main = constraint_count(&artifact.pilout) - copy;
            let publics = artifact.certificate.network.publics;
            prop_assert_eq!(
                constraint_polynomials(&artifact.pilout, width, publics, main),
                constraint_polynomials(&changed, width, publics, main)
            );
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
