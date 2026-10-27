# Changelog

Notable changes to the library, the `air` executable and the artifact formats.

Versions follow [Semantic Versioning](https://semver.org). Before 1.0, a minor
version (0.x) may break the Rust API, the CLI or the artifact formats; a patch
version (0.x.y) does not. A change to the source relations, the public order,
the permutation argument, the hints, the certificate format or the bytes of any
output is breaking: it raises the minor version, is listed here, and updates
`SPEC.md` and the hashes in `fixtures/*/expected.json`.

## Unreleased (0.1.0)

First version.

- Reference relation (`r1cs-per-row-publics-at-row-zero`), with optional
  expression simplification; `air compile` builds it with 64 rows unless
  `--rows` is given, and packed relation (`r1cs-packed-static-copy`)
  with a permutation argument.
- Independent checkers for both relations, run on every compiled artifact.
- Witness projection into a packed trace.
- Modules (`r1cs-module-per-row`): `compile_module` and `air module` compile
  a circuit, optionally with optimized expressions, to a fragment for a host
  AIR, with one column per wire, every equation as one every-row constraint,
  exported wire ids and no public values. `check_composition` and
  `air check-composition` check that a host's final PILout enforces every
  equation on every row over the columns in a binding.
- `air` executable with the commands `compile`, `check`, `plan` and `witness`,
  and JSON reports. A failure report's `kind` is `usage`, `io`, `invalid` or
  `limit`. `certificate.json` and `plan.json` are compact JSON followed by a
  newline.
- Packed layouts join equations that share wires into row groups and stack
  equal row groups in the same columns, which gives few gate constraints and
  selectors; the optional `rowGroupWidth` setting bounds the size of the row
  groups.
- Every constraint has degree at most 3, so a prover with a blowup factor of 2
  needs no intermediate columns of its own. Each permutation ratio takes at most two
  denominator factors, and two numerator factors besides the previous ratio
  (three for the first ratio).
- The permutation argument covers only the columns holding a cell of a wire with
  several cells. Other columns get no `S` column, no factors and no share of
  the ratio columns.
- Packed layout settings are all optional (`LayoutConfig::default()`, or
  `air compile --packed`): a missing equation width, row count or row group
  width is chosen by the estimated proving time of each candidate with
  pil2-proofman, and the other settings take their defaults.
- Optional scripts in `tools/backend-check` that prove every fixture with
  pil2-proofman (`run.sh`) and time the prover on one artifact
  (`prove-time.sh`).
- An optional program, `tools/explain/explain.py`, that writes an HTML page
  showing how an artifact places the circuit: the trace by gate, and for each
  equation its auxiliary wires and placement.
- An optional program, `tools/smt-check/check.py`, independent of the crate,
  that proves for one packed artifact that its stage-one constraints and copy
  cycles imply every equation and public binding of its R1CS, by linear
  elimination and an SMT solver.
- `Error::kind` tells an invalid input (`ErrorKind::Invalid`) from one that
  exceeds a size limit (`ErrorKind::Limit`).
- The types that only the library builds (artifacts, reports, `Witness`) and
  the setting enum `Reuse` is `#[non_exhaustive]`, so later
  versions can add fields and variants.
