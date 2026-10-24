//! Turns a Circom witness into the stage-one trace of a packed artifact:
//! computes helper values, checks every equation, then copies each wire's value
//! into its cells.
use crate::{
    certificate::PackedCertificate,
    ensure,
    field::{add, mul},
    placement_check,
    r1cs::{self, Equation, LinearCombination},
    Result,
};

/// Values to load into a packed artifact's trace.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Witness {
    /// Stage-one trace, row by row; unused cells are 0.
    pub trace: Vec<u64>,
    /// Public values in order.
    pub publics: Vec<u64>,
}

/// Trace values row by row (0 in unused cells) and public values. The caller
/// must have checked the artifact first.
pub fn project(source: &[u8], certificate: &PackedCertificate, wtns: &[u8]) -> Result<Witness> {
    let circuit = r1cs::read(source)?;
    let checked = placement_check::check(&circuit, &certificate.network, &certificate.layout)?;
    let mut values = r1cs::read_witness(wtns, circuit.wires)?;
    for helper in &certificate.network.helpers {
        ensure(
            helper.id as usize == values.len(),
            "helper ids must follow the source wires in order",
        )?;
        values.push(evaluate(&helper.terms, &values));
    }
    for (index, equation) in checked.equations.iter().enumerate() {
        ensure(
            satisfied(equation, &values),
            format!("witness does not satisfy equation {index} (helper equations come first)"),
        )?;
    }
    let trace = checked
        .map
        .iter()
        .map(|w| if *w == 0 { 0 } else { values[*w as usize] })
        .collect();
    Ok(Witness {
        trace,
        publics: values[1..=circuit.publics as usize].to_vec(),
    })
}
/// Value of `lc` for the wire values `values`.
fn evaluate(lc: &LinearCombination, values: &[u64]) -> u64 {
    lc.iter()
        .fold(0, |sum, (w, k)| add(sum, mul(values[*w as usize], k.0)))
}
/// Whether `A · B = C` holds for the wire values `values`.
fn satisfied([a, b, c]: &Equation, values: &[u64]) -> bool {
    mul(evaluate(a, values), evaluate(b, values)) == evaluate(c, values)
}
