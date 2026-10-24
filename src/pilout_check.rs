//! Checks shared by the reference and packed checkers: decoding, the parts of
//! the PILout that both artifact kinds have in common, the symbol table and the
//! layout of hints. Used only by the checkers.
use crate::{
    contract::{
        AIR_NAME, CERTIFICATE_VERSION, MAX_EXPRESSIONS, MAX_PILOUT_BYTES, PILOUT_NAME, STAGE_TWO,
    },
    ensure, ensure_within_limit, proto as pb, sha256_hex, Result, P,
};
use pb::SymbolType;
use prost::Message;
use std::collections::BTreeMap;

/// Fields that bind a certificate to its relation and to the files it certifies.
pub struct Binding<'a> {
    pub version: u32,
    pub relation: &'a str,
    pub source_sha256: &'a str,
    pub pilout_sha256: &'a str,
}

/// Require the certificate format version, the relation `expected_relation`,
/// and the SHA-256 hashes of `source` and `pilout`.
pub fn check_binding(
    binding: &Binding,
    expected_relation: &str,
    source: &[u8],
    pilout: &[u8],
) -> Result<()> {
    ensure(
        binding.version == CERTIFICATE_VERSION,
        format!("unsupported certificate version {}", binding.version),
    )?;
    ensure(
        binding.relation == expected_relation,
        format!(
            "certificate relation is '{}', expected '{expected_relation}'",
            binding.relation
        ),
    )?;
    ensure(
        binding.source_sha256 == sha256_hex(source),
        "certificate sourceSha256 does not match the source",
    )?;
    ensure(
        binding.pilout_sha256 == sha256_hex(pilout),
        "certificate piloutSha256 does not match the PILout",
    )
}

/// Decode PILout bytes, which must be the canonical encoding of the message.
pub fn decode(bytes: &[u8]) -> Result<pb::PilOut> {
    ensure_within_limit(
        bytes.len() <= MAX_PILOUT_BYTES,
        "PILout exceeds the size limit",
    )?;
    let pilout =
        pb::PilOut::decode(bytes).map_err(|e| format!("PILout is not valid protobuf: {e}"))?;
    ensure(
        pilout.encode_to_vec() == bytes,
        "PILout is not in canonical protobuf encoding",
    )?;
    Ok(pilout)
}

/// Decode a field element from big-endian bytes without leading zeros; the
/// empty array is zero.
pub fn scalar(bytes: &[u8]) -> Result<u64> {
    ensure(bytes.len() <= 8, "field element longer than 8 bytes")?;
    ensure(
        bytes.first() != Some(&0),
        "field element bytes must have no leading zeros",
    )?;
    let mut value = 0u64;
    for byte in bytes {
        value = (value << 8) | u64::from(*byte);
    }
    ensure(value < P, "field element not below P")?;
    Ok(value)
}

/// Expected global counts and AIR geometry.
pub struct Geometry {
    pub rows: u32,
    pub stage_widths: [u32; 2],
    pub publics: u32,
}

/// Field, names, global counts and AIR geometry, and the absence of any
/// unsupported content. Returns the single AIR.
pub fn check_envelope<'a>(pilout: &'a pb::PilOut, expected: &Geometry) -> Result<&'a pb::Air> {
    ensure(
        pilout.name.as_deref() == Some(PILOUT_NAME),
        format!("PILout must be named '{PILOUT_NAME}'"),
    )?;
    ensure(
        pilout.base_field == P.to_be_bytes(),
        "PILout field must be Goldilocks",
    )?;
    ensure(
        pilout.num_challenges == [0, 2],
        "PILout must declare two stage-two challenges and no others",
    )?;
    ensure(
        pilout.num_public_values == expected.publics,
        "PILout public value count differs from the source",
    )?;
    ensure(
        pilout.num_proof_values.is_empty()
            && pilout.public_tables.is_empty()
            && pilout.expressions.is_empty()
            && pilout.constraints.is_empty(),
        "unsupported proof values, public tables, global expressions or global constraints",
    )?;
    ensure(
        pilout.air_groups.len() == 1,
        "exactly one AIR group required",
    )?;
    let group = &pilout.air_groups[0];
    ensure(
        group.name.as_deref() == Some(AIR_NAME),
        format!("AIR group must be named '{AIR_NAME}'"),
    )?;
    ensure(
        group.air_group_values.is_empty(),
        "unsupported AIR group values",
    )?;
    ensure(group.airs.len() == 1, "exactly one AIR required")?;
    let air = &group.airs[0];
    ensure(
        air.name.as_deref() == Some(AIR_NAME),
        format!("AIR must be named '{AIR_NAME}'"),
    )?;
    ensure(
        air.num_rows == Some(expected.rows),
        "AIR row count differs from the certificate",
    )?;
    ensure(
        air.stage_widths == expected.stage_widths,
        "AIR stage widths differ from the expected widths",
    )?;
    ensure(air.aggregable, "AIR must be aggregable")?;
    ensure(
        air.periodic_cols.is_empty() && air.air_values.is_empty() && air.custom_commits.is_empty(),
        "unsupported periodic columns, AIR values or custom commits",
    )?;
    ensure_within_limit(
        air.expressions.len() <= MAX_EXPRESSIONS,
        "too many expressions",
    )?;
    Ok(air)
}

