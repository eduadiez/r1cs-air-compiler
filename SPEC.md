# Specification

The compiler turns an R1CS circuit into a PILout file (an AIR) and a JSON
certificate. Two output forms exist, each encoding a different relation:
reference and packed. For each one, an independent checker reads the source,
the PILout and the certificate, and either accepts them or reports why not.

Status: the checks are carried out by a program. No part of this specification
has been proved in a proof assistant, and check reports always contain
`"formallyVerified": false`.

## Scope and assumptions

The checker establishes one property: if it accepts an R1CS file, a PILout file
and a certificate, then the AIR decoded from the PILout encodes the relation
described below for that R1CS. It rejects malformed or inconsistent input with
an error; it never accepts it and never panics.

Inputs are assumed to come from a cooperating user, not from an adversary trying
to exhaust resources. The parser and the checkers limit input sizes, circuit
dimensions and the size of intermediate polynomials, but there is no limit on
total time or memory. A crafted certificate can make a check take time and
memory quadratic in its size before it is rejected, and the CLI reads whole
files into memory. Run the checker on untrusted artifacts only under external
resource limits.

Both the compiler and the checkers rely on these components, which are trusted
rather than checked: the R1CS and WTNS parser, the field arithmetic and the
constants in `src/field.rs`, the protobuf schema and decoder, SHA-256, the Rust
toolchain, and the backend that reads the PILout.

## Notation

All arithmetic is in the Goldilocks field, `P = 2^64 - 2^32 + 1`.

An R1CS circuit has wires `w₀ … w_{N-1}`, where `w₀ = 1` is constant. Wires
`w₁ … w_k` are public: first the outputs, then the public inputs. Each equation
`i` has the form `Aᵢ(w) · Bᵢ(w) = Cᵢ(w)`, where `Aᵢ`, `Bᵢ` and `Cᵢ` are linear
combinations of wires.

The AIR has `n` rows, a power of two. Stage one holds the witness columns `a[c]`.
Stage two holds columns computed after two challenges, `α` and `γ` (symbols
`alpha` and `gamma`), are drawn.
Fixed columns hold values known at compile time. A constraint is a polynomial
that must be zero on every row.

## Reference relation (`r1cs-per-row-publics-at-row-zero`)

The trace has one stage-one column per nonconstant wire: `N - 1` columns. The
certificate's `wireColumns` gives the column `m(i)` of each wire `i ≥ 1`; every
such wire has its own column. On row `r`, let `assignment(r, 0) = 1` and
`assignment(r, i) = a[m(i)][r]`.

The relation is:

1. For every row `r`, every equation of the circuit holds for `assignment(r, ·)`.
2. For every public index `j`, `assignment(0, j + 1) = public[j]`.

Nothing relates different rows to each other.

Each equation of the circuit is one AIR constraint over its own columns, so the
PILout grows with the number of constraints and wires. A circuit whose
expressions exceed the size limits of the checker (`MAX_EXPRESSIONS` and
`MAX_EVALUATED_TERMS` in `src/contract.rs`) is rejected by the compiler with a
limit error; such circuits need the packed relation.

The PILout contains exactly these constraints, in this order:

- `stage2_zero = 0`, where `stage2_zero` is stage-two column 0;
- `Aᵢ · Bᵢ - Cᵢ` for each equation `i`, with each wire replaced by its column;
- `FIRST · (a[m(j + 1)] - public[j])` for each public index `j`, where the fixed
  column `FIRST` is 1 on row 0 and 0 elsewhere.

It also contains the hint for `stage2_zero` (see "Hints"). The checker
evaluates every PILout expression to an exact polynomial and requires each
constraint to equal the expected one. It also checks the `FIRST` column values,
every symbol and every metadata field, rather than equation labels or a digest
supplied by the compiler.

With `--optimize`, the compiler simplifies expressions using only identities that
hold for every field value and reuses repeated subexpressions. The polynomials,
the certificate format and the checker are the same as without it.

## Packed relation (`r1cs-packed-static-copy`)

A single assignment of the circuit is spread over the cells of the trace, so
that each row holds several small equations instead of the whole circuit.

