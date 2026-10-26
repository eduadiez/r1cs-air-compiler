# r1cs-air-compiler

A Rust compiler from R1CS (rank-1 constraint system) circuits over the
Goldilocks field to PILout, the AIR (algebraic intermediate representation)
format of the pil2 tools (pil2-compiler and the pil2-proofman prover). Every
compiled artifact is checked by an independent checker, and a Circom witness
file (`.wtns`) can be turned into the matching trace. Building and running the
compiler needs neither Circom nor Node.js.

```text
R1CS ──→ compiler ──→ PILout + certificate
  │                        │
  └──────→ checker ←───────┘
WTNS ──→ witness projection ──→ trace + public values
```

## Build and test

Install Rust and `protoc` (on Debian or Ubuntu: `protobuf-compiler`). The
protobuf schema and its licenses are in `vendor/pilout`, and Rust dependency
versions are locked. Requires Rust 1.85 or later; CI tests both 1.85 and the
current stable release.

```sh
cargo build --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Add `--offline` once the locked dependencies are in the local Cargo cache.

## Example

Run from the repository root. Each output directory must not exist yet.

```sh
mkdir -p out
cargo run --bin air -- --help
cargo run --bin air -- compile fixtures/xor/kernel.r1cs out/reference --rows 64
cargo run --bin air -- check fixtures/xor/kernel.r1cs out/reference/kernel.pilout out/reference/certificate.json

cargo run --bin air -- compile fixtures/xor/kernel.r1cs out/packed --layout fixtures/xor/layout.json
cargo run --bin air -- check fixtures/xor/kernel.r1cs out/packed/kernel.pilout out/packed/certificate.json
cargo run --bin air -- plan fixtures/xor/kernel.r1cs out/plan --layout fixtures/xor/layout.json
cargo run --bin air -- witness fixtures/xor/kernel.r1cs out/packed fixtures/xor/input.wtns out/witness
```

[The CLI reference](docs/CLI.md) describes every argument and the JSON reports.

## Choosing a layout

A packed layout is described by a JSON file. For proving speed, start from the
`columns` strategy, which `fixtures/sha256/layout.json` uses:

```json
{"options":{"width":64,"chunk":32,"reuse":"available","fold":true},"strategy":"columns","minRows":1024}
```

`columns` joins equations that share wires into bundles and places equal
bundles in runs that share their constraints. In
[the measurements](docs/SCALE.md#prover-time) it proved nine circomlib circuits
21–88% faster than the best row-filling layout. The trace has exactly `minRows`
rows, so `minRows` sets how wide it gets, and the best value depends on the
circuit; so may the optional `bundleWidth`, the most wires of a bundle.

With the pil2-proofman build of the [backend check](tools/backend-check/README.md),
two scripts help choose:

```sh
# Estimate the fastest layouts without proving, best first.
tools/backend-check/choose-layout.sh WORK_DIR circuit.r1cs
# Time the prover on one layout, phase by phase.
tools/backend-check/prove-time.sh WORK_DIR circuit.r1cs input.wtns layout.json
```

For small proofs, the prover's proof-of-work setting can matter more than the
layout; [Scale](docs/SCALE.md#proof-of-work-bits) explains it.

## Fixtures

Each directory in `fixtures/` holds a Circom source, its R1CS and WTNS files, a
layout configuration and the expected SHA-256 hashes of the generated artifacts:

- `xor`: `305419896 XOR 4275878552 = 3974687968` on 32-bit words;
- `range`: checks `lo <= x <= hi` for 32-bit values with public bounds; its
  packed layout needs no copy argument;
- `mux`: selects a row of a public table;
- `adder`: adds three 32-bit words and compares the sum with a public value;
- `sha256`: circomlib's SHA-256 of a 256-bit message (29 380 constraints), with
  the hash bits as public outputs. It is too large for the reference layout, so
  it has only packed outputs.

`range`, `mux`, `adder` and `sha256` use circomlib templates. The CLI test runs
the `air` executable from outside the repository and compares the reference,
optimized and packed PILout files, the trace and the public values of every
fixture with the expected hashes. The `sha256` test is slow in a debug build and
is ignored by default; run it with `cargo test --release -- --ignored`.
[Scale](docs/SCALE.md) gives its sizes and run times.

The R1CS and WTNS files are committed, so tests need no Circom.
`fixtures/generate.sh --check` (circom 2.2.3 and Node.js) regenerates them and
checks that they are identical byte for byte; `fixtures/generate.sh NAME`
regenerates one fixture.

## Rust API

```rust
use r1cs_air_compiler::{compile_reference, Result};

