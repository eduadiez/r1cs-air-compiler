//! Checkers for modules: a module fragment against its source, and the
//! embedding of a module in a host PILout. They read only the R1CS, the
//! PILout and the certificate or binding, and do not call the compiler. The
//! checks are tests run by a program, not a machine-checked proof.
use crate::{
    contract::{
        AIR_NAME, CERTIFICATE_VERSION, MAX_EVALUATED_TERMS, MAX_EXPRESSIONS, MAX_PILOUT_BYTES,
        MODULE_RELATION, PILOUT_NAME, STAGE_ONE, TRACE_SYMBOL,
    },
    ensure, ensure_within_limit,
    field::neg,
    pilout_check::{self, Binding, ExpectedSymbol},
    polynomial::{
        accumulate, constant, difference, product, sum, variable, Interpreter, Polynomial, Var,
    },
    proto as pb, r1cs, sha256_hex, CompositionBinding, ModuleCertificate, Result, P,
};
use pb::SymbolType;
use prost::Message;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// What a successful composition check establishes, as printed in its report.
pub const COMPOSITION_CLAIM: &str = "the selected AIR enforces every source equation on every \
     row over the declared stage-one columns at row offset 0; nothing is claimed about its \
     other constraints, the bus or the proof protocol";

/// Result of a successful module check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ModuleCheckReport {
    /// Always true: a failed check returns an error instead.
    pub accepted: bool,
    /// Name of the checked relation.
    pub relation: String,
    /// Number of equations in the source circuit.
    pub source_equations: usize,
    /// Number of stage-one columns of the fragment.
    pub width: usize,
    /// Number of exported wires.
    pub exports: usize,
    /// Always false: the check is not a formal proof.
    pub formally_verified: bool,
}

/// Result of a successful composition check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CompositionReport {
    /// Always true: a failed check returns an error instead.
    pub preserved: bool,
    /// Name of the module's relation.
    pub relation: String,
    /// What the check establishes: [`COMPOSITION_CLAIM`].
    pub claim: String,
    /// Index of the selected AIR group.
    pub air_group: u32,
    /// Index of the selected AIR in its group.
    pub air: u32,
    /// Name of the selected AIR, if it has one; for diagnostics only.
    pub air_name: Option<String>,
    /// Number of rows of the selected AIR.
    pub rows: u32,
    /// Number of equations in the source circuit.
    pub source_equations: usize,
    /// Equations whose `A · B - C` is the zero polynomial, which hold without
    /// any constraint.
    pub trivial_equations: usize,
    /// Each exported wire and its host column.
    pub exports: Vec<crate::WireColumn>,
    /// Always false: the check is not a formal proof.
    pub formally_verified: bool,
}

/// Check a module fragment against its source, in this order:
/// 1. certificate version, relation, hashes, wire map and exports;
/// 2. fragment field, names, geometry, symbols and the absence of anything a
///    module does not have (rows, stage two, fixed columns, public values,
///    challenges, hints);
/// 3. one every-row constraint per source equation, in order, each equal as a
///    polynomial to `A · B - C` under the wire map.
pub fn check_module(
    source: &[u8],
    fragment: &[u8],
    certificate: &ModuleCertificate,
) -> Result<ModuleCheckReport> {
    let binding = Binding {
        version: certificate.version,
        relation: &certificate.relation,
        source_sha256: &certificate.source_sha256,
        pilout_sha256: &certificate.pilout_sha256,
    };
    pilout_check::check_binding(&binding, MODULE_RELATION, source, fragment)?;
    let circuit = r1cs::read(source)?;
    check_wires(&circuit, certificate)?;
    let pilout = pilout_check::decode(fragment)?;
    let air = check_fragment_envelope(&pilout, certificate.width)?;
    pilout_check::check_symbols(
        &pilout,
        BTreeMap::from([(
            TRACE_SYMBOL.to_string(),
            ExpectedSymbol::air(
                SymbolType::WitnessCol,
                0,
                STAGE_ONE,
                vec![certificate.width],
            ),
        )]),
    )?;
    ensure(
        air.constraints.len() == circuit.equations.len(),
        "one constraint per source equation",
    )?;
    let mut interpreter = Interpreter::new(air, certificate.width, 0);
    interpreter.evaluate_until(air.expressions.len())?;
    for (index, (constraint, equation)) in
        air.constraints.iter().zip(&circuit.equations).enumerate()
    {
        let root = match &constraint.constraint {
            Some(pb::constraint::Constraint::EveryRow(row)) if row.debug_line.is_none() => {
                row.expression_idx.as_ref().map(|e| e.idx as usize)
            }
            _ => None,
        }
        .ok_or_else(|| format!("constraint {index} must apply to every row, without a label"))?;
        let expected = equation_polynomial(equation, |wire| {
            certificate.wire_columns[wire as usize].map(Var::trace)
        })?;
        ensure(
            interpreter.values.get(root) == Some(&expected),
            format_args!("constraint {index} differs from source equation {index}"),
        )?;
    }
    Ok(ModuleCheckReport {
        accepted: true,
        relation: certificate.relation.clone(),
        source_equations: circuit.equations.len(),
        width: certificate.width(),
        exports: certificate.exports.len(),
        formally_verified: false,
    })
}

