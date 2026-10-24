//! Any bytes as an R1CS file: parsing, compiling and checking must never panic.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|source: &[u8]| {
    let _ = r1cs_air_compiler::compile_reference(source, 64);
});
