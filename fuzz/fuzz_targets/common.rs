//! The xor fixture's artifacts, compiled once, and a helper that rebinds a
//! certificate to fuzzed PILout bytes.
// Each target uses only some of these.
#![allow(dead_code)]
use r1cs_air_compiler::{LayoutConfig, PackedArtifact, ReferenceArtifact};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

pub const SOURCE: &[u8] = include_bytes!("../../fixtures/xor/kernel.r1cs");
pub const WITNESS: &[u8] = include_bytes!("../../fixtures/xor/input.wtns");
const LAYOUT: &[u8] = include_bytes!("../../fixtures/xor/layout.json");

pub fn reference() -> &'static ReferenceArtifact {
    static ARTIFACT: OnceLock<ReferenceArtifact> = OnceLock::new();
    ARTIFACT.get_or_init(|| r1cs_air_compiler::compile_reference(SOURCE, 64).unwrap())
}

pub fn packed() -> &'static PackedArtifact {
    static ARTIFACT: OnceLock<PackedArtifact> = OnceLock::new();
    ARTIFACT.get_or_init(|| {
        let config: LayoutConfig = serde_json::from_slice(LAYOUT).unwrap();
        r1cs_air_compiler::compile_packed(SOURCE, &config).unwrap()
    })
}

/// `certificate` with its PILout hash replaced by the hash of `pilout`.
pub fn bound_to<T: Serialize + DeserializeOwned>(certificate: &T, pilout: &[u8]) -> T {
    let mut value = serde_json::to_value(certificate).unwrap();
    value["piloutSha256"] = Value::String(format!("{:x}", Sha256::digest(pilout)));
    serde_json::from_value(value).unwrap()
}
