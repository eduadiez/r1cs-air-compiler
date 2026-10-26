//! Compiles Goldilocks R1CS circuits to PILout and checks the result.
//!
//! Every compile function runs an independent checker on its own output before
//! returning it. The checkers can also be run on artifacts supplied from
//! elsewhere. `SPEC.md` describes the relations, output files and certificates.
//!
//! ```
//! use r1cs_air_compiler::{compile_reference, check_reference, Result};
//!
//! # fn main() -> Result<()> {
//! let source = include_bytes!("../fixtures/xor/kernel.r1cs");
//! let artifact = compile_reference(source, 64)?;
//! let report = check_reference(source, &artifact.pilout, &artifact.certificate)?;
//! assert!(report.accepted);
//! # Ok(())
//! # }
//! ```

mod api;
mod certificate;
mod checker;
mod compiler;
mod contract;
mod dag;
mod emit;
mod error;
mod field;
mod layout;
mod packed;
mod packed_check;
mod pilout_check;
mod placement_check;
mod polynomial;
mod r1cs;
mod rewrite;
mod witness;

// Generated from the vendored schema; its names and style are not ours.
#[allow(clippy::all, clippy::pedantic)]
mod proto {
    include!(concat!(env!("OUT_DIR"), "/pilout.rs"));
}

pub use api::{
    check_packed, check_reference, compile_optimized_reference, compile_packed, compile_reference,
    plan_layout, project_witness, LayoutPlan, PackedArtifact, ReferenceArtifact,
};
pub use certificate::{PackedCertificate, ReferenceCertificate};
pub use checker::ReferenceCheckReport;
pub use contract::{PACKED_RELATION, REFERENCE_RELATION};
pub use error::{Error, ErrorKind, Result};
pub use field::GOLDILOCKS_PRIME;
pub use layout::{LayoutConfig, Strategy};
pub use packed_check::PackedCheckReport;
pub use rewrite::{Reuse, RewriteOptions};
pub use witness::Witness;

pub(crate) use certificate::sha256_hex;
pub(crate) use error::{ensure, ensure_within_limit, Context};
pub(crate) use field::P;

#[cfg(test)]
mod tests;
