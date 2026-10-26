//! Fixed conventions and size limits of the generated artifacts, shared by the
//! compiler and the checkers. Contains only constants.

/// Certificate format version.
pub const CERTIFICATE_VERSION: u32 = 1;
/// Relation certified by reference artifacts.
pub const REFERENCE_RELATION: &str = "r1cs-per-row-publics-at-row-zero";
/// Relation certified by packed artifacts.
pub const PACKED_RELATION: &str = "r1cs-packed-static-copy";

/// Name of the PILout.
pub const PILOUT_NAME: &str = "kernel";
/// Name of the single AIR group and AIR; fixed columns are named `Kernel.{name}`.
pub const AIR_NAME: &str = "Kernel";
/// Symbol name of the stage-one columns.
pub const TRACE_SYMBOL: &str = "a";
/// Symbol name of the public values.
pub const PUBLICS_SYMBOL: &str = "publics";
/// Symbol name of the permutation argument's ratio columns.
pub const RATIOS_SYMBOL: &str = "permutation_ratios";
/// Name of the fixed column that is 1 on row 0 only.
pub const FIRST_COLUMN: &str = "FIRST";
/// Hint computing a ratio column.
pub const RATIO_HINT: &str = "im_col";
/// Hint computing the permutation argument's running product.
pub const PRODUCT_HINT: &str = "gprod_col";
/// Hint computing the zero column.
pub const SUM_HINT: &str = "gsum_col";

/// Constraint labels (`debug_line`), checked like every other field.
pub mod label {
    /// The zero column's constraint.
    pub const ZERO_COLUMN: &str = "stage-two zero";
    /// A packed gate constraint.
    pub const PACKED_GATE: &str = "packed gate";
    /// A packed public-binding constraint.
    pub const PACKED_PUBLIC: &str = "packed public";
    /// A permutation-argument ratio constraint.
    pub const PERMUTATION_RATIO: &str = "permutation ratio";
    /// The permutation argument's running-product recurrence.
    pub const PERMUTATION_RECURRENCE: &str = "permutation recurrence";
    /// The permutation argument's last-row closure.
    pub const PERMUTATION_CLOSURE: &str = "permutation closure";
    /// The reference constraint of source equation `index`.
    pub fn source_equation(index: usize) -> String {
        format!("R1CS {index}")
    }
    /// The reference binding of public value `index`.
    pub fn public(index: usize) -> String {
        format!("public {index}")
    }
}

/// Stage of the trace columns.
pub const STAGE_ONE: u32 = 1;
/// Stage of the columns computed after the challenges are drawn.
pub const STAGE_TWO: u32 = 2;
/// Stage-two column 0: the permutation argument's running product, or the zero column.
pub const PRODUCT_COLUMN: u32 = 0;
/// Symbol name of stage-two column 0 with a permutation argument.
pub const PRODUCT_COLUMN_NAME: &str = "permutation_product";
/// Symbol name of stage-two column 0 without a permutation argument; it is
/// constrained to zero.
pub const ZERO_COLUMN_NAME: &str = "stage2_zero";
/// Index of the stage-two challenge `α`.
pub const ALPHA: u32 = 0;
/// Index of the stage-two challenge `γ`.
pub const GAMMA: u32 = 1;
/// Symbol names of the challenges `α` and `γ`.
pub const CHALLENGE_NAMES: [&str; 2] = ["alpha", "gamma"];
/// Highest degree of any constraint. Degree 3 is what a STARK prover with a
/// blowup factor of 2 handles without adding columns of its own, so each
/// permutation ratio takes two denominator factors and two numerator factors plus the
/// previous ratio (three numerator factors for the first).
pub const MAX_DEGREE: usize = 3;

/// Smallest row count (and smallest `minRows`).
pub const MIN_ROWS: usize = 64;
/// Largest row count.
pub const MAX_ROWS: usize = 1 << 16;
/// Smallest packed width.
pub const MIN_WIDTH: usize = 3;
/// Largest packed width.
pub const MAX_WIDTH: usize = 1024;
/// Largest packed trace, in cells.
pub const MAX_CELLS: usize = 4_000_000;

/// Largest accepted R1CS file.
pub const MAX_R1CS_BYTES: usize = 256 << 20;
/// Largest number of wires or equations in a circuit.
pub const MAX_CIRCUIT_SIZE: u32 = 1_000_000;
/// Largest accepted PILout file.
pub const MAX_PILOUT_BYTES: usize = 128 << 20;
/// Largest number of expressions in an AIR.
pub const MAX_EXPRESSIONS: usize = 300_000;
/// Largest number of term products in one polynomial multiplication.
pub const MAX_PRODUCT_TERMS: usize = 1_000_000;
/// Largest total number of terms over all evaluated expressions.
pub const MAX_EVALUATED_TERMS: usize = 2_000_000;
/// Largest number of source wires in the expansion of one auxiliary wire.
pub const MAX_AUXILIARY_EXPANSION: usize = 100_000;