/// Expected fields of one symbol.
#[derive(PartialEq, Eq, Debug)]
pub struct ExpectedSymbol {
    kind: SymbolType,
    id: u32,
    stage: Option<u32>,
    /// Whether the symbol belongs to the single AIR (AIR group 0, AIR 0).
    in_air: bool,
    lengths: Vec<u32>,
}
impl ExpectedSymbol {
    /// A symbol of the single AIR.
    pub fn air(kind: SymbolType, id: u32, stage: u32, lengths: Vec<u32>) -> Self {
        Self {
            kind,
            id,
            stage: Some(stage),
            in_air: true,
            lengths,
        }
    }
    /// A challenge, which belongs to no AIR.
    pub fn challenge(index: u32, stage: u32) -> Self {
        Self {
            kind: SymbolType::Challenge,
            id: index,
            stage: Some(stage),
            in_air: false,
            lengths: vec![],
        }
    }
    /// The array of all public values.
    pub fn publics(count: u32) -> Self {
        Self {
            kind: SymbolType::PublicValue,
            id: 0,
            stage: None,
            in_air: false,
            lengths: vec![count],
        }
    }
}

/// Require the symbol table to hold exactly `expected`, by name, with every
/// field equal.
pub fn check_symbols(
    pilout: &pb::PilOut,
    mut expected: BTreeMap<String, ExpectedSymbol>,
) -> Result<()> {
    ensure(pilout.symbols.len() == expected.len(), "symbol count")?;
    for symbol in &pilout.symbols {
        let wanted = expected
            .remove(&symbol.name)
            .ok_or_else(|| format!("unexpected or duplicate symbol '{}'", symbol.name))?;
        let air = if wanted.in_air { Some(0) } else { None };
        ensure(
            symbol.r#type == wanted.kind as i32
                && symbol.id == wanted.id
                && symbol.stage == wanted.stage
                && symbol.air_group_id == air
                && symbol.air_id == air
                && symbol.lengths == wanted.lengths
                && symbol.dim == symbol.lengths.len() as u32
                && symbol.commit_id.is_none()
                && symbol.debug_line.is_none(),
            format!("symbol '{}' has unexpected fields", symbol.name),
        )?;
    }
    Ok(())
}

/// Root expression index of every constraint. Every constraint must apply to
/// every row and carry the expected label, in order.
pub fn constraint_roots(air: &pb::Air, labels: &[String]) -> Result<Vec<usize>> {
    use pb::constraint::Constraint;
    ensure(air.constraints.len() == labels.len(), "constraint count")?;
    let mut roots = vec![];
    for (index, (constraint, label)) in air.constraints.iter().zip(labels).enumerate() {
        let row = match constraint.constraint.as_ref() {
            Some(Constraint::EveryRow(row)) => row,
            Some(Constraint::FirstRow(_) | Constraint::LastRow(_) | Constraint::EveryFrame(_))
            | None => return Err(format!("constraint {index} must apply to every row").into()),
        };
        ensure(
            row.debug_line.as_deref() == Some(label.as_str()),
            format!("constraint {index} must be labeled '{label}'"),
        )?;
        let root = row
            .expression_idx
            .as_ref()
            .ok_or_else(|| format!("constraint {index} has no expression"))?;
        roots.push(root.idx as usize);
    }
    Ok(roots)
}

/// Named operand fields of one hint.
pub type HintFields<'a> = BTreeMap<&'a str, &'a pb::Operand>;

