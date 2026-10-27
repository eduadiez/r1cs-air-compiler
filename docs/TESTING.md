# Testing

## What `cargo test` runs

`cargo test --locked` runs every test a change needs:

| Tests | Where | What they check |
| --- | --- | --- |
| Unit tests | `src/field.rs`, `src/dag.rs` | Field arithmetic and expression simplification |
| Internal tests | `src/tests/parser.rs`, `reference.rs`, `placement.rs`, `packed.rs`, `module.rs` | Most change one field of a source, PILout or certificate and require the check to fail with a specific message, after checking that the unchanged artifact is accepted |
| Independence | `src/tests/mod.rs` | No checker uses a compiler module |
| Property tests | `src/tests/properties.rs` | See [Property tests](#property-tests) |
| Public API | `tests/api.rs` | The library interface |
| CLI | `tests/cli.rs` | See below |

The CLI tests run the `air` executable from a temporary directory outside the
repository. For each fixture they run `compile`, `check`, `plan` and `witness`
and compare every output (PILout, certificate, plan, cell map, trace and public
values) with the hashes in the fixture's `expected.json`. Other CLI tests cover
the reports, error messages, argument forms and the hashes in
`docs/provenance.json`.

A fixture whose `expected.json` has no `reference` hash, such as `sha256`, is
too large for the reference layout: its test requires both reference modes to
fail with a size-limit error and compares only the packed outputs.

The `sha256` tests are slow in a debug build, so they are ignored by default.
Run them with:

```sh
cargo test --locked --release -- --ignored
```

CI (`.github/workflows/check.yml`) runs both commands, plus `cargo fmt`,
`cargo clippy`, `cargo doc`, `cargo package`, `shellcheck` on the scripts and
`cargo test` on the minimum Rust version (1.85.0).

The fixture R1CS and WTNS files are committed, so tests need no Circom.
`fixtures/generate.sh --check` (needs circom 2.2.3 and Node.js) regenerates
them and checks that they are identical byte for byte.

## Property tests

`src/tests/properties.rs` generates random circuits (4–24 wires, 1–12
equations, up to 3 public values, nonzero coefficients) and random packed
layout settings. It checks that:

- every reference, packed and module artifact compiled from them passes its
  checker, and each module is preserved both when its fragment is embedded in
  a host AIR at a random column offset and with the reference artifact of the
  same circuit as the host;
- every witness of a satisfiable circuit projects, with each source wire's
  value in its cells;
- a PILout with one changed constant, stage-one column reference or
  fixed-column value is rejected, unless the change left every constraint
  polynomial as it was (for example a column whose terms cancel in
  `A · B - C`);
- arbitrary bytes, and single changed bytes in valid sources, PILout files and
  certificates, never make the parsers or checkers (the composition check
  included) panic.

Completeness and soundness (as defined in `SPEC.md`) are tested with
`src/tests/air.rs`, an evaluator that reads only the PILout. Like a prover, it
fills the stage-two columns from the hints, then evaluates every constraint on
every row for random challenges. On random satisfiable circuits (2–6 inputs,
1–10 equations, up to 2 public inputs) the tests require that:

- **completeness:** the honest trace of every reference and packed artifact
  satisfies the AIR;
- **soundness:** after changing one cell, every cell of one wire, or one public
  value, a trace that still satisfies the AIR encodes a satisfying assignment:
  every cell of a wire holds the same value, the values read back through the
  certificate satisfy the circuit, and the public values match.

A case where a permutation denominator is zero for the drawn challenges is skipped,
as the specification allows. `src/tests/module.rs` builds host AIRs the way an integration would: the
fragment's expressions are copied into a larger AIR, with the module's columns
moved, extra columns, a stage-two column and an adapter constraint on an
exported wire. It requires such hosts to be preserved (also with the
expressions reversed and wrapped), and a missing equation, a changed
coefficient, an equation on the first row only, a module column read at
another row, a wrong or aliased binding and a cancelling host operand to be
rejected.

A unit test checks that the evaluator rejects
each kind of change on the xor fixture, so these properties cannot pass by
accepting every trace.

`cargo test` runs 64 cases per property (256 for the panic properties). Set
`PROPTEST_CASES` to run more:

```sh
PROPTEST_CASES=100000 cargo test --release --lib properties
```

A failing case is shrunk to a minimal one and saved in
`proptest-regressions/tests/properties.txt`. Commit that file so the case runs
first from then on.

## Fuzzing

`fuzz/` holds coverage-guided fuzz targets for
[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz), which needs a nightly
toolchain:

| Target | Input | Must never panic |
| --- | --- | --- |
| `r1cs` | any bytes as an R1CS file | parsing, compiling and checking |
| `reference_pilout` | any bytes as the xor fixture's reference PILout (the certificate's PILout hash is updated to match) | the reference checker |
| `packed_pilout` | the same for the packed PILout | the packed checker |
| `packed_certificate` | any bytes as the packed certificate JSON | the packed checker and witness projection |