/// Check that a host PILout preserves a module under `binding`:
/// 1. the certificate's version, relation, source hash, wire map and exports;
/// 2. the host field, and the selected AIR with a row count;
/// 3. the binding: every nonconstant wire gets exactly one stage-one column of
///    that AIR, and no two wires share a column;
/// 4. for every source equation whose `A · B - C` is not the zero polynomial,
///    some every-row constraint of the AIR equals it as a polynomial, with
///    each wire read from its host column at row offset 0.
///
/// The host may have any other columns, constraints, stages and hints;
/// expressions may be shared or simplified and need not be in order.
pub fn check_composition(
    source: &[u8],
    host: &[u8],
    certificate: &ModuleCertificate,
    binding: &CompositionBinding,
) -> Result<CompositionReport> {
    ensure(
        certificate.version == CERTIFICATE_VERSION,
        format_args!("unsupported certificate version {}", certificate.version),
    )?;
    ensure(
        certificate.relation == MODULE_RELATION,
        format_args!(
            "certificate relation is '{}', expected '{MODULE_RELATION}'",
            certificate.relation
        ),
    )?;
    ensure(
        certificate.source_sha256 == sha256_hex(source),
        "certificate sourceSha256 does not match the source",
    )?;
    let circuit = r1cs::read(source)?;
    check_wires(&circuit, certificate)?;

    ensure_within_limit(
        host.len() <= MAX_PILOUT_BYTES,
        "PILout exceeds the size limit",
    )?;
    let pilout =
        pb::PilOut::decode(host).map_err(|e| format!("PILout is not valid protobuf: {e}"))?;
    ensure(
        pilout.base_field == P.to_be_bytes(),
        "PILout field must be Goldilocks",
    )?;
    let air = pilout
        .air_groups
        .get(binding.air_group as usize)
        .and_then(|group| group.airs.get(binding.air as usize))
        .ok_or("the binding selects no AIR of the PILout")?;
    let rows = air.num_rows.ok_or("the selected AIR has no row count")?;
    let host_columns = host_columns(&circuit, air, binding)?;

    let mut evaluator = Evaluator::new(air);
    let mut enforced = BTreeSet::new();
    for constraint in &air.constraints {
        if let Some(pb::constraint::Constraint::EveryRow(row)) = &constraint.constraint {
            let root = row
                .expression_idx
                .as_ref()
                .ok_or("a constraint has no expression")?;
            if let Some(polynomial) = evaluator.evaluate(root.idx as usize)? {
                enforced.insert(polynomial);
            }
        }
    }
    let mut trivial_equations = 0;
    for (index, equation) in circuit.equations.iter().enumerate() {
        let expected =
            equation_polynomial(equation, |wire| host_columns[wire as usize].map(Var::trace))?;
        if expected.is_empty() {
            trivial_equations += 1;
            continue;
        }
        ensure(
            enforced.contains(&expected),
            format_args!(
                "source equation {index} is not enforced by any every-row constraint of the \
                 selected AIR"
            ),
        )?;
    }
    Ok(CompositionReport {
        preserved: true,
        relation: certificate.relation.clone(),
        claim: COMPOSITION_CLAIM.into(),
        air_group: binding.air_group,
        air: binding.air,
        air_name: air.name.clone(),
        rows,
        source_equations: circuit.equations.len(),
        trivial_equations,
        exports: certificate
            .exports
            .iter()
            .map(|wire| crate::WireColumn {
                wire: *wire,
                column: host_columns[*wire as usize].expect("every nonconstant wire is bound"),
            })
            .collect(),
        formally_verified: false,
    })
}

