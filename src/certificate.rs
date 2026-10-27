//! Data carried by certificates: the rewritten circuit, the packed layout, the
//! three certificate types and a module's composition binding, with read-only
//! accessors and the hash used to bind them. Nothing here builds or checks certificates, so the checkers can
//! read them without depending on the code that builds them.
use crate::field::FieldElement;
use crate::r1cs::{Equation, LinearCombination};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Lowercase hexadecimal SHA-256 digest, as stored in certificates.
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Equation terms with wires replaced by their position in the gate type; position
/// -1 is the constant 1.
pub type GateTerms = [Vec<(i32, FieldElement)>; 3];

/// Auxiliary wire `id`, constrained to equal `terms`. The first coefficient is 1
/// and every term references an earlier wire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Auxiliary {
    pub id: u32,
    pub terms: LinearCombination,
}
/// The source circuit after rewriting: its auxiliary wires and its rewritten equations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RewrittenCircuit {
    /// Wire count of the source circuit.
    pub original_wires: u32,
    /// Source wires plus auxiliary wires; auxiliary ids are `original_wires..wires`.
    pub wires: u32,
    pub publics: u32,
    pub auxiliaries: Vec<Auxiliary>,
    /// One equation per source equation, in source order, possibly referencing auxiliary wires.
    pub equations: Vec<Equation>,
}

/// A distinct equation gate type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateType {
    pub id: usize,
    /// Number of wires in the gate type.
    pub width: usize,
    pub terms: GateTerms,
}
/// Where one equation is placed: gate type position `i` stands for `wires[i]`,
/// stored at `(row, columns[i])`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Placement {
    /// Index of the equation: auxiliary definitions first, then rewritten equations.
    pub index: usize,
    /// CanonicalGate group id.
    pub gate_type: usize,
    pub wires: Vec<u32>,
    pub row: usize,
    pub columns: Vec<usize>,
}
/// One PILout constraint: gate type `gate_type` applied to `columns`, enforced on
/// `rows` by selector column `selector`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Gate {
    pub gate_type: usize,
    pub columns: Vec<usize>,
    pub rows: Vec<usize>,
    pub selector: usize,
}
/// A fixed column that is 1 on `rows` and 0 elsewhere.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selector {
    pub id: usize,
    pub rows: Vec<usize>,
}
/// Public value `public` must equal the cell `(row, col)`, which holds `wire`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub wire: u32,
    pub public: u32,
    pub row: usize,
    pub col: usize,
}
/// A complete packed layout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Layout {
    pub min_rows: usize,
    /// Number of stage-one columns.
    pub width: usize,
    /// Trace rows: `used_rows` rounded up to a power of two, at least `min_rows`.
    pub rows: usize,
    /// Rows up to the last one holding an equation or a public cell (at least 1).
    pub used_rows: usize,
    /// The last row holding an equation, plus one.
    pub arithmetic_rows: usize,
    pub gate_types: Vec<GateType>,
    pub placements: Vec<Placement>,
    pub gates: Vec<Gate>,
    pub selectors: Vec<Selector>,
    pub public_bindings: Vec<Binding>,
    /// Whether some wire occupies more than one cell.
    pub requires_permutation: bool,
}
/// Certificate for a reference artifact: the source and PILout it binds, the
/// trace geometry and the column of every wire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReferenceCertificate {
    /// Certificate format version.
    pub(crate) version: u32,
    /// Name of the certified relation.
    pub(crate) relation: String,
    pub(crate) source_sha256: String,
    pub(crate) pilout_sha256: String,
    pub(crate) rows: u32,
    /// Number of stage-one columns: one per wire except the constant wire 0.
    pub(crate) width: u32,
    /// Public wires in public-value order: `1..=publics`.
    pub(crate) public_wires: Vec<u32>,
    /// Column of each wire. Wire 0 is the constant 1 and has no column; every
    /// other wire has its own column.
    pub(crate) wire_columns: Vec<Option<u32>>,
}
impl ReferenceCertificate {
    /// Certificate format version.
    pub fn version(&self) -> u32 {
        self.version
    }
    /// Name of the certified relation: [`crate::REFERENCE_RELATION`].
    pub fn relation(&self) -> &str {
        &self.relation
    }
    /// SHA-256 of the R1CS file, in lowercase hexadecimal.
    pub fn source_sha256(&self) -> &str {
        &self.source_sha256
    }
    /// SHA-256 of the PILout file, in lowercase hexadecimal.
    pub fn pilout_sha256(&self) -> &str {
        &self.pilout_sha256
    }
    /// Number of trace rows.
    pub fn rows(&self) -> usize {
        self.rows as usize
    }
    /// Number of stage-one columns.
    pub fn width(&self) -> usize {
        self.width as usize
    }
    /// Public wires in public-value order: `1..=publics`.
    pub fn public_wires(&self) -> &[u32] {
        &self.public_wires
    }
    /// Column of each wire; wire 0, the constant 1, has none.
    pub fn wire_columns(&self) -> &[Option<u32>] {
        &self.wire_columns
    }
}

