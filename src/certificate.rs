//! Data carried by certificates: the helper network, the packed layout and
//! the two certificate types, with read-only accessors and the hash used to
//! bind them. Nothing here builds or checks certificates, so the checkers can
//! read them without depending on the code that builds them.
use crate::field::FieldElement;
use crate::r1cs::{Equation, LinearCombination};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Lowercase hexadecimal SHA-256 digest, as stored in certificates.
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Equation terms with wires replaced by their position in the shape; position
/// -1 is the constant 1.
pub type ShapeTerms = [Vec<(i32, FieldElement)>; 3];

/// Helper wire `id`, constrained to equal `terms`. The first coefficient is 1
/// and every term references an earlier wire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Helper {
    pub id: u32,
    pub terms: LinearCombination,
}
/// The source circuit after rewriting: its helpers and its rewritten equations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Network {
    /// Wire count of the source circuit.
    pub original_wires: u32,
    /// Source wires plus helpers; helper ids are `original_wires..wires`.
    pub wires: u32,
    pub publics: u32,
    pub helpers: Vec<Helper>,
    /// One equation per source equation, in source order, possibly referencing helpers.
    pub converted: Vec<Equation>,
}

/// A distinct equation shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub id: usize,
    /// Number of wires in the shape.
    pub width: usize,
    pub terms: ShapeTerms,
}
/// Where one equation is placed: shape position `i` stands for `wires[i]`,
/// stored at `(row, columns[i])`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    /// Index of the equation: helper equations first, then converted equations.
    pub index: usize,
    /// Shape group id.
    pub shape: usize,
    pub wires: Vec<u32>,
    pub row: usize,
    pub columns: Vec<usize>,
}
/// One PILout constraint: shape `shape` applied to `columns`, enforced on
/// `rows` by selector column `selector`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slot {
    pub shape: usize,
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
    /// Rows holding at least one equation: the last such row plus one.
    pub arithmetic_rows: usize,
    pub groups: Vec<Group>,
    pub placements: Vec<Placement>,
    pub slots: Vec<Slot>,
    pub selectors: Vec<Selector>,
    pub public_bindings: Vec<Binding>,
    /// Whether some wire occupies more than one cell.
    pub requires_copy: bool,
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
/// helper network and the layout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackedCertificate {
    /// Certificate format version.
    pub(crate) version: u32,
    /// Name of the certified relation.
    pub(crate) relation: String,
    pub(crate) source_sha256: String,
    pub(crate) pilout_sha256: String,
    pub(crate) network: Network,
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
    /// contains a copy argument.
    pub fn requires_copy(&self) -> bool {
        self.layout.requires_copy
    }
    /// Number of helper wires.
    pub fn helper_count(&self) -> usize {
        self.network.helpers.len()
    }
}
