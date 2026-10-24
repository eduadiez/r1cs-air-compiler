//! Compiles the xor fixture to a reference artifact and prints its size.
use r1cs_air_compiler::{compile_reference, Result};

fn main() -> Result<()> {
    let source = include_bytes!("../fixtures/xor/kernel.r1cs");
    // Compilation includes independent validation before returning the artifact.
    let artifact = compile_reference(source, 64)?;
    println!(
        "{} rows, {} columns, {} PILout bytes",
        artifact.certificate.rows(),
        artifact.certificate.width(),
        artifact.pilout.len()
    );
    Ok(())
}