/// Width, a one-to-one map from nonconstant wires to fragment columns, and
/// distinct exported wires that are nonconstant wires of the circuit.
fn check_wires(circuit: &r1cs::Circuit, certificate: &ModuleCertificate) -> Result<()> {
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
    let mut seen = BTreeSet::new();
    for wire in &certificate.exports {
        ensure(
            (1..circuit.wires).contains(wire),
            format_args!("export {wire}: not a nonconstant wire of the circuit"),
        )?;
        ensure(
            seen.insert(wire),
            format_args!("export {wire}: listed twice"),
        )?;
    }
    Ok(())
}

/// Field, names, geometry and the absence of anything else. Returns the AIR.
fn check_fragment_envelope(pilout: &pb::PilOut, width: u32) -> Result<&pb::Air> {
    ensure(
        pilout.name.as_deref() == Some(PILOUT_NAME),
        format_args!("module must be named '{PILOUT_NAME}'"),
    )?;
    ensure(
        pilout.base_field == P.to_be_bytes(),
        "module field must be Goldilocks",
    )?;
    ensure(
        pilout.num_challenges.is_empty()
            && pilout.num_public_values == 0
            && pilout.num_proof_values.is_empty()
            && pilout.public_tables.is_empty()
            && pilout.expressions.is_empty()
            && pilout.constraints.is_empty()
            && pilout.hints.is_empty(),
        "a module has no challenges, public values, proof values, public tables, global \
         expressions, global constraints or hints",
    )?;
    let [group] = &pilout.air_groups[..] else {
        return Err("exactly one AIR group required".into());
    };
    ensure(
        group.name.as_deref() == Some(AIR_NAME) && group.air_group_values.is_empty(),
        format_args!("AIR group must be named '{AIR_NAME}' and have no values"),
    )?;
    let [air] = &group.airs[..] else {
        return Err("exactly one AIR required".into());
    };
    ensure(
        air.name.as_deref() == Some(AIR_NAME),
        format_args!("AIR must be named '{AIR_NAME}'"),
    )?;
    ensure(
        air.num_rows.is_none() && !air.aggregable,
        "a module has no row count and is not aggregable",
    )?;
    ensure(
        air.stage_widths == [width],
        "a module has only stage one, with one column per nonconstant wire",
    )?;
    ensure(
        air.fixed_cols.is_empty()
            && air.periodic_cols.is_empty()
            && air.air_values.is_empty()
            && air.custom_commits.is_empty(),
        "a module has no fixed or periodic columns, AIR values or custom commits",
    )?;
    ensure_within_limit(
        air.expressions.len() <= MAX_EXPRESSIONS,
        "too many expressions",
    )?;
    Ok(air)
}

