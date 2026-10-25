//! Checker for reference artifacts. It decodes the PILout, evaluates every
//! expression to an exact polynomial and compares each constraint with the one
//! expected from the source. It does not call the compiler. The checks are
//! tests run by a program, not a machine-checked proof.
use crate::{
    contract::{
        label, AIR_NAME, ALPHA, CHALLENGE_NAMES, FIRST_COLUMN, GAMMA, MAX_ROWS, MIN_ROWS,
        PRODUCT_COLUMN, PUBLICS_SYMBOL, REFERENCE_RELATION, STAGE_ONE, STAGE_TWO, SUM_HINT,
        TRACE_SYMBOL, ZERO_COLUMN_NAME,
    },
    ensure,
    pilout_check::{self, scalar, BinaryOp, Binding, ExpectedSymbol, Geometry, Pattern},
    polynomial::{accumulate, difference, product, variable, Interpreter, Polynomial, Var},
    proto as pb, r1cs, ReferenceCertificate, Result,
};
use pb::SymbolType;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Result of a successful reference check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ReferenceCheckReport {
    /// Always true: a failed check returns an error instead.
    pub accepted: bool,
    /// Name of the checked relation.
    pub relation: String,
    /// Number of equations in the source circuit.
    pub source_equations: usize,
    /// Number of public values.
    pub public_bindings: usize,
    /// Number of trace rows.
    pub rows: usize,
    /// Number of stage-one columns.
    pub width: usize,
    /// Always false: the check is not a formal proof.
    pub formally_verified: bool,
}

/// Check reference PILout bytes against their source, in this order:
/// 1. certificate version, relation, hashes, geometry, wire map and public order;
/// 2. PILout field, names, geometry, `FIRST` column and symbols;
/// 3. every constraint as an exact polynomial: `stage2_zero = 0`, each source
///    equation under the wire map, and each public binding at row 0;
/// 4. the hint of the `stage2_zero` column.
pub fn check(
    source: &[u8],
    pilout_bytes: &[u8],
    certificate: &ReferenceCertificate,
) -> Result<ReferenceCheckReport> {
    let binding = Binding {
        version: certificate.version,
        relation: &certificate.relation,
        source_sha256: &certificate.source_sha256,
        pilout_sha256: &certificate.pilout_sha256,
    };
    pilout_check::check_binding(&binding, REFERENCE_RELATION, source, pilout_bytes)?;
    let circuit = r1cs::read(source)?;
    check_mapping(&circuit, certificate)?;

    let pilout = pilout_check::decode(pilout_bytes)?;
    let air = pilout_check::check_envelope(
        &pilout,
        &Geometry {
            rows: certificate.rows,
            stage_widths: [certificate.width, 1],
            publics: circuit.publics,
        },
    )?;
    check_first_column(air, certificate.rows, circuit.publics)?;
    pilout_check::check_symbols(
        &pilout,
        expected_symbols(certificate.width, circuit.publics),
    )?;

    let mut interpreter = Interpreter::new(air, certificate.width, circuit.publics);
    interpreter.evaluate_until(air.expressions.len())?;
    let expected = expected_constraints(&circuit, certificate)?;
    let labels: Vec<_> = [label::ZERO_COLUMN.to_string()]
        .into_iter()
        .chain((0..circuit.equations.len()).map(label::source_equation))
        .chain((0..circuit.publics as usize).map(label::public))
        .collect();
    let roots = pilout_check::constraint_roots(air, &labels)?;
    for (index, (root, expected)) in roots.iter().zip(&expected).enumerate() {
        ensure(
            interpreter.values.get(*root) == Some(expected),
            format_args!("constraint {index} differs from the expected polynomial"),
        )?;
    }
    check_zero_hint(&pilout, air)?;

    Ok(ReferenceCheckReport {
        accepted: true,
        relation: certificate.relation.clone(),
        source_equations: circuit.equations.len(),
        public_bindings: circuit.publics as usize,
        rows: certificate.rows(),
        width: certificate.width(),
        formally_verified: false,
    })
}

/// Rows, width, a one-to-one map from nonconstant wires to columns, and public
/// wires `1..=publics` in order.
fn check_mapping(circuit: &r1cs::Circuit, certificate: &ReferenceCertificate) -> Result<()> {
    ensure(
        certificate.rows.is_power_of_two()
            && (MIN_ROWS..=MAX_ROWS).contains(&(certificate.rows as usize)),
        format_args!("rows must be a power of two in {MIN_ROWS}..={MAX_ROWS}"),
    )?;
    ensure(circuit.wires > 1, "circuit has no nonconstant wires")?;
    ensure(
        certificate.width == circuit.wires - 1
            && certificate.wire_columns.len() == circuit.wires as usize
            && certificate.wire_columns[0].is_none(),
        "wire map dimensions",
    )?;
    let columns: BTreeSet<_> = certificate.wire_columns[1..].iter().copied().collect();
    ensure(
        columns == (0..certificate.width).map(Some).collect(),
        "each nonconstant wire needs its own column",
    )?;
    ensure(
        certificate.public_wires == (1..=circuit.publics).collect::<Vec<_>>(),
        "public wires must be 1..=publics in order",
    )
}