**Helpers.** To limit each equation to at most `width` distinct wires, the
compiler introduces helper wires. A helper `h` is defined by
`h = Σ kᵢ · wᵢ`: at least two terms, the first coefficient 1, and every `wᵢ` a
nonconstant wire earlier than `h`. Each source equation is rewritten into a converted equation that may use
helpers. The checker replaces every helper by its definition and requires the
result to be exactly the source equation, as polynomials.

**Placement.** Every helper definition and every converted equation is placed
exactly once, on one row, with each of its wires in a cell of that row. Two
different wires never share a cell; one wire may occupy several cells.

**Shapes, slots and selectors.** An equation's shape is the equation with its
wires replaced by their positions. A slot is a shape applied to specific
columns. Each slot has a fixed selector column `E{i}` that is 1 on the rows
where the slot is used and 0 elsewhere; slots used on the same rows share one
selector. The PILout contains, for each slot, the constraint
`E · (A · B - C) = 0`, with each shape position read from its column. The
checker requires every placement to be on a row where its slot is active, every
active row of a slot to hold a placement of that slot, every selector to be
used by a slot and every shape by an equation. Only nonconstant wires occupy
cells.

**Public values.** Public value `j` is bound to a cell holding wire `j + 1`;
the checker accepts any such cell, and the compiler uses the first cell it
assigned to the wire. Let `q` be the number of distinct rows
that hold public cells. Those rows are numbered `0 … q-1` and split into groups
of `s = ⌈√q⌉`. The fixed column `PH{h}`
is 1 on the rows of group `h`, and `PL{l}` on the rows with position `l` inside
their group, so `PH · PL` is 1 on exactly one row. For each column `c` holding
public cells, the fixed column `PA{c}` is 1 on those rows, and the constraint is
`PA{c} · a[c] = Σ PH · PL · public[j]`, summed over the public values bound in
column `c`.

**Copies.** If some wire occupies more than one cell, a copy argument (next
section) forces all its cells to hold the same value. If every wire occupies at
most one cell, the PILout has no copy argument; stage two then holds only
`stage2_zero`, constrained to zero as in the reference relation.

## Copy argument

Every cell has its own label: `ω^r · COSET^c` for row `r` and column `c`, where
`ω = 7^((P - 1) / n)` generates the row domain and `COSET` is the constant in
`src/field.rs`. The compiler checks that all labels are distinct. The fixed
column `ID` holds `ω^r`. For each column `c`, the fixed column `S{c}` holds the
permuted labels: each cell of a wire holds the label of the previous cell of
the same wire, in row-major order and cyclically. An unused cell, or the only
cell of a wire, keeps its own label. The checker rebuilds all these columns
from the layout.

For each row and column `c`, define the factors

```text
num_c = a[c] + α · S{c} + γ
den_c = a[c] + α · COSET^c · ID + γ
```

A stage-two column `z` (`copy_product`) holds, on row `r`, the product of
`num_c / den_c` over all cells of rows `0 … r`. The constraints are:

```text
z[r] · ∏ den_c[r] = z[r - 1] · ∏ num_c[r]     (row 0 reads row n - 1)
LAST · (z - 1) = 0
```

`LAST` is `FIRST` read at the next row, so it is 1 only on the last row. Since
`z = 1` on the last row, row 0 reads a previous value of 1, and the constraints
hold exactly when the product of all `num_c` equals the product of all `den_c`.

**Degree reduction.** Every factor is linear in the columns, and each constraint
must have degree at most five. For `width = 3` the products above already fit.
For every larger width the compiler adds stage-two ratio columns (`copy_ratios`)
holding partial products, each constrained by `ratio_i · den_i = num_i`:

- the first ratio takes up to five numerator factors and up to four
  denominator factors;
- each later ratio takes the previous ratio and the next four numerator factors
  over the next four denominator factors;
- ratios are added until at most four numerator factors (counting the last
  ratio) and four denominator factors remain; the remaining factors go into the
  `z` constraint in place of the full products.

This gives `max(1, ⌊(width - 1) / 4⌋)` ratio columns for `width > 3`. The
checker computes this count and the stage-two width itself, rejects references
to any stage-two column beyond that width, and compares the ratio, recurrence
and `LAST` constraints with its own reconstruction, operation by operation.