/// Require the hints to have exactly the names `names`, in order, each in the
/// single AIR with one array of uniquely named operand fields, and return the
/// fields of each hint.
pub fn hint_fields<'a>(pilout: &'a pb::PilOut, names: &[&str]) -> Result<Vec<HintFields<'a>>> {
    ensure(pilout.hints.len() == names.len(), "hint count")?;
    let mut result = vec![];
    for (hint, name) in pilout.hints.iter().zip(names) {
        ensure(
            hint.name == *name
                && hint.air_group_id == Some(0)
                && hint.air_id == Some(0)
                && hint.hint_fields.len() == 1
                && hint.hint_fields[0].name.is_none(),
            format!("hint '{name}': name, AIR or field layout"),
        )?;
        let Some(pb::hint_field::Value::HintFieldArray(array)) = &hint.hint_fields[0].value else {
            return Err(format!("hint '{name}': fields must be an array").into());
        };
        let mut fields = BTreeMap::new();
        for field in &array.hint_fields {
            let field_name = field
                .name
                .as_deref()
                .ok_or_else(|| format!("hint '{name}': unnamed field"))?;
            let Some(pb::hint_field::Value::Operand(operand)) = &field.value else {
                return Err(
                    format!("hint '{name}': field '{field_name}' is not an operand").into(),
                );
            };
            ensure(
                fields.insert(field_name, operand).is_none(),
                format!("hint '{name}': duplicate field '{field_name}'"),
            )?;
        }
        result.push(fields);
    }
    Ok(result)
}

/// Require `actual` to have exactly the field names of `expected`.
pub fn check_field_names<T>(
    name: &str,
    actual: &HintFields,
    expected: &BTreeMap<&str, T>,
) -> Result<()> {
    ensure(
        actual.keys().eq(expected.keys()),
        format!("hint '{name}': field names"),
    )
}

/// A binary operation of an AIR expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
}

/// The operation and both operands of an expression. Negation, the only other
/// operation in the schema, is never generated and is rejected.
pub fn binary_operation(
    expression: &pb::Expression,
) -> Result<(BinaryOp, &pb::Operand, &pb::Operand)> {
    use pb::expression::Operation;
    let (op, lhs, rhs) = match expression.operation.as_ref().ok_or("empty expression")? {
        Operation::Add(x) => (BinaryOp::Add, &x.lhs, &x.rhs),
        Operation::Sub(x) => (BinaryOp::Sub, &x.lhs, &x.rhs),
        Operation::Mul(x) => (BinaryOp::Mul, &x.lhs, &x.rhs),
        Operation::Neg(_) => return Err("negation is not supported".into()),
    };
    Ok((
        op,
        lhs.as_ref().ok_or("expression without lhs")?,
        rhs.as_ref().ok_or("expression without rhs")?,
    ))
}

/// The expected structure of an operand, compared operation by operation.
pub enum Pattern {
    Constant(u64),
    /// A witness column at the current row: stage and column.
    Witness(u32, u32),
    /// A stage-two challenge, by index.
    Challenge(u32),
    Op(BinaryOp, Box<Pattern>, Box<Pattern>),
}
impl Pattern {
    /// `lhs op rhs`.
    pub fn op(op: BinaryOp, lhs: Pattern, rhs: Pattern) -> Self {
        Pattern::Op(op, Box::new(lhs), Box::new(rhs))
    }
}

/// Whether `operand` has exactly the structure `pattern`, following
/// expression references in `air`.
pub fn matches(air: &pb::Air, operand: &pb::Operand, pattern: &Pattern) -> Result<bool> {
    use pb::operand::Operand;
    let Some(operand) = operand.operand.as_ref() else {
        return Ok(false);
    };
    Ok(match (operand, pattern) {
        (Operand::Constant(x), Pattern::Constant(value)) => scalar(&x.value)? == *value,
        (Operand::WitnessCol(x), Pattern::Witness(stage, column)) => {
            x.stage == *stage && x.col_idx == *column && x.row_offset == 0
        }
        (Operand::Challenge(x), Pattern::Challenge(index)) => {
            x.stage == STAGE_TWO && x.idx == *index
        }
        (Operand::Expression(x), Pattern::Op(op, lhs, rhs)) => {
            let expression = air
                .expressions
                .get(x.idx as usize)
                .ok_or("expression index out of range")?;
            let (actual, actual_lhs, actual_rhs) = binary_operation(expression)?;
            actual == *op && matches(air, actual_lhs, lhs)? && matches(air, actual_rhs, rhs)?
        }
        _ => false,
    })
}