```sh
fuzz/seed.sh                                       # write valid starting inputs
cargo +nightly fuzz run packed_pilout -- -max_total_time=600
```

A crash is saved under `fuzz/artifacts/`; add it as a test before fixing it.
The `Fuzz` workflow (`.github/workflows/fuzz.yml`) runs each target for five
minutes every Monday and on demand.

## Changing expected hashes

A change that only cleans up code must not change any hash in
`fixtures/*/expected.json`. When an output is meant to change:

1. Run `cargo test --locked` and, for `sha256`,
   `cargo test --locked --release -- --ignored`. A fixture's test stops at
   its first changed output with `NAME: KEY changed` and shows the new hash
   as `left`.
2. Put that hash in the fixture's `expected.json`, and repeat until the
   fixture tests pass.
3. Run the tests again: `provenance_records_every_file_hash` now fails for each
   changed fixture file (at least `expected.json`) and shows its new hash as
   `left`. Update that file's `sha256` in `docs/provenance.json`.
4. Explain in the change why the output changed.

## Mutation testing

Accepting an artifact rests on the checkers, so the tests are measured by how
many deliberate bugs in the checkers they catch. [cargo-mutants](https://mutants.rs)
changes the code one edit at a time (each change is a "mutant") and runs the
tests; a mutant "survives" if all tests still pass:

```sh
cargo mutants -f src/checker.rs -f src/packed_check.rs -f src/placement_check.rs \
    -f src/module_check.rs -f src/pilout_check.rs -f src/polynomial.rs -f src/r1cs.rs \
    -f src/field.rs
```

Every mutant should be caught except these known survivors, which do not
change the behavior:

| Location | Mutation | Why it survives |
| --- | --- | --- |
| `pilout_check.rs`, `scalar`, and `module_check.rs`, `host_scalar` | `\|` → `^` when appending a byte | Same result: the shifted value has a zero low byte. |
| `packed_check.rs`, `add_permutation_columns` | `(P - 1) / n` → `(P + 1) / n` or `P / n` | Same result for every power of two `n ≥ 64`. |
| `r1cs.rs`, `linear_combination` and the equation count in `read` | looser bound on the size check before allocation | Same result: truncated input is still rejected while reading. |

Any other survivor means some check is no longer tested: add a test that
breaks only that condition. cargo-mutants changes operators and return values
but never deletes a whole check, so it cannot prove on its own that every
check is tested. Each new check therefore needs a test that fails with that
check's own message.

## Backend check

[`tools/backend-check`](../tools/backend-check/README.md) is an optional
script, run by hand, that tests the artifacts with the pil2-proofman prover at
a fixed commit. For each fixture's reference and packed artifacts (only the
packed one when the fixture has no reference outputs) it runs the prover's
setup, verifies every constraint on the witness, and generates and verifies a
STARK proof. It needs network access and a C++ toolchain, so it is not part of
`cargo test` or CI.

## SMT check

[`tools/smt-check`](../tools/smt-check/README.md) is an optional program, run
by hand, that proves for one packed artifact that its stage-one constraints,
with the copy cycles of its `ID` and `S` columns held constant, imply every
equation and public binding of its R1CS. It shares no code with the compiler
or the checkers: it reads both files itself, proves most goals by linear
elimination, and sends the rest to an SMT solver with finite-field support
(cvc5). It does not cover the permutation argument itself or completeness.
The packed artifacts of `xor`, `adder`, `range` and `mux`, with their own
layouts and with `--packed`, are proved by linear elimination alone, each in
under a second.
