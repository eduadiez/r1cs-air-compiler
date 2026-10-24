//! Any bytes as the certificate JSON of the xor fixture's packed artifact: the
//! packed checker and witness projection must never panic.
#![no_main]
use libfuzzer_sys::fuzz_target;
use r1cs_air_compiler::PackedCertificate;

#[path = "common.rs"]
mod common;

fuzz_target!(|json: &[u8]| {
    let Ok(certificate) = serde_json::from_slice::<PackedCertificate>(json) else {
        return;
    };
    let pilout = &common::packed().pilout;
    let _ = r1cs_air_compiler::check_packed(common::SOURCE, pilout, &certificate);
    let _ =
        r1cs_air_compiler::project_witness(common::SOURCE, pilout, &certificate, common::WITNESS);
});
