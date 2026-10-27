//! Compiles a circuit to a module fragment: one stage-one column per
//! nonconstant wire and every source equation as its own constraint, for a
//! host AIR to embed. The fragment has no row count, public values, fixed
//! columns or stage two.
use crate::{
    compiler::equation_residuals,
    contract::{CERTIFICATE_VERSION, MODULE_RELATION},
    emit::{finish_module, Builder},
    ensure,
    r1cs::Circuit,
    sha256_hex, ModuleCertificate, Result,
};

/// Module fragment and certificate. Constraint `i` of the fragment is source
/// equation `i`. With `optimize`, expressions are simplified as in the
/// optimized reference.
pub fn compile(
    source: &[u8],
    circuit: &Circuit,
    exports: &[u32],
    optimize: bool,
) -> Result<(Vec<u8>, ModuleCertificate)> {
    ensure(circuit.wires > 1, "circuit has no nonconstant wires")?;
    for (index, wire) in exports.iter().enumerate() {
        ensure(
            (1..circuit.wires).contains(wire),
            format_args!("export {wire}: not a nonconstant wire of the circuit"),
        )?;
        ensure(
            !exports[..index].contains(wire),
            format_args!("export {wire}: listed twice"),
        )?;
    }
    let width = circuit.wires - 1;
    let mut builder = Builder::module(width, optimize);
    for residual in equation_residuals(&mut builder, circuit) {
        builder.constrain_labeled(residual, None);
    }
    let bytes = finish_module(builder);
    let certificate = ModuleCertificate {
        version: CERTIFICATE_VERSION,
        relation: MODULE_RELATION.into(),
        source_sha256: sha256_hex(source),
        pilout_sha256: sha256_hex(&bytes),
        width,
        wire_columns: (0..circuit.wires).map(|w| w.checked_sub(1)).collect(),
        exports: exports.to_vec(),
    };
    Ok((bytes, certificate))
}