/// With public values, the only fixed column is `FIRST`: 1 on row 0, 0 elsewhere.
fn check_first_column(air: &pb::Air, rows: u32, publics: u32) -> Result<()> {
    ensure(
        air.fixed_cols.len() == usize::from(publics > 0),
        "fixed column count",
    )?;
    if let Some(first) = air.fixed_cols.first() {
        ensure(first.values.len() == rows as usize, "FIRST column length")?;
        for (row, value) in first.values.iter().enumerate() {
            ensure(scalar(value)? == u64::from(row == 0), "FIRST column values")?;
        }
    }
    Ok(())
}

fn expected_symbols(width: u32, publics: u32) -> BTreeMap<String, ExpectedSymbol> {
    let mut symbols = BTreeMap::from([
        (
            TRACE_SYMBOL.to_string(),
            ExpectedSymbol::air(SymbolType::WitnessCol, 0, STAGE_ONE, vec![width]),
        ),
        (
            ZERO_COLUMN_NAME.to_string(),
            ExpectedSymbol::air(SymbolType::WitnessCol, 0, STAGE_TWO, vec![]),
        ),
        (
            CHALLENGE_NAMES[ALPHA as usize].to_string(),
            ExpectedSymbol::challenge(ALPHA, STAGE_TWO),
        ),
        (
            CHALLENGE_NAMES[GAMMA as usize].to_string(),
            ExpectedSymbol::challenge(GAMMA, STAGE_TWO),
        ),
    ]);
    if publics > 0 {
        symbols.insert(PUBLICS_SYMBOL.into(), ExpectedSymbol::publics(publics));
        symbols.insert(
            format!("{AIR_NAME}.{FIRST_COLUMN}"),
            ExpectedSymbol::air(SymbolType::FixedCol, 0, 0, vec![]),
        );
    }
    symbols
}

/// A source linear combination over the certificate's wire columns.
fn source_linear_combination(
    terms: &r1cs::LinearCombination,
    certificate: &ReferenceCertificate,
) -> Polynomial {
    let mut result = BTreeMap::new();
    for (wire, k) in terms {
        let key = match certificate.wire_columns[*wire as usize] {
            None => vec![],
            Some(column) => vec![Var::trace(column)],
        };
        accumulate(&mut result, key, k.0);
    }
    result
}

/// Constraint polynomials in PILout order: `stage2_zero`, `A*B - C` for each
/// source equation, then `FIRST * (wire - public)` for each public value.
fn expected_constraints(
    circuit: &r1cs::Circuit,
    certificate: &ReferenceCertificate,
) -> Result<Vec<Polynomial>> {
    let mut expected = vec![variable(Var::product())];
    for [a, b, c] in &circuit.equations {
        expected.push(difference(
            &product(
                &source_linear_combination(a, certificate),
                &source_linear_combination(b, certificate),
            )?,
            &source_linear_combination(c, certificate),
        ));
    }
    for (public, column) in certificate.wire_columns[1..=circuit.publics as usize]
        .iter()
        .flatten()
        .enumerate()
    {
        expected.push(product(
            &variable(Var::Fixed(0)),
            &difference(
                &variable(Var::trace(*column)),
                &variable(Var::Public(public as u32)),
            ),
        )?);
    }
    Ok(expected)
}

/// The single `gsum_col` hint of the `stage2_zero` column, compared by
/// structure: `reference` is `z + 0` and `numerator_air` is `α - α`.
fn check_zero_hint(pilout: &pb::PilOut, air: &pb::Air) -> Result<()> {
    let [fields] = &pilout_check::hint_fields(pilout, &[SUM_HINT])?[..] else {
        unreachable!("hint_fields checks the hint count")
    };
    let z = Pattern::Witness(STAGE_TWO, PRODUCT_COLUMN);
    let expected = BTreeMap::from([
        (
            "reference",
            Pattern::op(BinaryOp::Add, z, Pattern::Constant(0)),
        ),
        (
            "numerator_air",
            Pattern::op(
                BinaryOp::Sub,
                Pattern::Challenge(ALPHA),
                Pattern::Challenge(ALPHA),
            ),
        ),
        ("denominator_air", Pattern::Constant(1)),
        ("numerator_direct", Pattern::Constant(0)),
        ("denominator_direct", Pattern::Constant(1)),
    ]);
    pilout_check::check_field_names(SUM_HINT, fields, &expected)?;
    for (name, operand) in fields {
        let pattern = &expected[name];
        ensure(
            pilout_check::matches(air, operand, pattern)?,
            format_args!("hint '{SUM_HINT}': field '{name}' differs from the expected expression"),
        )?;
    }
    Ok(())
}
