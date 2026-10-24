//! Internal tests. Most build an artifact, change one part of the source, the
//! PILout or the certificate, and require a check to reject the change for the
//! expected reason.
mod air;
mod packed;
mod parser;
mod placement;
mod properties;
mod reference;

use crate::{contract::AIR_NAME, proto as pb, ErrorKind, Result, P};
use prost::Message;

/// Byte offsets of the R1CS header counts in files built by [`r1cs_bytes`].
pub mod header {
    pub const WIRES: usize = 36;
    pub const OUTPUTS: usize = 40;
    pub const INPUTS: usize = 44;
    pub const PRIVATE: usize = 48;
    pub const LABELS: usize = 52;
}

/// An R1CS file with header counts `[wires, outputs, public inputs, private
/// inputs]`, one label per wire, and for each `[a, b, c]` in `equations` the
/// equation `w_a · w_b = w_c` with coefficients 1.
pub fn r1cs_bytes(header: [u32; 4], equations: &[[u32; 3]]) -> Vec<u8> {
    let equations: Vec<_> = equations
        .iter()
        .map(|wires| wires.map(|wire| vec![(wire, 1)]))
        .collect();
    r1cs_with_terms(header, &equations)
}

/// Like [`r1cs_bytes`], with each factor given as `(wire, coefficient)` terms,
/// written in the order given.
pub fn r1cs_with_terms(header: [u32; 4], equations: &[[Vec<(u32, u64)>; 3]]) -> Vec<u8> {
    let mut h = vec![];
    h.extend(8u32.to_le_bytes());
    h.extend(P.to_le_bytes());
    for v in header {
        h.extend(v.to_le_bytes());
    }
    h.extend(u64::from(header[0]).to_le_bytes());
    h.extend((equations.len() as u32).to_le_bytes());
    let mut eq = vec![];
    for factors in equations {
        for terms in factors {
            eq.extend((terms.len() as u32).to_le_bytes());
            for (wire, coefficient) in terms {
                eq.extend(wire.to_le_bytes());
                eq.extend(coefficient.to_le_bytes());
            }
        }
    }
    sections(b"r1cs", 1, [(1, h), (2, eq)])
}

/// A WTNS file with the given wire values.
pub fn wtns_bytes(values: &[u64]) -> Vec<u8> {
    let mut header = 8u32.to_le_bytes().to_vec();
    header.extend(P.to_le_bytes());
    header.extend((values.len() as u32).to_le_bytes());
    let data = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    sections(b"wtns", 2, [(1, header), (2, data)])
}

/// A binary file: magic, version, section count, then `(tag, length, data)`.
fn sections<const N: usize>(magic: &[u8; 4], version: u32, parts: [(u32, Vec<u8>); N]) -> Vec<u8> {
    let mut bytes = magic.to_vec();
    bytes.extend(version.to_le_bytes());
    bytes.extend((N as u32).to_le_bytes());
    for (tag, data) in parts {
        bytes.extend(tag.to_le_bytes());
        bytes.extend((data.len() as u64).to_le_bytes());
        bytes.extend(data);
    }
    bytes
}

/// Overwrite the little-endian `u32` at `offset`.
pub fn set_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

/// Overwrite the little-endian `u64` at `offset`.
pub fn set_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

/// Require `result` to be an invalid-input error whose message chain contains
/// `expected`.
#[track_caller]
pub fn assert_rejected<T>(result: Result<T>, expected: &str) {
    assert_error(result, ErrorKind::Invalid, expected);
}

/// Require a size limit error whose message chain contains `expected`.
pub fn assert_limit<T>(result: Result<T>, expected: &str) {
    assert_error(result, ErrorKind::Limit, expected);
}

fn assert_error<T>(result: Result<T>, kind: ErrorKind, expected: &str) {
    match result {
        Ok(_) => panic!("accepted; expected an error containing {expected:?}"),
        Err(error) => {
            let message = format!("{error:#}");
            assert!(
                message.contains(expected),
                "error {message:?} does not contain {expected:?}"
            );
            assert_eq!(error.kind(), kind, "{message}");
        }
    }
}

/// Decode `pilout`, apply `change` and encode the result again.
pub fn changed_pilout(pilout: &[u8], change: impl FnOnce(&mut pb::PilOut)) -> Vec<u8> {
    let mut decoded = pb::PilOut::decode(pilout).unwrap();
    change(&mut decoded);
    decoded.encode_to_vec()
}

/// A change to a decoded PILout.
pub type Change = Box<dyn Fn(&mut pb::PilOut)>;

/// The single AIR of a PILout.
pub fn air(pilout: &mut pb::PilOut) -> &mut pb::Air {
    &mut pilout.air_groups[0].airs[0]
}

/// The fixed column named `name` (without the `Kernel.` prefix).
pub fn fixed_column<'a>(pilout: &'a mut pb::PilOut, name: &str) -> &'a mut pb::FixedCol {
    let symbol = format!("{AIR_NAME}.{name}");
    let id = pilout
        .symbols
        .iter()
        .find(|s| s.name == symbol)
        .unwrap_or_else(|| panic!("no fixed column {name}"))
        .id;
    &mut air(pilout).fixed_cols[id as usize]
}