fn main() -> Result<()> {
    let source = std::fs::read("fixtures/xor/kernel.r1cs")?;
    let artifact = compile_reference(&source, 64)?;
    println!("{} PILout bytes", artifact.pilout.len());
    Ok(())
}
```

Each compile function runs the checker before it returns. Try
`cargo run --example compile`, and see [the API reference](docs/API.md) or
`cargo doc --no-deps`.

## What is supported

- R1CS version 1 and WTNS version 2 files over Goldilocks, with 8-byte field
  elements. Custom gates and other fields are rejected.
- **Reference layout:** one column per nonconstant wire, the whole circuit
  constrained on every row, and the public values bound on row 0. Every
  constraint of the circuit becomes a constraint of the AIR, so large circuits
  exceed the size limits of the checker and need a packed layout; the `sha256`
  fixture (29 380 constraints) is one of them.
- **`--optimize`:** the reference layout with expressions simplified by
  identities that hold for every field value, and repeated subexpressions reused.
- **`--layout`:** a packed layout. Long linear combinations are split using
  helper wires, equations are grouped by shape and placed in rows (strategies
  `mixed`, `local` and `components`) or in runs of bundles (`columns`), and the
  checker verifies that every source equation and the public order are preserved.
- **Copy argument:** a packed layout that puts a wire in more than one cell gets
  a permutation argument forcing those cells to be equal, over the columns that
  hold such cells. It is left out only when every wire occupies at most one
  cell.
- **Independent checks:** the checkers compare the actual PILout constraints,
  fixed columns, symbols, hints and certificate fields with what the source
  requires. They never call the compiler's conversion, packing or PILout
  generation code.

This is an experimental compiler, not a virtual machine or a prover. It does not
implement buses or lookups. The checks are carried out by a program, not proved
formally, and some components are trusted rather than checked: the parser, the
field arithmetic, the protobuf schema and decoder, SHA-256 and the proving
backend (see the specification). Inputs are assumed not to be adversarial: there
is no limit on total time or memory, so run the checker on untrusted artifacts
only under external resource limits. [The specification](SPEC.md) argues on
paper that every accepted artifact is sound and complete.

## More documentation

- [Architecture](docs/ARCHITECTURE.md): modules and how they depend on each other.
- [Testing](docs/TESTING.md): the test suite, mutation testing and the optional
  check against a real prover.
- [Scale](docs/SCALE.md): sizes, run times and prover times of the strategies
  on the `sha256` fixture and larger circomlib circuits.
- [Changelog](CHANGELOG.md): changes and the versioning policy.
- [Provenance](docs/provenance.json): hashes of the schema, the vendored
  circomlib files and the fixtures.
- [Third-party notices](THIRD_PARTY.md): origin and licenses of the schema and
  the circomlib templates.

## License

The compiler is licensed under either the [Apache License, Version 2.0](LICENSE-APACHE)
or the [MIT license](LICENSE-MIT), at your option. Third-party files keep their
own licenses: the PILout schema is `MIT OR Apache-2.0`, and the circomlib
templates used by the fixtures are GPL-3.0-or-later. See [THIRD_PARTY.md](THIRD_PARTY.md).
