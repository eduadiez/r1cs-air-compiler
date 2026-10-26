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
that each row holds several small equations instead of the whole circuit. The
relation is: there is an assignment `w` that satisfies every equation of the
circuit with `w_{j+1} = public[j]` for every public index `j`, extended to the
helpers below by their definitions, such that every cell of each wire `u` holds
`w_u`.

**Helpers.** To limit each equation to at most `width` distinct wires, the
compiler introduces helper wires. A helper `h` is defined by
`h = Σ kᵢ · wᵢ`: at least two terms, the first coefficient 1, and every `wᵢ` a
nonconstant wire earlier than `h`. Each source equation is rewritten into a
converted equation that may use helpers. The checker replaces every helper by
its definition and requires `A · B - C` of the result to be the same polynomial
as `A · B - C` of the source equation.

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
that hold public cells. Those rows are numbered `0 … q-1` in increasing row
order and split into groups
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
`src/field.rs`. The compiler and the checker both check that all labels are
distinct. The fixed
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
`z = 1` on the last row, row 0 reads a previous value of 1, and, when every
denominator factor is nonzero, the constraints hold exactly when the product of
all `num_c` equals the product of all `den_c`.

**Degree reduction.** Every factor is linear in the columns, and each constraint
has degree at most three (`MAX_DEGREE` in `src/contract.rs`): a STARK prover
with a blowup factor of 2 handles that degree without adding columns of its
own. Since `width ≥ 3`, the products above never fit, so the compiler adds
stage-two ratio columns (`copy_ratios`) holding partial products, each
constrained by `ratio_i · den_i = num_i`:

- the first ratio takes up to three numerator factors and up to two
  denominator factors;
- each later ratio takes the previous ratio and the next two numerator factors
  over the next two denominator factors;
- ratios are added until at most two numerator factors (counting the last
  ratio) and two denominator factors remain; the remaining factors go into the
  `z` constraint in place of the full products.

This gives `⌊(width - 1) / 2⌋` ratio columns. The
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

## Correctness argument

This section argues on paper that the soundness and completeness properties
hold for every artifact the checker accepts. It assumes that acceptance means
the certificate passes the checks described above (for packed artifacts: the
helper and placement identities, one wire per cell, the public bindings, the
active slot rows, `requiresCopy` and distinct labels) and that the PILout
contains exactly the constraints, fixed columns and hints described above; whether the checker's code achieves that is a separate question (see
"Status of the properties"). Constraints are read as a backend reads them: an
`EveryRow` constraint must vanish on every row, row offsets wrap around, and
the copy challenges `α` and `γ` are drawn from a field `K` containing the
Goldilocks field after the stage-one trace is fixed.

### Reference relation

*Soundness.* Let a trace satisfy the AIR. Every equation constraint vanishes on
every row `r`, so `assignment(r, ·)` satisfies every equation. On row 0,
`FIRST = 1`, so the public constraints give `assignment(0, j + 1) = public[j]`. No challenge appears in a constraint, so
this holds with certainty.

*Completeness.* Given an assignment `w` that satisfies the circuit with public
values `public`, put `w` on every row, which is possible because each wire
`i ≥ 1` has its own column `m(i)`, and fill `stage2_zero` with 0, which is
what its `gsum_col` hint computes (a zero numerator over 1). Every equation
constraint then holds on every row and the public constraints hold on row 0.

### Packed relation

Let `M = n · width` be the number of cells; the checker requires
`M ≤ MAX_CELLS`. Write `label(x)` for the label of cell `x`, and `prev(x)` for
the previous cell of the same wire, so that `S` holds `label(prev(x))` at cell
`x`. The map `prev` is a permutation of the cells whose cycles are exactly the
sets of cells of each wire; an unused cell, or the only cell of a wire, is its
own cycle.

**Lemma 1 (copy argument).** Fix the stage-one values `a(x)`, and suppose every
denominator factor `a(x) + α · label(x) + γ` is nonzero.

1. If stage-two values satisfy the ratio, recurrence and `LAST` constraints,
   then `∏ num = ∏ den` over all cells.
2. If `a` is constant on the cells of every wire, the columns computed by the
   hints satisfy those constraints.

For (1), each ratio constraint has the form `ratio · d = e` with `d` a product
of denominator factors, so `ratio = e / d`; by induction over the ratios, the
`z` constraint on row `r` reads `z[r] = z[r - 1] · N[r] / D[r]`, where `N[r]`
and `D[r]` are the products of the numerator and denominator factors of row
`r`. Multiplying over all rows, cyclically, gives
`z[n - 1] = z[n - 1] · ∏ N / ∏ D`, and `LAST` gives `z[n - 1] = 1`. For (2),
the hints compute the same ratios and the running product from row 0, so
`z[r] = z[r - 1] · N[r] / D[r]` for `r ≥ 1` and `z[0] = N[0] / D[0]`, and the
last value is `z[n - 1] = ∏ N / ∏ D`. Since `prev` is a permutation and
`a(prev(x)) = a(x)`, the numerator factors `a(x) + α · label(prev(x)) + γ` are
a reordering of the denominator factors, so `z[n - 1] = 1`. Hence `LAST` holds,
and so does the recurrence on row 0, which reads `z[n - 1] = 1`.

