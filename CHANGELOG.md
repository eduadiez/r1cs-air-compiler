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
  expression simplification, and packed relation (`r1cs-packed-static-copy`)
  with a permutation argument.
- Independent checkers for both relations, run on every compiled artifact.
- Witness projection into a packed trace.
- `air` executable with the commands `compile`, `check`, `plan` and `witness`,
  and JSON reports. A failure report's `kind` is `usage`, `io`, `invalid` or
  `limit`. `certificate.json` and `plan.json` are compact JSON followed by a
  newline.
- Layout strategies `mixed`, `local`, `components` and `columns`. `columns`
  joins equations that share wires into row groups and stacks equal row groups in
  the same columns, which gives far fewer gate constraints and selectors; the
  optional `rowGroupWidth` setting bounds the size of its row groups.
- Every constraint has degree at most 3, so a prover with a blowup factor of 2
  needs no intermediate columns of its own. Each permutation ratio takes at most two
  denominator factors, and two numerator factors besides the previous ratio
  (three for the first ratio).
- The permutation argument covers only the columns holding a cell of a wire with
  several cells. Other columns get no `S` column, no factors and no share of
  the ratio columns.
- Optional scripts in `tools/backend-check` that prove every fixture with
  pil2-proofman (`run.sh`), time the prover on one artifact (`prove-time.sh`)
  and estimate the fastest layout of a circuit without proving
  (`choose-layout.sh`).
- `Error::kind` tells an invalid input (`ErrorKind::Invalid`) from one that
  exceeds a size limit (`ErrorKind::Limit`).
- The types that only the library builds (artifacts, reports, `Witness`) and
  the setting enums (`Strategy`, `Reuse`) are `#[non_exhaustive]`, so later
  versions can add fields and variants.
