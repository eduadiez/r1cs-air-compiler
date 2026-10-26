# Specification

The compiler turns an R1CS circuit (a list of equations `A · B = C` over
wires) into a PILout file, the protobuf description of an AIR (the table of
columns and row constraints a STARK prover works on), and a JSON certificate.
There are two output forms, each encoding its own relation:

| Form | Relation | Trace |
| --- | --- | --- |
| reference | `r1cs-per-row-publics-at-row-zero` | one column per wire; every row satisfies the whole circuit |
| packed | `r1cs-packed-static-copy` | one assignment spread over the cells; each row holds a few small equations |

For each form, an independent checker reads the R1CS, the PILout and the
certificate, and either accepts them or reports why not.

Status: the checks are carried out by a program. Nothing in this
specification has been proved in a proof assistant, and every check report
contains `"formallyVerified": false`.

## Scope and assumptions

- **Property.** If the checker accepts an R1CS file, a PILout file and a
  certificate, the AIR decoded from the PILout encodes the relation below for
  that R1CS. Malformed or inconsistent input is rejected with an error; it is
  never accepted and never causes a panic.
- **Resources.** Inputs are assumed to come from a cooperating user, not from
  an adversary trying to exhaust resources. Input sizes, circuit dimensions and
  intermediate polynomials are limited (`src/contract.rs`); total time and
  memory are not. A crafted certificate can cost time and memory quadratic in
  its size before it is rejected, and the CLI reads whole files into memory.
  Check untrusted artifacts only under external resource limits.
- **Trusted, not checked:** the R1CS and WTNS parser, the field arithmetic and
  constants in `src/field.rs`, the protobuf schema and decoder, SHA-256, the
  Rust toolchain, and the backend that reads the PILout.

## Notation

- **Field.** All arithmetic is in the Goldilocks field, `P = 2^64 - 2^32 + 1`.
- **R1CS.** Wires `w₀ … w_{N-1}`, with `w₀ = 1`. Wires `w₁ … w_k` are public:
  first the outputs, then the public inputs; `public[j]` is the value of
  `w_{j+1}`. Equation `i` is `Aᵢ(w) · Bᵢ(w) = Cᵢ(w)`, where `Aᵢ`, `Bᵢ`, `Cᵢ`
  are linear combinations of wires.
- **Trace.** `n` rows, a power of two in `64 … 65536`.
  - Stage one: the witness columns `a[0] … a[width - 1]`.
  - Stage two: columns computed after two challenges `α` and `γ` (symbols
    `alpha` and `gamma`) are drawn. Column 0 is `stage2_zero` or
    `permutation_product`; columns `1, 2, …` are `permutation_ratios`.
  - Fixed columns: values known at compile time, named `Kernel.{name}`.
  - Cell: one row of one stage-one column.
- **Constraint.** A polynomial in the columns that must be zero on every row.
  It reads every column at the current row, except that the permutation argument reads
  `permutation_product` at the previous row and `FIRST` at the next row; row offsets
  wrap around.
- **Selector.** A fixed column that is 1 on some rows and 0 on the others;
  multiplying a constraint by it turns the constraint off on the other rows.

## Reference relation (`r1cs-per-row-publics-at-row-zero`)

**Trace.** `width = N - 1` stage-one columns, one per nonconstant wire. The
certificate's `wireColumns` gives the column `m(i)` of each wire `i ≥ 1`; the
checker requires each wire to have its own column (the compiler uses
`m(i) = i - 1`). The value of wire `i` on row `r` is `v(r, i) = a[m(i)][r]`,
and `v(r, 0) = 1`.

**Relation.**

1. On every row `r`, every equation holds for `v(r, ·)`.
2. For every public index `j`, `v(0, j + 1) = public[j]`.

Nothing relates different rows to each other.

**PILout.** Exactly these constraints, in this order:

1. `stage2_zero` (stage-two column 0);
2. `Aᵢ · Bᵢ - Cᵢ` for each equation `i`, with each wire replaced by its column;
3. `FIRST · (a[m(j + 1)] - public[j])` for each public index `j`, where the
   fixed column `FIRST` is 1 on row 0 and 0 elsewhere (present only when there
   are public values).