**Lemma 2 (copy failure probability).** Fix `a` before `α` and `γ` are drawn
uniformly from `K`. Except with probability at most `2M / |K|`, every
denominator factor is nonzero, and `∏ num = ∏ den` holds only if the cells of
each wire hold equal values.

As polynomials in `α` and `γ`, each denominator factor has the form `γ + …` and
is nonzero, so it vanishes with probability at most `1 / |K|`; there are `M` of
them. Next, `F = ∏ num - ∏ den` has total degree at most `M`. If `F` is the zero
polynomial, unique factorization in `K[α, γ]` makes the multisets of pairs
`(a(x), label(prev(x)))` and `(a(x), label(x))` equal. The labels are distinct,
so the pair `(a(x), label(prev(x)))` can only equal `(a(prev(x)), label(prev(x)))`,
which gives `a(x) = a(prev(x))` for every cell: values are constant on every
cycle of `prev`, that is, on the cells of every wire. Otherwise `F` is nonzero
and, by the Schwartz–Zippel lemma, vanishes with probability at most `M / |K|`.

**Lemma 3 (public constraints).** On every row `r` and column `c`, the sum
`Σ PH · PL · public[j]` is `public[j]` if `r` holds the public cell of value `j`
in column `c`, and 0 otherwise. `PH{h} · PL{l}` is 1 only on the public row
numbered `h · s + l`; two public values bound in the same column are different
wires, so they are on different rows. Hence the constraint for column `c`
requires `a[c] = public[j]` on each such cell, and nothing on other rows,
where `PA{c}` and every product `PH · PL` in the sum vanish.

*Soundness.* Let a trace satisfy the AIR for challenges that avoid the
exceptions of Lemma 2. By Lemmas 1 and 2 (or trivially if the PILout has no
copy argument, since each wire then has at most one cell), every wire's cells
hold one value; let `w` be that value, `w₀ = 1`, and 0 for a wire without
cells. On every row where a slot is active, its selector is 1, so the
equation placed there holds for `w`. Every helper definition is placed, so
each helper equals its linear combination of earlier wires. Every converted
equation is placed, and the checker requires `A · B - C` of each converted
equation, with every helper replaced by its definition, to be the same
polynomial as `Aᵢ · Bᵢ - Cᵢ` of its source equation; so the source equation
holds for `w`. These are polynomial identities, so they hold whatever value a
wire without cells takes, in particular 0. By Lemma 3, `w_{j+1} = public[j]`.

*Completeness.* Given an assignment `w` that satisfies the circuit with public
values `public`, compute each helper from its definition, in order, and put
each wire's value in all of its cells and 0 in unused cells. Each placed
equation is a helper definition or a converted equation, and both hold, so
every slot constraint holds on its active rows and vanishes elsewhere. By
Lemma 3, the public constraints hold. If every denominator factor is nonzero,
Lemma 1 (2) gives stage two; this fails with probability at most `M / |K|`.
Without a copy argument, `stage2_zero` is 0 as in the reference relation.

With `M ≤ MAX_CELLS < 2^22` and `|K| ≥ P > 2^63`, the bound `2M / |K|` is below
`2^-40` for one draw of the challenges. The bound is for challenges drawn
after a fixed stage-one trace; under Fiat–Shamir a prover can try many traces,
and this argument does not cover that.

## Status of the properties

- **Checker correctness:** that the checker accepts only artifacts whose PILout
  contains exactly the constraints, columns and hints described above. This is
  not proved; rejection tests and mutation tests exercise it.
- **Soundness and completeness:** argued on paper above, assuming checker
  correctness. The argument has not been checked by a proof assistant.
- **Binding to bytes:** the properties apply to the canonical decoded PILout
  bytes. The parsers, the hash and the serialization must either be covered or
  be listed as trusted.

The polynomial comparison is an ordinary Rust implementation of polynomial
equality over the field. Rejection tests exercise it; they do not prove it correct.

Soundness and completeness are also tested by property tests over random
satisfiable circuits and random layouts. A test-only evaluator, which reads
only the PILout, fills stage two from the hints and evaluates every constraint
on every row for random challenges. The tests require that the honest trace of
every reference and packed artifact satisfies its AIR, and that whenever a
changed trace (one cell, every cell of one wire, or one public value) still
satisfies it, the assignment read back through the certificate satisfies the
circuit with the same public values. [Testing](docs/TESTING.md) describes them.

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
