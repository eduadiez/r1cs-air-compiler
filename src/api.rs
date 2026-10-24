//! Public operations. Each compile function builds an artifact, then checks it
//! with the independent checker before returning it.
use crate::{
    certificate, checker, compiler, packed, packed_check, r1cs, witness, Context, LayoutConfig,
    PackedCertificate, PackedCheckReport, ReferenceCertificate, ReferenceCheckReport, Result,
    Witness,
};
use serde::Serialize;

/// A reference PILout file and its certificate, both already checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferenceArtifact {
    /// Encoded PILout.
    pub pilout: Vec<u8>,
    /// Certificate binding the PILout to its source.
    pub certificate: ReferenceCertificate,
}

/// A packed PILout file, its certificate, and the wire held by each cell (row
/// by row, 0 for an unused cell), all already checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackedArtifact {
    /// Encoded PILout.
    pub pilout: Vec<u8>,
    /// Wire held by each cell, row by row; 0 marks an unused cell.
    pub map: Vec<u32>,
    /// Certificate binding the PILout to its source.
    pub certificate: PackedCertificate,
}

/// A checked packed layout without PILout. Serializes to JSON with its helper
/// network and placement; the cell map is available through [`LayoutPlan::map`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LayoutPlan {
    network: certificate::Network,
    layout: certificate::Layout,
    #[serde(skip)]
    map: Vec<u32>,
    #[serde(skip)]
    checked_equations: usize,
}
impl LayoutPlan {
    /// Number of trace rows (a power of two).
    pub fn rows(&self) -> usize {
        self.layout.rows
    }
    /// Stage-one column count.
    pub fn width(&self) -> usize {
        self.layout.width
    }
    /// Wire held by each cell, row by row; 0 marks an unused cell.
    pub fn map(&self) -> &[u32] {
        &self.map
    }
    /// Number of constrained helper wires.
    pub fn helper_count(&self) -> usize {
        self.network.helpers.len()
    }
    /// Whether some wire occupies more than one cell, so that a copy argument
    /// is needed to make those cells equal.
    pub fn requires_copy(&self) -> bool {
        self.layout.requires_copy
    }
    /// Number of placed equations: helper definitions plus rewritten source equations.
    pub fn checked_equations(&self) -> usize {
        self.checked_equations
    }
}

/// Compile `source` to a reference PILout with `rows` rows, where every row
/// must satisfy the whole circuit, and check the result.
///
/// # Errors
/// Fails on malformed or unsupported R1CS, on a row count that is not a power of
/// two in `64..=65536`, and on circuits beyond the checker's size limits. The
/// message names the failing stage.
pub fn compile_reference(source: &[u8], rows: usize) -> Result<ReferenceArtifact> {
    reference(source, rows, false)
}

/// Like [`compile_reference`], but simplifies expressions with identities that
/// hold for every field value and reuses repeated subexpressions.
///
/// # Errors
/// Same as [`compile_reference`].
pub fn compile_optimized_reference(source: &[u8], rows: usize) -> Result<ReferenceArtifact> {
    reference(source, rows, true)
}
fn reference(source: &[u8], rows: usize, optimize: bool) -> Result<ReferenceArtifact> {
    let circuit = r1cs::read(source).context("reference source")?;
    let (pilout, certificate) =
        compiler::compile(source, &circuit, rows, optimize).context("reference compilation")?;
    check_reference(source, &pilout, &certificate)?;
    Ok(ReferenceArtifact {
        pilout,
        certificate,
    })
}

/// Check that `pilout` encodes the reference relation of `source` as described
/// by `certificate`.
///
/// # Errors
/// Fails if a hash does not match, if the PILout contains anything the
/// specification does not allow, or if any constraint differs from the
/// expected one.
pub fn check_reference(
    source: &[u8],
    pilout: &[u8],
    certificate: &ReferenceCertificate,
) -> Result<ReferenceCheckReport> {
    checker::check(source, pilout, certificate).context("reference validation")
}

/// Build and check a packed layout without generating PILout.
///
/// # Errors
/// Fails on malformed R1CS, invalid settings, layouts beyond the size limits
/// and layouts that fail the placement check.
pub fn plan_layout(source: &[u8], config: &LayoutConfig) -> Result<LayoutPlan> {
    let circuit = r1cs::read(source).context("layout source")?;
    let (network, layout, checked) = packed::plan(&circuit, config)?;
    Ok(LayoutPlan {
        network,
        layout,
        map: checked.map,
        checked_equations: checked.equations.len(),
    })
}

/// Compile `source` to a packed PILout and check the result.
///
/// # Errors
/// Fails on malformed R1CS or invalid settings, and if the checker rejects any
/// helper, placement, public binding or copy constraint.
pub fn compile_packed(source: &[u8], config: &LayoutConfig) -> Result<PackedArtifact> {
    let artifact = packed::compile(source, config).context("packed compilation")?;
    check_packed(source, &artifact.pilout, &artifact.certificate)?;
    Ok(artifact)
}

/// Check that `pilout` encodes the packed relation of `source` as described by
/// `certificate`: helpers, placement, selectors, public bindings, the copy
/// argument and hints.
///
/// # Errors
/// Fails on an invalid certificate and on any difference between the PILout
/// and the expected constraints, fixed columns, symbols or hints.
pub fn check_packed(
    source: &[u8],
    pilout: &[u8],
    certificate: &PackedCertificate,
) -> Result<PackedCheckReport> {
    packed_check::check(source, pilout, certificate).context("packed validation")
}

/// Check a packed artifact, then turn a Circom witness (`.wtns`) into its
/// stage-one trace and public values.
///
/// # Errors
/// Fails if the artifact does not pass [`check_packed`], if the witness is
/// malformed, or if it does not satisfy every source and helper equation.
pub fn project_witness(
    source: &[u8],
    pilout: &[u8],
    certificate: &PackedCertificate,
    wtns: &[u8],
) -> Result<Witness> {
    check_packed(source, pilout, certificate)?;
    witness::project(source, certificate, wtns).context("witness projection")
}