/// Index of the first constraint labeled `label`.
pub fn constraint_labeled(pilout: &pb::PilOut, label: &str) -> usize {
    pilout.air_groups[0].airs[0]
        .constraints
        .iter()
        .position(|c| match &c.constraint {
            Some(pb::constraint::Constraint::EveryRow(row)) => {
                row.debug_line.as_deref() == Some(label)
            }
            _ => false,
        })
        .unwrap_or_else(|| panic!("no constraint labeled {label}"))
}

/// Point constraint `target` at the expression of constraint `source`,
/// keeping `target`'s label.
pub fn reuse_expression(pilout: &mut pb::PilOut, target: usize, source: usize) {
    let expression = |c: &pb::Constraint| match &c.constraint {
        Some(pb::constraint::Constraint::EveryRow(row)) => row.expression_idx,
        _ => panic!("not an every-row constraint"),
    };
    let constraints = &mut air(pilout).constraints;
    let index = expression(&constraints[source]);
    if let Some(pb::constraint::Constraint::EveryRow(row)) = &mut constraints[target].constraint {
        row.expression_idx = index;
    }
}

/// The named fields of the hint called `name`.
pub fn hint_fields<'a>(pilout: &'a mut pb::PilOut, name: &str) -> &'a mut Vec<pb::HintField> {
    let hint = pilout
        .hints
        .iter_mut()
        .find(|h| h.name == name)
        .unwrap_or_else(|| panic!("no hint {name}"));
    match &mut hint.hint_fields[0].value {
        Some(pb::hint_field::Value::HintFieldArray(array)) => &mut array.hint_fields,
        _ => panic!("hint {name} has no field array"),
    }
}

/// An operand wrapping `value`.
pub fn operand(value: pb::operand::Operand) -> pb::Operand {
    pb::Operand {
        operand: Some(value),
    }
}

/// Append the unreferenced expression `bad + 0`, so that only operand
/// validation can reject it; the relation itself is unchanged.
pub fn with_unreferenced_operand(bad: pb::operand::Operand) -> impl FnOnce(&mut pb::PilOut) {
    move |pilout| {
        let zero = operand(pb::operand::Operand::Constant(pb::operand::Constant {
            value: vec![],
        }));
        air(pilout).expressions.push(pb::Expression {
            operation: Some(pb::expression::Operation::Add(pb::expression::Add {
                lhs: Some(operand(bad)),
                rhs: Some(zero),
            })),
        });
    }
}

/// A witness-column operand.
pub fn witness_column(stage: u32, col_idx: u32, row_offset: i32) -> pb::operand::Operand {
    pb::operand::Operand::WitnessCol(pb::operand::WitnessCol {
        stage,
        col_idx,
        row_offset,
    })
}

/// A fixed-column operand.
pub fn fixed_operand(idx: u32, row_offset: i32) -> pb::operand::Operand {
    pb::operand::Operand::FixedCol(pb::operand::FixedCol { idx, row_offset })
}

/// A challenge operand.
pub fn challenge_operand(stage: u32, idx: u32) -> pb::operand::Operand {
    pb::operand::Operand::Challenge(pb::operand::Challenge { stage, idx })
}

/// The checkers must not depend on the code that builds artifacts: no
/// compiler module may appear in a `crate::` path, and no type that the crate
/// root re-exports from a compiler module may be imported.
#[test]
fn checkers_do_not_use_compiler_modules() {
    const CHECKERS: &[(&str, &str)] = &[
        ("checker.rs", include_str!("../checker.rs")),
        ("packed_check.rs", include_str!("../packed_check.rs")),
        ("placement_check.rs", include_str!("../placement_check.rs")),
        ("pilout_check.rs", include_str!("../pilout_check.rs")),
        ("polynomial.rs", include_str!("../polynomial.rs")),
        ("witness.rs", include_str!("../witness.rs")),
    ];
    const COMPILER_MODULES: &[&str] = &["compiler", "dag", "emit", "layout", "network", "packed"];
    const COMPILER_TYPES: &[&str] = &[
        "PackedArtifact",
        "LayoutConfig",
        "Strategy",
        "ConversionOptions",
        "Reuse",
    ];
    let words = |text: &str| -> Vec<String> {
        text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .filter(|word| !word.is_empty())
            .map(String::from)
            .collect()
    };
    for (file, text) in CHECKERS {
        let code: String = text
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut imported = vec![];
        for statement in code.split(';') {
            if let Some(start) = statement.find("use crate::") {
                imported.extend(words(&statement[start..]));
            }
        }
        for module in COMPILER_MODULES {
            assert!(
                !imported.iter().any(|word| word == module),
                "{file} imports the compiler module `{module}`"
            );
            assert!(
                !code.contains(&format!("crate::{module}::")),
                "{file} refers to the compiler module `{module}`"
            );
        }
        for name in COMPILER_TYPES {
            assert!(
                !imported.iter().any(|word| word == name),
                "{file} imports `{name}`, which belongs to the compiler"
            );
        }
    }
}

/// The public error type can be sent between threads.
#[test]
fn errors_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<crate::Error>();
}