/// The host column of each wire (`None` for wire 0), from a binding that maps
/// every nonconstant wire to a distinct stage-one column of `air`.
fn host_columns(
    circuit: &r1cs::Circuit,
    air: &pb::Air,
    binding: &CompositionBinding,
) -> Result<Vec<Option<u32>>> {
    let width = air.stage_widths.first().copied().unwrap_or(0);
    let mut columns = vec![None; circuit.wires as usize];
    let mut holders = BTreeMap::new();
    for entry in &binding.columns {
        ensure(
            (1..circuit.wires).contains(&entry.wire),
            format_args!("binding: wire {} is not a nonconstant wire", entry.wire),
        )?;
        ensure(
            columns[entry.wire as usize].is_none(),
            format_args!("binding: wire {} is listed twice", entry.wire),
        )?;
        ensure(
            entry.column < width,
            format_args!(
                "binding: column {} of wire {} is not a stage-one column of the AIR ({width})",
                entry.column, entry.wire
            ),
        )?;
        if let Some(other) = holders.insert(entry.column, entry.wire) {
            return Err(format!(
                "binding: wires {other} and {} share column {}",
                entry.wire, entry.column
            )
            .into());
        }
        columns[entry.wire as usize] = Some(entry.column);
    }
    if let Some(wire) = (1..circuit.wires).find(|w| columns[*w as usize].is_none()) {
        return Err(format!("binding: wire {wire} has no column").into());
    }
    Ok(columns)
}

/// `A · B - C` of a source equation, with wire 0 the constant 1 and each other
/// wire the variable `variable(wire)`.
fn equation_polynomial(
    equation: &r1cs::Equation,
    variable_of: impl Fn(u32) -> Option<Var>,
) -> Result<Polynomial> {
    let linear = |terms: &r1cs::LinearCombination| {
        let mut result = BTreeMap::new();
        for (wire, k) in terms {
            let key = variable_of(*wire).into_iter().collect();
            accumulate(&mut result, key, k.0);
        }
        result
    };
    let [a, b, c] = equation;
    Ok(difference(&product(&linear(a), &linear(b))?, &linear(c)))
}

/// Evaluates host expressions to polynomials on demand. A polynomial of degree
/// above 3 or beyond the product size limit is left unevaluated (`None`): its
/// constraint then matches no module equation, which can only make the check
/// fail, never pass.
struct Evaluator<'a> {
    air: &'a pb::Air,
    values: Vec<Option<Option<Polynomial>>>,
    terms: usize,
}
impl<'a> Evaluator<'a> {
    fn new(air: &'a pb::Air) -> Self {
        Self {
            air,
            values: vec![None; air.expressions.len()],
            terms: 0,
        }
    }

    /// The polynomial of expression `root`, evaluating the expressions it
    /// depends on first, without recursion.
    fn evaluate(&mut self, root: usize) -> Result<Option<Polynomial>> {
        ensure(
            root < self.air.expressions.len(),
            "expression index out of range",
        )?;
        let mut stack = vec![(root, false)];
        let mut open = BTreeSet::new();
        while let Some((index, ready)) = stack.pop() {
            if self.values[index].is_some() {
                continue;
            }
            let operands = self.operands(index)?;
            if ready {
                let value = self.combine(index, &operands)?;
                if let Some(polynomial) = &value {
                    self.terms = self.terms.saturating_add(polynomial.len());
                    ensure_within_limit(
                        self.terms <= MAX_EVALUATED_TERMS,
                        "evaluated expressions exceed the term limit",
                    )?;
                }
                self.values[index] = Some(value);
                open.remove(&index);
                continue;
            }
            ensure(
                open.insert(index),
                "expressions refer to each other in a cycle",
            )?;
            stack.push((index, true));
            for operand in operands {
                if let Some(pb::operand::Operand::Expression(e)) = &operand.operand {
                    let child = e.idx as usize;
                    ensure(
                        child < self.air.expressions.len(),
                        "expression index out of range",
                    )?;
                    ensure(
                        !open.contains(&child),
                        "expressions refer to each other in a cycle",
                    )?;
                    if self.values[child].is_none() {
                        stack.push((child, false));
                    }
                }
            }
        }
        Ok(self.values[root].clone().flatten())
    }