## Hints

Hints tell the backend how to compute the stage-two columns. The checker
requires every hint to match exactly:

- `im_col` for each ratio column: `reference` is the column, with its
  `numerator` and `denominator` as in its constraint;
- `gprod_col` for `z`: the remaining numerator and denominator products;
- `gsum_col` for `stage2_zero`: a zero numerator, written as `α - α`.

## Artifact conventions

- `Air.numRows` holds the row count `n`, not `log2(n)` as a comment in the
  vendored schema says. Both checkers require this, and the backend exercised by
  `tools/backend-check` reads it the same way.
- Both constants that define copy labels (`7` and `COSET`) are fixed by a unit test.
- PILout bytes must be the canonical protobuf encoding of the decoded message,
  so unknown fields are rejected.
- Every constraint carries a fixed label in `debug_line` (for example
  `R1CS 3`, `packed equation`, `copy closure`), and both checkers require it.
- Constraints, fixed columns and hints must appear in the order the compiler
  emits them. The packed checker evaluates expressions in order up to
  the last slot or public constraint, so copy-argument expressions must come
  after those. An artifact that encodes the same relation in a different order
  is rejected.

## Certificates

Both certificate kinds contain `version` (the certificate format version,
always 1) and `relation`, the name of the relation they certify. The CLI chooses
the checker from `relation`. Every field below is checked.

Field names are in camelCase. A reference certificate contains `version`,
`relation`, `sourceSha256`, `piloutSha256`, `rows`, `width`, `publicWires` and
`wireColumns`.

A packed certificate contains `version`, `relation`, `sourceSha256` and
`piloutSha256`; the helper network `network` (`originalWires`, `wires`,
`publics`, `helpers` and the converted equations `converted`); and the layout
`layout` (`minRows`, `width`, `rows`, `usedRows`, `arithmeticRows`, the shapes
`groups`, `placements`, `slots`, `selectors`, `publicBindings` and
`requiresCopy`). Settings that
do not affect the relation (helper chunk size, reuse policy, folding and
placement strategy) are not recorded.

## Properties not yet formally proved

- **Checker correctness:** if the checker accepts an R1CS file, a PILout file
  and a certificate, the soundness and completeness properties below hold for
  them.
- **Soundness:** from every trace that satisfies the AIR, a satisfying
  assignment of the circuit with the same public values can be read back
  through the certificate's wire map. For packed artifacts this holds except
  with the failure probability of the copy argument over the choice of
  challenges.
- **Completeness:** every satisfying assignment of the circuit extends to a
  trace that satisfies the AIR, including helpers, unused cells and stage two.
  For packed artifacts this requires that no copy denominator is zero for the
  drawn challenges.
- **Binding to bytes:** the properties apply to the canonical decoded PILout
  bytes. The parsers, the hash and the serialization must either be covered or
  be listed as trusted.

Bounds on the failure probability, the zero-denominator cases and the effect of
deriving challenges by Fiat–Shamir have not been analyzed in full.

The polynomial comparison is an ordinary Rust implementation of polynomial
equality over the field. Rejection tests exercise it; they do not prove it correct.

Soundness and completeness are tested, not proved, by property tests over
random satisfiable circuits and random layouts. A test-only evaluator, which
reads only the PILout, fills stage two from the hints and evaluates every
constraint on every row for random challenges. The tests require that the
honest trace of every reference and packed artifact satisfies its AIR, and
that whenever a changed trace (one cell, every cell of one wire, or one public
value) still satisfies it, the assignment read back through the certificate
satisfies the circuit with the same public values. [Testing](docs/TESTING.md)
describes them.

## Out of scope

Preserving an R1CS relation says nothing about what the R1CS computes. Machine
semantics, execution across rows, memory ordering, buses and lookups would need
their own specifications. Likewise, the soundness of the STARK, the security of
Fiat–Shamir and zero knowledge do not follow from the compiler preserving the
relation.

A future extension could remove intermediate wires whose values can be
recomputed from their defining equations. That would need its own argument for
both soundness and completeness; removing an equation without one would break
the relation. Copies, selectors, range conditions and public order would still
have to be preserved.
