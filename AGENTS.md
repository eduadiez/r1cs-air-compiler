# Repository instructions

This repository is a Rust compiler from Goldilocks R1CS to PILout with
independent checkers. Focus on clarity and maintainability; do not add
optimizations unless the owner asks for them.

The checkers must not use the compiler's modules (a test enforces this).
Preserve the source relations, public order, the permutation argument and the
supported hints; changes to these contracts need explicit tests and an update
to SPEC.md.

Keep the repository generic and self-contained: no project-specific history,
external workspace paths or claims based on unavailable benchmarks. Keep the
vendored schema and circomlib files, their licenses and docs/provenance.json
intact.

Before finishing a change, run cargo fmt --check, cargo clippy --locked
--all-targets -- -D warnings and cargo test --locked. A cleanup-only change must
not change any hash in fixtures/*/expected.json; docs/TESTING.md explains what
to update when an output is meant to change.

Do not commit, push, add remotes or publish unless asked.
