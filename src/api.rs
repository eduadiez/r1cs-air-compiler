//! Public operations. Each compile function builds an artifact, then checks it
//! with the independent checker before returning it.
use crate::{
    certificate, checker, compiler, module, module_check, packed, packed_check, r1cs, witness,
    CompositionBinding, CompositionReport, Context, LayoutConfig, ModuleCertificate,
    ModuleCheckReport, PackedCertificate, PackedCheckReport, ReferenceCertificate,
    ReferenceCheckReport, Result, Witness,
};
use serde::Serialize;

/// A reference PILout file and its certificate, both already checked.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReferenceArtifact {
    /// Encoded PILout.
    pub pilout: Vec<u8>,
    /// Certificate binding the PILout to its source.
    pub certificate: ReferenceCertificate,
}

/// A module fragment and its certificate, both already checked.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ModuleArtifact {
    /// Encoded module fragment: a PILout message with one AIR, not provable on
    /// its own. Constraint `i` is source equation `i`.
    pub pilout: Vec<u8>,
    /// Certificate binding the fragment to its source.
    pub certificate: ModuleCertificate,
}

/// A packed PILout file, its certificate, and the wire held by each cell (row
/// by row, 0 for an unused cell), all already checked.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PackedArtifact {
    /// Encoded PILout.
    pub pilout: Vec<u8>,
    /// Wire held by each cell, row by row; 0 marks an unused cell.
    pub map: Vec<u32>,
    /// Certificate binding the PILout to its source.
    pub certificate: PackedCertificate,
    /// The layout settings used, every field filled in.
    pub config: LayoutConfig,
}

/// A checked packed layout without PILout. Serializes to JSON as the
/// `rewrittenCircuit` and `layout` fields a packed certificate would have; the
/// cell map is available through [`LayoutPlan::map`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutPlan {
    rewritten_circuit: certificate::RewrittenCircuit,
    layout: certificate::Layout,
    #[serde(skip)]
    map: Vec<u32>,
    #[serde(skip)]
    checked_equations: usize,
    #[serde(skip)]
    config: LayoutConfig,
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
    /// Number of constrained auxiliary wires.
    pub fn auxiliary_count(&self) -> usize {
        self.rewritten_circuit.auxiliaries.len()
    }
    /// Whether some wire occupies more than one cell, so that a permutation argument
    /// is needed to make those cells equal.
    pub fn requires_permutation(&self) -> bool {
        self.layout.requires_permutation
    }
    /// Number of placed equations: auxiliary definitions plus rewritten source equations.
    pub fn checked_equations(&self) -> usize {
        self.checked_equations
    }
    /// The layout settings used, every field filled in.
    pub fn config(&self) -> &LayoutConfig {
        &self.config
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

/// Build and check a packed layout without generating PILout. Settings that
/// `config` leaves out are filled in as by [`compile_packed`].
///
/// # Errors
/// Fails on malformed R1CS, invalid settings, layouts beyond the size limits
/// and layouts that fail the placement check.
pub fn plan_layout(source: &[u8], config: &LayoutConfig) -> Result<LayoutPlan> {
    let circuit = r1cs::read(source).context("layout source")?;
    let (config, rewritten, layout, checked) = packed::plan(&circuit, config)?;
    Ok(LayoutPlan {
        rewritten_circuit: rewritten,
        layout,
        map: checked.map,
        checked_equations: checked.equations.len(),
        config,
    })
}

/// Compile `source` to a packed PILout and check the result.
///
/// A missing `width`, `minRows` or `rowGroupWidth` is chosen by building the
/// PILout of each candidate and estimating its proving time with
/// pil2-proofman; the other settings take their defaults (`auxiliaryTerms`
/// half of `width`, `available` reuse, constant folding). The artifact's
/// `config` holds the settings used.
///
/// # Errors
/// Fails on malformed R1CS or invalid settings, and if the checker rejects any
/// auxiliary wire, placement, public binding or copy constraint.
pub fn compile_packed(source: &[u8], config: &LayoutConfig) -> Result<PackedArtifact> {
    let artifact = packed::compile(source, config).context("packed compilation")?;
    check_packed(source, &artifact.pilout, &artifact.certificate)?;
    Ok(artifact)
}

/// Check that `pilout` encodes the packed relation of `source` as described by
/// `certificate`: auxiliary wires, placement, selectors, public bindings, the
/// permutation argument and hints.
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
/// malformed, or if it does not satisfy every source and auxiliary equation.
pub fn project_witness(
    source: &[u8],
    pilout: &[u8],
    certificate: &PackedCertificate,
    wtns: &[u8],
) -> Result<Witness> {
    check_packed(source, pilout, certificate)?;
    witness::project(source, certificate, wtns).context("witness projection")
}

/// Compile `source` to a module fragment for embedding in a host AIR, with
/// `exports` the wires the host may constrain, and check the result. Every
/// source equation becomes one constraint over one stage-one column per
/// nonconstant wire; the circuit's public values are not bound to anything.
/// With `optimize`, expressions are simplified as in
/// [`compile_optimized_reference`].
///
/// # Errors
/// Fails on malformed or unsupported R1CS, on an export that is not a
/// nonconstant wire or is listed twice, and on circuits beyond the checker's
/// size limits.
pub fn compile_module(source: &[u8], exports: &[u32], optimize: bool) -> Result<ModuleArtifact> {
    let circuit = r1cs::read(source).context("module source")?;
    let (pilout, certificate) =
        module::compile(source, &circuit, exports, optimize).context("module compilation")?;
    check_module(source, &pilout, &certificate)?;
    Ok(ModuleArtifact {
        pilout,
        certificate,
    })
}

/// Check that a module fragment encodes every equation of `source` as
/// described by `certificate`.
///
/// # Errors
/// Fails if a hash does not match, if the fragment contains anything a module
/// may not have, or if any constraint differs from its source equation.
pub fn check_module(
    source: &[u8],
    pilout: &[u8],
    certificate: &ModuleCertificate,
) -> Result<ModuleCheckReport> {
    module_check::check_module(source, pilout, certificate).context("module validation")
}

/// Check that the host PILout `host` preserves the module of `source` under
/// `binding`: the selected AIR enforces every source equation on every row
/// over the bound stage-one columns. Nothing is checked about the host's other
/// constraints, which may only restrict the trace further.
///
/// # Errors
/// Fails on an invalid certificate or binding, on a binding that misses a
/// wire, repeats one or gives two wires the same column, and when some source
/// equation is not exactly the polynomial of an every-row constraint.
pub fn check_composition(
    source: &[u8],
    host: &[u8],
    certificate: &ModuleCertificate,
    binding: &CompositionBinding,
) -> Result<CompositionReport> {
    module_check::check_composition(source, host, certificate, binding)
        .context("composition validation")
}
