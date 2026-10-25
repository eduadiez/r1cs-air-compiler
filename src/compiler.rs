//! Compiles a circuit to the reference PILout: one stage-one column per
//! nonconstant wire, every source equation constrained on every row, and each
//! public value bound to its wire at row 0.
use crate::{
    contract::{
        label, CERTIFICATE_VERSION, FIRST_COLUMN, MAX_ROWS, MIN_ROWS, REFERENCE_RELATION,
        ZERO_COLUMN_NAME,
    },
    dag::{operand, trace, Op},
    emit::Builder,
    ensure, proto as pb,
    r1cs::Circuit,
    sha256_hex, ReferenceCertificate, Result,
};
use pb::operand::Operand;

/// Reference PILout and certificate. With `optimize`, expressions are
/// simplified and repeated subexpressions reused.
pub fn compile(
    source: &[u8],
    circuit: &Circuit,
    rows: usize,
    optimize: bool,
) -> Result<(Vec<u8>, ReferenceCertificate)> {
    ensure(
        rows.is_power_of_two() && (MIN_ROWS..=MAX_ROWS).contains(&rows),
        format_args!("rows must be a power of two in {MIN_ROWS}..={MAX_ROWS}"),
    )?;
    // At most MAX_ROWS, so it fits.
    let rows = rows as u32;
    ensure(circuit.wires > 1, "circuit has no nonconstant wires")?;
    let width = circuit.wires - 1;
    let mut builder = Builder::new(rows, width, ZERO_COLUMN_NAME, optimize);
    builder.constrain_zero_column();
    for (index, [a, b, c]) in circuit.equations.iter().enumerate() {
        let a = builder.linear_combination(a);
        let b = builder.linear_combination(b);
        let c = builder.linear_combination(c);
        let product = builder.expression(Op::Mul, a, b);
        let residual = builder.expression(Op::Sub, product, c);
        builder.constrain(residual, label::source_equation(index));
    }
    if circuit.publics > 0 {
        let first = builder.fixed(FIRST_COLUMN, (0..rows).map(|r| u64::from(r == 0)).collect());
        for index in 0..circuit.publics {
            let public = operand(Operand::PublicValue(pb::operand::PublicValue {
                idx: index,
            }));
            let difference = builder.expression(Op::Sub, trace(index), public);
            let residual = builder.expression(Op::Mul, first.clone(), difference);
            builder.constrain(residual, label::public(index as usize));
        }
    }
    let bytes = builder.finish(circuit.publics);
    let certificate = ReferenceCertificate {
        version: CERTIFICATE_VERSION,
        relation: REFERENCE_RELATION.into(),
        source_sha256: sha256_hex(source),
        pilout_sha256: sha256_hex(&bytes),
        rows,
        width,
        public_wires: (1..=circuit.publics).collect(),
        wire_columns: (0..circuit.wires).map(|w| w.checked_sub(1)).collect(),
    };
    Ok((bytes, certificate))
}