It also contains the `gsum_col` hint for `stage2_zero` (see [Hints](#hints)).

**Check.** The checker evaluates every PILout expression to an exact
polynomial and requires each constraint to equal the one it computes from the
R1CS. It also checks the `FIRST` values, every symbol and every other field. It
never relies on a label or digest supplied by the compiler.

**Size.** Each equation is one constraint, so the PILout grows with the
circuit. Compilation runs the checker, so a circuit beyond its size limits
(`MAX_EXPRESSIONS`, `MAX_EVALUATED_TERMS`) fails with a limit error; such
circuits need the packed form.

**`--optimize`.** The compiler simplifies expressions using only identities
that hold for every field value, and reuses repeated subexpressions. The
polynomials, the certificate and the checker are unchanged.

## Packed relation (`r1cs-packed-static-copy`)

**Relation.** There is an assignment `w` of the wires such that:

1. `w` satisfies every equation of the circuit;
2. `w_{j+1} = public[j]` for every public index `j`;
3. with `w` extended to the auxiliary wires below by their definitions, every cell
   assigned to a wire `u` holds `w_u`.

### Auxiliary wires

Auxiliary wires keep each equation to at most `width` distinct wires. They are new
wires numbered `N, N + 1, …` in order. Auxiliary wire `h` is defined by
`h = Σ kᵢ · wᵢ`, with:

- at least two terms;
- first coefficient 1;
- every `wᵢ` a nonconstant wire numbered below `h`.

Its definition is placed as the equation `0 · 0 = Σ kᵢ · wᵢ - h`.

Each source equation is rewritten into a *rewritten equation* that may use
auxiliary wires. The checker replaces every auxiliary wire by its definition, down to source
wires, and requires `A · B - C` of the result to be the same polynomial as
`Aᵢ · Bᵢ - Cᵢ` of the source equation.

### Placement

Every auxiliary definition and every rewritten equation is placed exactly once:
on one row, with each of its wires in a different column of that row. A cell
holds at most one wire, never `w₀`. A wire may occupy several cells. A cell
with no wire is *unused*.

### Gate types, gates and selectors

- A **gate type** is an equation with its wires replaced by positions `0, 1, …`
  (position `-1` is the constant `w₀`). Gate types are the certificate's `gateTypes`.
- A **gate** is a gate type together with one distinct column per position. A
  placement uses a gate on its row; putting its wires into the gate type must give
  the same `A · B - C` polynomial as its equation.
- Each gate has a **selector** `E{i}`: a fixed column that is 1 exactly on the
  gate's rows. The compiler makes one selector per distinct set of rows, so
  gates used on the same rows share it.
- For each gate, the PILout has the constraint `E · (A · B - C)`, with gate type
  position `p` read from the gate's column `p`.

The checker requires that the placements are exactly the pairs (gate, row)
where the gate is active, that no two gates are equal, that every selector is
used by a gate, and that every gate type is used by a placement.

### Public values

Public value `j` is bound to one cell, which then holds wire `j + 1`. The
checker accepts any cell; the compiler uses the first cell it gave the wire,
and gives a public wire that appears in no equation a cell of its own.

Let `q` be the number of distinct rows that hold public cells. Number those
rows `0 … q - 1` in increasing order and let `s = ⌈√q⌉`. The fixed columns are:

- `PH{h}` for `h = 0 … ⌈q / s⌉ - 1`: 1 on the public rows numbered
  `h · s … h · s + s - 1`;
- `PL{l}` for `l = 0 … s - 1`: 1 on the public rows whose number is `l`
  modulo `s`;
- `PA{c}` for each column `c` holding public cells: 1 on those rows.

So `PH{h} · PL{l}` is 1 on exactly one row, public row `h · s + l`, using about
`2√q` columns instead of `q`. For each column `c` holding public cells, the
constraint is

```text
PA{c} · a[c] - Σ PH{h} · PL{l} · public[j]
```

summed over the public values `j` bound in column `c`, with `h · s + l` the
number of the row of `j`.

### Copy constraints

If some wire occupies several cells (`requiresPermutation`), the
[permutation argument](#permutation-argument) forces all its cells to hold the same value.
Otherwise the PILout has no permutation argument, and stage two holds only
`stage2_zero`, constrained to zero as in the reference relation.

## Permutation argument

The permutation argument enforces the copy constraints: each cell gets a
distinct label, a permutation of the labels links the cells of each wire, and
a product identity over random challenges holds only if linked cells hold equal
values.

**Labels.** Cell `(r, c)` has the label `ω^r · COSET^c`, where
`ω = 7^((P - 1) / n)` generates the row domain and `COSET` is the constant in
`src/field.rs`. The compiler and the checker both check that all labels are
distinct. The fixed column `ID` holds `ω^r`.

**Permutation.** List the cells of each wire in row-major order. Each cell's
permuted label is the label of the previous cell of the same wire; the first
cell takes the label of the last. An unused cell, or the only cell of a wire,
keeps its own label.

**Permutation columns.** The columns holding at least one cell of a wire with several
cells, in increasing order; `p` is their number. In every other column each
cell keeps its own label, so its numerator and denominator factors below are
equal, and leaving the column out does not change the products. For each permutation
column `c`, the fixed column `S{c}` holds the permuted labels. The checker
derives the permutation columns from the layout and rebuilds `ID`, every `S{c}` and
`FIRST`.

**Factors**, for each row and permutation column `c`:

```text
num_c = a[c] + α · S{c} + γ
den_c = a[c] + α · COSET^c · ID + γ
```

**Running product.** The stage-two column `z` (`permutation_product`) holds, on row
`r`, the product of `num_c / den_c` over all permutation columns and rows `0 … r`.
The constraints are:

```text
z[r] · ∏ den_c[r] = z[r - 1] · ∏ num_c[r]     (row 0 reads row n - 1)
LAST · (z - 1) = 0
```

`LAST` is `FIRST` read at the next row, so it is 1 only on the last row.
[Lemma 1](#packed-relation) shows that, when every denominator factor is
nonzero, these constraints hold exactly when `∏ num = ∏ den` over all cells of
the permutation columns.

**Degree reduction.** Every constraint has degree at most 3 (`MAX_DEGREE` in
`src/contract.rs`), which a STARK prover with a blowup factor of 2 handles
without adding columns of its own. The recurrence above has degree `p + 1`, so
it fits when `p ≤ 2`. Otherwise the compiler adds `⌊(p - 1) / 2⌋` ratio
columns (`permutation_ratios`), numbered from 1. With factors indexed `0 … p - 1` in
permutation-column order:

```text
ratio_1 · den_0 · den_1          = num_0 · num_1 · num_2
ratio_i · den_{2i-2} · den_{2i-1} = ratio_{i-1} · num_{2i-1} · num_{2i}    (i ≥ 2)
```

The last ratio and the remaining factors (at most one numerator and two
denominator factors) replace the full products in the recurrence. For example,
`p = 4` gives one ratio and `z · den_2 · den_3 = z[r - 1] · ratio_1 · num_3`.

**Exact form.** The checker computes `p`, the ratio count and the stage-two
width itself, rejects references to stage-two columns beyond that width, and
compares the ratio, recurrence and closure constraints with its own
reconstruction operation by operation. Products are multiplied left to right
starting from the constant 1 (or from the previous ratio), each factor is
written `(a[c] + α · (COSET^c · ID)) + γ` or `(a[c] + α · S{c}) + γ`, and each
ratio or recurrence equation `x = y` becomes the constraint `x - y`.

## Hints

Hints tell the backend how to compute the stage-two columns. The checker
requires exactly these hints, in this order, each with exactly these fields:

| Hint | Computes | Fields |
| --- | --- | --- |
| `im_col` | each ratio column, in order | `reference`: the column; `numerator`: the right side of its constraint; `denominator`: the product of denominator factors on its left side |
| `gprod_col` | `z` | `reference`: `z + 0`; `numerator_air`, `denominator_air`: the products left in the recurrence; `numerator_direct`, `denominator_direct`, `result`: 1 |
| `gsum_col` | `stage2_zero` (only without a permutation argument) | `reference`: `stage2_zero + 0`; `numerator_air`: `α - α`; `denominator_air`: 1; `numerator_direct`: 0; `denominator_direct`: 1 |

The `gsum_col` numerator `α - α` is zero, but written as an expression over a
stage-two challenge.

## Artifact conventions

- `Air.numRows` holds the row count `n`, not `log2(n)` as a comment in the
  vendored schema says. Both checkers require this, and the backend exercised by
  `tools/backend-check` reads it the same way.
- The constants that define permutation labels (`7` and `COSET`) are fixed by a unit
  test.
- PILout bytes must be the canonical protobuf encoding of the decoded message,
  so unknown fields are rejected. Field elements are big-endian bytes with no
  leading zeros, below `P`.
- The PILout (`kernel`) has one AIR group and one AIR, both named `Kernel`, and
  nothing the relation does not use: no proof values, AIR values, periodic
  columns, custom commits, global constraints or negation.
- Every constraint applies to every row and carries a fixed label in
  `debug_line`, which both checkers require.
- Constraints, fixed columns and hints must appear in the order below. An
  artifact that encodes the same relation in a different order is rejected.

| Form | Constraints (label) | Fixed columns |
| --- | --- | --- |
| reference | `stage-two zero`; `R1CS {i}` per equation; `public {j}` per public value | `FIRST`, only with public values |
| packed | `stage-two zero` only without a permutation argument; `packed gate` per gate; `packed public` per `PA` column; with a permutation argument, `permutation ratio` per ratio, then `permutation recurrence` and `permutation closure` | with a permutation argument, `ID`, `S{c}` per permutation column in increasing order and `FIRST` (so `FIRST` is fixed column `p + 1`); then `E{i}` in selector order; `PH{h}`; `PL{l}`; `PA{c}` in the order of the first public value bound in each column |

The packed checker evaluates expressions as polynomials only up to the last
gate or public constraint, so permutation-argument expressions must come after those.

## Certificates

Field names are in camelCase, unknown fields are rejected, and every field is
checked. Both kinds contain:

- `version`: the certificate format version, always 1;
- `relation`: the certified relation; the CLI chooses the checker from it;
- `sourceSha256`, `piloutSha256`: SHA-256 of the R1CS and PILout bytes.

A **reference** certificate adds `rows`, `width` (`N - 1`), `publicWires`
(`1 … k` in order) and `wireColumns` (the column of each wire, `null` for
`w₀`).

A **packed** certificate adds:

- `rewrittenCircuit`: `originalWires` (`N`), `wires` (`N` plus the auxiliary wires), `publics`
  (`k`), `auxiliaries` (each `id` and `terms`), and the rewritten equations
  `equations`, one per source equation;
- `layout`: `minRows`, `width`, `rows`, `usedRows`, `arithmeticRows`, the
  gate types `gateTypes`, `placements`, `gates`, `selectors`, `publicBindings` and
  `requiresPermutation`.

The layout must satisfy:

- `width` in `3 … 1024`, `minRows` a power of two of at least 64, and
  `rows · width ≤ MAX_CELLS`;
- `arithmeticRows` = the last row holding a placement, plus 1;
- `usedRows` = the last row holding an equation or public cell, plus 1, and at
  least 1;
- `rows` = `usedRows` rounded up to a power of two, and at least `minRows`;
- `publicBindings[j]` binds public value `j` to wire `j + 1`;
- `requiresPermutation` is true exactly when some wire occupies several cells.

Settings that do not affect the relation (`auxiliaryTerms`, `reuseAuxiliaries`,
`foldConstants`, the placement strategy and `rowGroupWidth`) are not recorded.

## Correctness argument

This section argues on paper that soundness and completeness hold for every
artifact the checker accepts. It assumes that acceptance means:

- the certificate passes the checks above (for packed artifacts: the auxiliary wire and
  placement identities, one wire per cell, the public bindings, the active gate
  rows, `requiresPermutation` and distinct labels);
- the PILout contains exactly the constraints, fixed columns and hints above.

Whether the checker's code achieves this is a separate question (see
[Status of the properties](#status-of-the-properties)). Constraints are read as
a backend reads them: each must vanish on every row, row offsets wrap around,
and the challenges `α` and `γ` are drawn from a field `K` containing the
Goldilocks field after the stage-one trace is fixed.

### Reference relation

*Soundness.* Let a trace satisfy the AIR. Every equation constraint vanishes on
every row `r`, so `v(r, ·)` satisfies every equation. On row 0, `FIRST = 1`,
so the public constraints give `v(0, j + 1) = public[j]`. No challenge appears
in a constraint, so this holds with certainty.

*Completeness.* Given an assignment `w` that satisfies the circuit with the
values `public`, put `w` on every row (possible because each wire `i ≥ 1` has
its own column `m(i)`) and fill `stage2_zero` with 0, which is what its
`gsum_col` hint computes (a zero numerator over 1). Every equation constraint
holds on every row and the public constraints hold on row 0.

### Packed relation

In this section, *cells* are the cells of the permutation columns, and `M` is their
number. The checker requires `n · width ≤ MAX_CELLS`, so `M ≤ MAX_CELLS`.
Every cell of a wire with several cells is among them; the other cells need no
permutation argument, since they are unused or hold a wire with one cell.

Write `label(x)` for the label of cell `x` and `prev(x)` for the previous cell
of the same wire, so `S` holds `label(prev(x))` at cell `x`. The map `prev` is
a permutation of the cells whose cycles are exactly the sets of cells of each
wire; an unused cell, or the only cell of a wire, is its own cycle.

**Lemma 1 (permutation argument).** Fix the stage-one values `a(x)`, and suppose
every denominator factor `a(x) + α · label(x) + γ` is nonzero.

1. If stage-two values satisfy the ratio, recurrence and `LAST` constraints,
   then `∏ num = ∏ den` over all cells.
2. If `a` is constant on the cells of every wire, the columns computed by the
   hints satisfy those constraints.

*Proof of (1).* Each ratio constraint has the form `ratio · d = e`, with `d` a
product of denominator factors, so `ratio = e / d`. By induction over the
ratios, the recurrence on row `r` reads `z[r] = z[r - 1] · N[r] / D[r]`, where
`N[r]` and `D[r]` are the products of the numerator and denominator factors of
row `r`. Chaining these equations over all rows, cyclically, gives
`z[n - 1] = z[n - 1] · ∏ N / ∏ D`, and `LAST` gives `z[n - 1] = 1`.

*Proof of (2).* The hints compute the same ratios and the running product from
row 0: `z[0] = N[0] / D[0]` and `z[r] = z[r - 1] · N[r] / D[r]` for `r ≥ 1`, so
`z[n - 1] = ∏ N / ∏ D`. Since `prev` is a permutation and
`a(prev(x)) = a(x)`, the numerator factors `a(x) + α · label(prev(x)) + γ` are
a reordering of the denominator factors, so `z[n - 1] = 1`. Hence `LAST` holds,
and so does the recurrence on row 0, which reads `z[n - 1] = 1`.

**Lemma 2 (permutation failure probability).** Fix `a`, then draw `α` and `γ`
uniformly from `K`. Except with probability at most `2M / |K|`, every
denominator factor is nonzero, and `∏ num = ∏ den` holds only if the cells of
each wire hold equal values.

*Proof.* As a polynomial in `α` and `γ`, each denominator factor has the form
`γ + …`, so it is nonzero and vanishes with probability at most `1 / |K|`;
there are `M` of them. Next, `F = ∏ num - ∏ den` has total degree at most `M`.

- If `F` is the zero polynomial, unique factorization in `K[α, γ]` makes the
  multisets of pairs `(a(x), label(prev(x)))` and `(a(x), label(x))` equal.
  The labels are distinct, so `(a(x), label(prev(x)))` can only equal
  `(a(prev(x)), label(prev(x)))`, which gives `a(x) = a(prev(x))` for every
  cell: values are constant on every cycle of `prev`, that is, on the cells of
  every wire.
- Otherwise, by the Schwartz–Zippel lemma, `F` vanishes with probability at
  most `M / |K|`.

**Lemma 3 (public constraints).** On every row `r` and column `c`, the sum
`Σ PH · PL · public[j]` is `public[j]` if row `r` holds the public cell of
value `j` in column `c`, and 0 otherwise.

*Proof.* `PH{h} · PL{l}` is 1 only on public row `h · s + l`. Two public values
bound in the same column are different wires, so they are on different rows.
Hence the constraint for column `c` requires `a[c] = public[j]` on each public
cell, and nothing on other rows, where `PA{c}` and every product `PH · PL` in
the sum vanish.

*Soundness.* Let a trace satisfy the AIR for challenges that avoid the
exceptions of Lemma 2.

1. Every wire's cells hold one value: by Lemmas 1 and 2, or trivially without a
   permutation argument, since each wire then has at most one cell. Let `w_u` be that
   value for wire `u`, `w₀ = 1`, and `w_u = 0` for a wire without cells.
2. On every row where a gate is active, its selector is 1, so the equation
   placed there holds for `w`.
3. Every auxiliary definition is placed, so each auxiliary wire equals its linear
   combination of earlier wires.
4. Every rewritten equation is placed, and its `A · B - C`, with each auxiliary wire
   replaced by its definition, is the same polynomial as `Aᵢ · Bᵢ - Cᵢ` of its
   source equation; so the source equation holds for `w`. This is a polynomial
   identity, so it holds whatever value a wire without cells takes, in
   particular 0.
5. By Lemma 3, `w_{j+1} = public[j]`.

*Completeness.* Given an assignment `w` that satisfies the circuit with the
values `public`, compute each auxiliary wire from its definition, in order, put each
wire's value in all of its cells, and put 0 in unused cells. Each placed
equation is an auxiliary definition or a rewritten equation, and both hold, so
every gate constraint holds on its active rows and vanishes elsewhere. By
Lemma 3, the public constraints hold. If every denominator factor is nonzero,
Lemma 1 (2) gives stage two; this fails with probability at most `M / |K|`.
Without a permutation argument, `stage2_zero` is 0 as in the reference relation.

**Bound.** With `M ≤ MAX_CELLS < 2^22` and `|K| ≥ P > 2^63`, the bound
`2M / |K|` is below `2^-40` for one draw of the challenges. It assumes the
challenges are drawn after a fixed stage-one trace; under Fiat–Shamir a prover
can try many traces, and this argument does not cover that.

## Status of the properties

- **Checker correctness:** the checker accepts only artifacts whose PILout
  contains exactly the constraints, columns and hints described above. This is
  not proved; rejection tests and mutation tests exercise it. In particular,
  the polynomial comparison is an ordinary Rust implementation of polynomial
  equality over the field.
- **Soundness and completeness:** argued on paper above, assuming checker
  correctness. The argument has not been checked by a proof assistant.
- **Binding to bytes:** the properties apply to the canonical decoded PILout
  bytes. The parsers, the hash and the serialization must either be covered or
  be listed as trusted.

Soundness and completeness are also tested by property tests over random
satisfiable circuits and random layouts. A test-only evaluator reads only the
PILout, fills stage two from the hints and evaluates every constraint on every
row for random challenges. The tests require that:

- the honest trace of every reference and packed artifact satisfies its AIR;
- whenever a changed trace (one cell, every cell of one wire, or one public
  value) still satisfies it, the assignment read back through the certificate
  satisfies the circuit with the same public values.

[Testing](docs/TESTING.md) describes them.

## Out of scope

Preserving an R1CS relation says nothing about what the R1CS computes. Machine
semantics, execution across rows, memory ordering, buses and lookups would need
their own specifications. The soundness of the STARK, the security of
Fiat–Shamir and zero knowledge do not follow from the compiler preserving the
relation either.

A future extension could remove intermediate wires whose values can be
recomputed from their defining equations. That would need its own argument for
both soundness and completeness; removing an equation without one would break
the relation. Copies, selectors, range conditions and public order would still
have to be preserved.
