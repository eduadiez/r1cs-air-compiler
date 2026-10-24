# Testing

`cargo test --locked` runs every test needed for a change:

- unit tests of the field arithmetic and expression simplification, inside
  `src/field.rs` and `src/dag.rs`;
- property tests in `src/tests/properties.rs` (see below);
- internal tests in `src/tests/`, one file per component (`parser.rs`,
  `reference.rs`, `placement.rs`, `packed.rs`). Most change one field of a
  source, PILout or certificate and require the check to fail with a specific
  message, after confirming that the unchanged artifact is accepted. `mod.rs`
  also checks that no checker uses a compiler module;
- public API tests (`tests/api.rs`);
- CLI tests (`tests/cli.rs`), one per fixture, that run `compile`, `check`,
  `plan` and `witness` from outside the repository and compare every output
  (PILout, certificate, plan, cell map, trace and public values) with the
  hashes in the fixture's `expected.json`. A fixture whose `expected.json` has
  no `reference` hash, such as `sha256`, is too large for the reference layout:
  its test requires both reference modes to fail with a size limit error and
  compares only the packed outputs. The `sha256` test is ignored by default
  because it is slow in a debug build; CI runs it with
  `cargo test --locked --release --test cli -- --ignored`. Other CLI tests cover the reports,
  error messages and argument forms, and check the hashes in
  `docs/provenance.json`.

The fixture R1CS and WTNS files are committed, so tests need no Circom.
`fixtures/generate.sh --check` regenerates them and checks that they are
identical byte for byte (requires circom 2.2.3 and Node.js).

## Property tests

`src/tests/properties.rs` generates random circuits (up to 24 wires, 12
equations and 3 public values, with nonzero coefficients) and random packed
layout settings, and checks that:

- every reference and packed artifact compiled from them passes its checker;
- every witness of a satisfiable circuit projects, with each source wire's
  value in its cells;
- after changing one constant, one stage-one column reference or one
  fixed-column value in a compiled PILout, the checker rejects it, unless the
  change left every constraint polynomial as it was (for example a column whose
  terms cancel in `A · B - C`);
- arbitrary bytes, and single changed bytes in valid sources, PILout files and
  certificates, never make the parsers or checkers panic.

`cargo test` runs 64 cases per property (256 for the panic properties). Set
`PROPTEST_CASES` to run more, for example
`PROPTEST_CASES=100000 cargo test --release --lib properties`. A failing case is
shrunk to a minimal one and saved in `proptest-regressions/tests/properties.txt`;
commit that file so the case is run first from then on.

## Fuzzing

`fuzz/` holds coverage-guided fuzz targets for [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz),
which needs a nightly toolchain:

| Target | Input | Must never panic |
| --- | --- | --- |
| `r1cs` | any bytes as an R1CS file | parsing, compiling and checking |
| `reference_pilout` | any bytes as the xor fixture's reference PILout, with the certificate rebound to them | the reference checker |
| `packed_pilout` | the same for the packed PILout | the packed checker |
| `packed_certificate` | any bytes as the packed certificate JSON | the packed checker and witness projection |

```sh
fuzz/seed.sh                                   # valid starting inputs
cargo +nightly fuzz run packed_pilout -- -max_total_time=600
```

A crash is saved under `fuzz/artifacts/`; add it as a test before fixing it. The
`Fuzz` workflow runs every target for five minutes each week and on demand.

## Changing expected hashes

A change that only cleans up code must not change any hash in
`fixtures/*/expected.json`. When an output is meant to change, regenerate the
fixture outputs, update the affected keys in `expected.json`, and then update
the `sha256` of every changed fixture file in `docs/provenance.json`, which
also records the hash of each `expected.json`. Explain in the change why the
output changed.

## Mutation testing

Accepting an artifact rests on the checkers, so the tests are measured by how
many deliberate changes to the checkers they detect, with
[cargo-mutants](https://mutants.rs). It changes the code one edit at a time
(each change is a "mutant") and runs the tests; a mutant "survives" if all
tests still pass:

```sh
cargo mutants -f src/checker.rs -f src/packed_check.rs -f src/placement_check.rs \
    -f src/pilout_check.rs -f src/polynomial.rs -f src/r1cs.rs -f src/field.rs
```

Every mutant should be detected except these known survivors:

| Location | Mutation | Why it survives |
| --- | --- | --- |
| `pilout_check.rs`, `scalar` | `\|` → `^` when appending a byte | Same result: the shifted value has a zero low byte. |
| `packed_check.rs`, `add_copy_columns` | `(P - 1) / n` → `(P + 1) / n` or `P / n` | Same result for every power of two `n ≥ 64`. |
| `r1cs.rs`, `lc` and constraint count | looser bound on the size check before allocation | Same result: truncated input is still rejected while reading. |

A survivor that is not in this table means some check is no longer tested: add a
test that breaks only that condition. cargo-mutants changes operators and
return values but never deletes a whole check, so it cannot show on its own
that every check has a test: each new check needs a test that fails with that
check's own message.

## Backend check

[`tools/backend-check`](../tools/backend-check/README.md) is an optional script,
run by hand, that tests the artifacts with the pil2-proofman prover at a fixed
commit. For the reference and packed artifact of every fixture (only the packed
artifact of a fixture without reference outputs) it runs the
prover's setup, verifies every constraint on the witness, and generates and
verifies a STARK proof. It needs network access and a C++ toolchain, so it is
not part of `cargo test` or CI.

