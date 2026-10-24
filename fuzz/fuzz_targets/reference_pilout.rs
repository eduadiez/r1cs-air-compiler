//! Any bytes as the PILout of the xor fixture's reference artifact. The
//! certificate's PILout hash is updated, so the checker examines the content.
#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "common.rs"]
mod common;

fuzz_target!(|pilout: &[u8]| {
    let certificate = common::bound_to(&common::reference().certificate, pilout);
    let _ = r1cs_air_compiler::check_reference(common::SOURCE, pilout, &certificate);
});