/// Certificate for a packed artifact: the source and PILout it binds, the
/// rewritten circuit and the layout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackedCertificate {
    /// Certificate format version.
    pub(crate) version: u32,
    /// Name of the certified relation.
    pub(crate) relation: String,
    pub(crate) source_sha256: String,
    pub(crate) pilout_sha256: String,
    pub(crate) rewritten_circuit: RewrittenCircuit,
    pub(crate) layout: Layout,
}
impl PackedCertificate {
    /// Certificate format version.
    pub fn version(&self) -> u32 {
        self.version
    }
    /// Name of the certified relation: [`crate::PACKED_RELATION`].
    pub fn relation(&self) -> &str {
        &self.relation
    }
    /// SHA-256 of the R1CS file, in lowercase hexadecimal.
    pub fn source_sha256(&self) -> &str {
        &self.source_sha256
    }
    /// SHA-256 of the PILout file, in lowercase hexadecimal.
    pub fn pilout_sha256(&self) -> &str {
        &self.pilout_sha256
    }
    /// Number of trace rows.
    pub fn rows(&self) -> usize {
        self.layout.rows
    }
    /// Number of stage-one columns.
    pub fn width(&self) -> usize {
        self.layout.width
    }
    /// Whether some wire occupies more than one cell, so that the PILout
    /// contains a permutation argument.
    pub fn requires_permutation(&self) -> bool {
        self.layout.requires_permutation
    }
    /// Number of auxiliary wires.
    pub fn auxiliary_count(&self) -> usize {
        self.rewritten_circuit.auxiliaries.len()
    }
}

/// Certificate for a module artifact: the source and module fragment it binds,
/// the column of every wire in the fragment and the exported wires.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModuleCertificate {
    /// Certificate format version.
    pub(crate) version: u32,
    /// Name of the certified relation.
    pub(crate) relation: String,
    pub(crate) source_sha256: String,
    /// SHA-256 of the module fragment (a PILout message).
    pub(crate) pilout_sha256: String,
    /// Number of stage-one columns of the fragment: one per wire except the
    /// constant wire 0.
    pub(crate) width: u32,
    /// Column of each wire in the fragment. Wire 0 is the constant 1 and has no
    /// column; every other wire has its own column.
    pub(crate) wire_columns: Vec<Option<u32>>,
    /// Wires the host may constrain, in the order given at compilation.
    pub(crate) exports: Vec<u32>,
}
impl ModuleCertificate {
    /// Certificate format version.
    pub fn version(&self) -> u32 {
        self.version
    }
    /// Name of the certified relation: [`crate::MODULE_RELATION`].
    pub fn relation(&self) -> &str {
        &self.relation
    }
    /// SHA-256 of the R1CS file, in lowercase hexadecimal.
    pub fn source_sha256(&self) -> &str {
        &self.source_sha256
    }
    /// SHA-256 of the module fragment, in lowercase hexadecimal.
    pub fn pilout_sha256(&self) -> &str {
        &self.pilout_sha256
    }
    /// Number of stage-one columns of the fragment.
    pub fn width(&self) -> usize {
        self.width as usize
    }
    /// Column of each wire in the fragment; wire 0, the constant 1, has none.
    pub fn wire_columns(&self) -> &[Option<u32>] {
        &self.wire_columns
    }
    /// Exported wires.
    pub fn exports(&self) -> &[u32] {
        &self.exports
    }
}

/// Where a module's wires are in a host PILout: the AIR, and the stage-one
/// column of every nonconstant wire (all at row offset 0).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompositionBinding {
    /// Index of the AIR group in the host PILout.
    pub air_group: u32,
    /// Index of the AIR in that group.
    pub air: u32,
    /// The host column of each nonconstant wire.
    pub columns: Vec<WireColumn>,
}
impl CompositionBinding {
    /// A binding that puts the module's columns at `offset` and above: fragment
    /// column `c` becomes host column `offset + c`.
    pub fn with_offset(
        certificate: &ModuleCertificate,
        air_group: u32,
        air: u32,
        offset: u32,
    ) -> Self {
        let columns = certificate
            .wire_columns
            .iter()
            .enumerate()
            .filter_map(|(wire, column)| {
                column.map(|column| WireColumn {
                    wire: wire as u32,
                    column: offset.saturating_add(column),
                })
            })
            .collect();
        Self {
            air_group,
            air,
            columns,
        }
    }
}

/// One wire and its host stage-one column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireColumn {
    /// R1CS wire id, at least 1.
    pub wire: u32,
    /// Stage-one column of the host AIR.
    pub column: u32,
}