    fn operands(&self, index: usize) -> Result<Vec<&'a pb::Operand>> {
        use pb::expression::Operation;
        let missing = || format!("expression {index} has a missing operand");
        Ok(
            match self.air.expressions[index]
                .operation
                .as_ref()
                .ok_or_else(|| format!("expression {index} is empty"))?
            {
                Operation::Add(x) => vec![
                    x.lhs.as_ref().ok_or_else(missing)?,
                    x.rhs.as_ref().ok_or_else(missing)?,
                ],
                Operation::Sub(x) => vec![
                    x.lhs.as_ref().ok_or_else(missing)?,
                    x.rhs.as_ref().ok_or_else(missing)?,
                ],
                Operation::Mul(x) => vec![
                    x.lhs.as_ref().ok_or_else(missing)?,
                    x.rhs.as_ref().ok_or_else(missing)?,
                ],
                Operation::Neg(x) => vec![x.value.as_ref().ok_or_else(missing)?],
            },
        )
    }

    /// Apply expression `index`'s operation to its evaluated operands.
    fn combine(&self, index: usize, operands: &[&pb::Operand]) -> Result<Option<Polynomial>> {
        use pb::expression::Operation;
        let mut values = vec![];
        for operand in operands {
            match self.operand(operand)? {
                Some(value) => values.push(value),
                None => return Ok(None),
            }
        }
        Ok(
            match (&self.air.expressions[index].operation, &values[..]) {
                (Some(Operation::Add(_)), [a, b]) => Some(sum(a, b)),
                (Some(Operation::Sub(_)), [a, b]) => Some(difference(a, b)),
                (Some(Operation::Mul(_)), [a, b]) => product(a, b).ok(),
                (Some(Operation::Neg(_)), [a]) => {
                    Some(a.iter().map(|(k, v)| (k.clone(), neg(*v))).collect())
                }
                _ => unreachable!("operands() gives each operation its operand count"),
            },
        )
    }

    /// The polynomial of one operand. Stage-one columns at row offset 0 are the
    /// only variables a module equation uses; every other operand is a
    /// variable of its own, so it can never cancel one of them.
    fn operand(&self, operand: &pb::Operand) -> Result<Option<Polynomial>> {
        use pb::operand::Operand;
        let host = |kind: u8, ids: [u32; 3], offset: i32| {
            Ok(Some(variable(Var::Host { kind, ids, offset })))
        };
        match operand.operand.as_ref().ok_or("missing operand")? {
            Operand::Constant(x) => Ok(Some(constant(host_scalar(&x.value)?))),
            Operand::Expression(x) => Ok(self.values[x.idx as usize].clone().flatten()),
            Operand::WitnessCol(x) if x.row_offset == 0 => Ok(Some(variable(Var::Witness {
                stage: x.stage,
                column: x.col_idx,
            }))),
            Operand::WitnessCol(x) => host(0, [x.stage, x.col_idx, 0], x.row_offset),
            Operand::FixedCol(x) => host(1, [x.idx, 0, 0], x.row_offset),
            Operand::PeriodicCol(x) => host(2, [x.idx, 0, 0], x.row_offset),
            Operand::Challenge(x) => Ok(Some(variable(Var::Challenge {
                stage: x.stage,
                index: x.idx,
            }))),
            Operand::PublicValue(x) => Ok(Some(variable(Var::Public(x.idx)))),
            Operand::ProofValue(x) => host(3, [x.stage, x.idx, 0], 0),
            Operand::AirGroupValue(x) => host(4, [x.idx, 0, 0], 0),
            Operand::AirValue(x) => host(5, [x.idx, 0, 0], 0),
            Operand::CustomCol(x) => host(6, [x.commit_id, x.stage, x.col_idx], x.row_offset),
        }
    }
}

/// A field element in big-endian bytes, as any PILout writer may encode it:
/// leading zero bytes are allowed, the value must be below `P`.
fn host_scalar(bytes: &[u8]) -> Result<u64> {
    let start = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len());
    let bytes = &bytes[start..];
    ensure(bytes.len() <= 8, "field element longer than 8 bytes")?;
    let value = bytes.iter().fold(0u64, |v, b| (v << 8) | u64::from(*b));
    ensure(value < P, "field element not below P")?;
    Ok(value)
}
