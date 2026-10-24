# Testing

`cargo test --locked` runs every test needed for a change:

- unit tests of the field arithmetic and expression simplification, inside
  `src/field.rs` and `src/dag.rs`;
- internal tests in `src/tests/`, one file per component (`parser.rs`,
  `reference.rs`, `placement.rs`, `packed.rs`). Most change one field of a
  source, PILout or certificate and require the check to fail with a specific
  message, after confirming that the unchanged artifact is accepted. `mod.rs`
  also checks that no checker uses a compiler module;
- public API tests (`tests/api.rs`);
- CLI tests (`tests/cli.rs`), one per fixture, that run `compile`, `check`,
  `plan` and `witness` from outside the repository and compare every output
  (PILout, certificate, plan, cell map, trace and public values) with the
  hashes in the fixture's `expected.json`. Other CLI tests cover the reports,
  error messages and argument forms, and check the hashes in
  `docs/provenance.json`.

The fixture R1CS and WTNS files are committed, so tests need no Circom.
`fixtures/generate.sh --check` regenerates them and checks that they are
identical byte for byte (requires circom 2.2.3 and Node.js).

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
commit. For the reference and packed artifact of every fixture it runs the
prover's setup, verifies every constraint on the witness, and generates and
verifies a STARK proof. It needs network access and a C++ toolchain, so it is
not part of `cargo test` or CI.

