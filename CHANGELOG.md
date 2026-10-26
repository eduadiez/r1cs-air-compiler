# Changelog

All notable changes to the library, the `air` executable and the artifact
formats are recorded here.

Versions follow [Semantic Versioning](https://semver.org). Before 1.0, a minor
version (0.x) may break the Rust API, the CLI or the artifact formats, and a
patch version (0.x.y) does not. Any change to the source relations, the public
order, the copy argument, the hints, the certificate format or an output's
bytes is a breaking change: it raises the minor version, is listed here, and
updates `SPEC.md` and the hashes in `fixtures/*/expected.json`.

## Unreleased (0.1.0)

First version.

- Reference relation (`r1cs-per-row-publics-at-row-zero`), with optional
  expression simplification, and packed relation (`r1cs-packed-static-copy`)
  with a copy argument.
- Independent checkers for both relations, run on every compiled artifact.
- Witness projection into a packed trace.
- `air` executable with `compile`, `check`, `plan` and `witness` commands and
  JSON reports. A failure report's `kind` is `usage`, `io`, `invalid` or
  `limit`. `certificate.json` and `plan.json` are compact JSON followed by a
  newline.
- Layout strategies `mixed`, `local`, `components` and `columns`; `columns`
  puts each shape in its own band of columns, which gives far fewer slot
  constraints and selectors.
- Copy ratios have degree 3 (two numerator and two denominator factors each),
  so a prover with a blowup factor of 2 needs no intermediate columns of its
  own; this changed the packed PILout and certificate of every fixture with a
  copy argument.
- `Error::kind` tells an invalid input (`ErrorKind::Invalid`) from one that
  exceeds a size limit (`ErrorKind::Limit`).
- Types that only the library builds (artifacts, reports, `Witness`) and the
  setting enums (`Strategy`, `Reuse`) are `#[non_exhaustive]`, so later versions
  can add fields and variants.
