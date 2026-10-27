# r1cs-air-compiler

Compiles R1CS circuits (rank-1 constraint systems) over the Goldilocks field to
PILout, the AIR format (algebraic intermediate representation) read by the pil2
tools (pil2-compiler and the pil2-proofman prover). An independent checker
verifies every compiled artifact, and a Circom witness (`.wtns`) can be turned
into the matching trace. Neither Circom nor Node.js is needed to build or run
it.

| Command | Reads | Produces |
| --- | --- | --- |
| `compile` | R1CS; optionally a row count, `--packed` or a layout file | PILout and certificate, already checked |
| `check` | R1CS, PILout, certificate | Accepted, or an error |
| `plan` | R1CS, optionally a layout file | The packed placement only, checked, without PILout |
| `witness` | R1CS, packed artifact, WTNS | Trace and public values |
| `module` | R1CS, exported wire ids | A module fragment for a host AIR, and its certificate, already checked |
| `check-composition` | R1CS, host PILout, module certificate, binding | Whether the host AIR enforces every equation of the module |

A certificate is a JSON file that ties the PILout to its R1CS (by SHA-256) and
records how wires were placed in the trace; the checker needs it.

## Build and test

You need Rust 1.85 or later (CI tests 1.85 and the current stable release) and
`protoc` (on Debian or Ubuntu, the `protobuf-compiler` package). Dependency
versions are locked.

```sh
cargo build --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Add `--offline` once the locked dependencies are in the local Cargo cache.

## Example

`air` reads R1CS and WTNS files, not Circom source. Every fixture includes
both, so the Circom step below is optional: it shows how `fixtures/xor`'s files
are made from its `kernel.circom`, and reproduces them byte for byte. Run from
the repository root. `air` creates each output directory and fails if it
already exists, so run `rm -rf build out` before running the example again.

```sh
mkdir -p build out
cargo run --bin air -- --help

# Optional: the R1CS and a witness from Circom source, with either generator.
circom fixtures/xor/kernel.circom --r1cs --wasm --c --prime goldilocks --O2 -o build
node build/kernel_js/generate_witness.js build/kernel_js/kernel.wasm fixtures/xor/input.json build/input.wtns
# or: make -C build/kernel_cpp && build/kernel_cpp/kernel fixtures/xor/input.json build/input.wtns

# Reference layout: one column per wire, 64 rows unless --rows N says otherwise.
cargo run --bin air -- compile fixtures/xor/kernel.r1cs out/reference
cargo run --bin air -- check fixtures/xor/kernel.r1cs out/reference/kernel.pilout out/reference/certificate.json

# Packed layout, then the trace for a witness.
cargo run --bin air -- compile fixtures/xor/kernel.r1cs out/packed --packed
cargo run --bin air -- check fixtures/xor/kernel.r1cs out/packed/kernel.pilout out/packed/certificate.json
cargo run --bin air -- plan fixtures/xor/kernel.r1cs out/plan
cargo run --bin air -- witness fixtures/xor/kernel.r1cs out/packed fixtures/xor/input.wtns out/witness
```

Each command prints one JSON report. [The CLI reference](docs/CLI.md) describes
every argument, output file, layout setting and report field.

For your own circuit:

- `--prime goldilocks` is required: Circom's default field is BN254, and `air`
  rejects files over any other field.
- The Circom optimization level (`--O0`, `--O1`, `--O2`) changes the
  constraints, and so the PILout. The fixtures use `--O2`.
- Both witness generators write the same WTNS file. The WebAssembly one needs
  Node.js; the C++ one needs a C++ compiler, GMP and make, and is much faster
  on large circuits. Pass only `--wasm` or `--c` to write just one.
- Any other tool that writes R1CS version 1 and WTNS version 2 files over
  Goldilocks works as well.

## Choosing a layout

`--packed` lets the compiler choose the layout. It builds candidate layouts
and keeps the one with the lowest estimated proving time with pil2-proofman;
the `layout` field of the report shows the settings it used.

A layout file sets some or all of the settings and leaves the rest to the
compiler, for example `fixtures/sha256/layout.json`:

```json
{"width":64,"auxiliaryTerms":32,"reuseAuxiliaries":"available","foldConstants":true,"minRows":1024,"rowGroupWidth":64}
```

The compiler splits long equations so that none uses more than `width` wires,
groups equations that share wires into row groups of at most `rowGroupWidth`
wires, and stacks equal row groups in the same columns, so they share their
constraints. The trace has exactly `minRows` rows, so a smaller `minRows`
gives a wider trace. [The CLI reference](docs/CLI.md#layout-configuration)
lists every setting and its default, and [Scale](docs/SCALE.md#prover-time)
gives measurements.

To time the prover on a layout, phase by phase, with pil2-proofman built by
the [backend check](tools/backend-check/README.md):

```sh
tools/backend-check/prove-time.sh WORK_DIR circuit.r1cs input.wtns layout.json
```

For small proofs, the prover's proof-of-work setting can matter more than the
layout; see [Proof-of-work bits](docs/SCALE.md#proof-of-work-bits).

## Fixtures

Each directory in `fixtures/` holds a Circom source (`kernel.circom`), its R1CS
and WTNS files, a layout file and the SHA-256 hashes of every expected output
(`expected.json`):

| Fixture | Circuit |
| --- | --- |
| `xor` | `305419896 XOR 4275878552 = 3974687968` on 32-bit words |
| `range` | `lo <= x <= hi` for 32-bit values with public bounds |
| `mux` | Selects a row of a public table |
| `adder` | Adds three 32-bit words and compares the sum with a public value |
| `sha256` | circomlib's SHA-256 of a 256-bit message (29 380 constraints), with the hash bits as public outputs; packed outputs only, as it is too large for the reference layout |

All fixtures except `xor` use circomlib templates. `tests/cli.rs` compiles every
fixture and compares the outputs with `expected.json`. The `sha256` tests are
slow in a debug build and ignored by default; run them with
`cargo test --release -- --ignored`. [Testing](docs/TESTING.md) describes the
whole test suite, and [Scale](docs/SCALE.md) the sizes and run times.

The R1CS and WTNS files are committed, so the tests need no Circom.
`fixtures/generate.sh --check` (needs circom 2.2.3 and Node.js) regenerates
them and checks that they are identical byte for byte;
`fixtures/generate.sh NAME` regenerates one fixture.

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

Every compile function runs the checker before it returns. See
[the API reference](docs/API.md), `cargo run --example compile` and
`cargo doc --no-deps`.

## What is supported

- **Input:** R1CS version 1 and WTNS version 2 files over Goldilocks, with
  8-byte field elements. Custom gates and other fields are rejected.
- **Reference layout** (the default; `--rows N`, 64 if not given): one column
  per nonconstant wire, every constraint of the circuit on every row, and the
  public values bound on row 0.
  Large circuits exceed the checker's size limits and need a packed layout.
- **Optimized reference** (`--optimize`): the same, with expressions
  simplified by identities that hold for every field value (such as `x · 1 = x`)
  and repeated subexpressions reused.
- **Packed layout** (`--packed`, or `--layout FILE` to set the layout): long
  linear combinations are split with auxiliary wires (new wires defined as a
  sum of other wires), equations that share wires are joined into row groups,
  and equal row groups are stacked in the same columns so that they share
  constraints.
- **Modules** (`module`): the reference equations as a fragment for a larger
  AIR (a host), with exported wires and no public values or row count. The
  host places the columns anywhere; `check-composition` reads the host's final
  PILout and checks that its AIR still enforces every equation on every row.
  It claims nothing about the host's other constraints, such as a bus.
- **Permutation argument:** when a packed layout puts a wire in more than one cell, a
  permutation argument forces those cells to be equal. It covers only the
  columns that hold such cells, and is left out when every wire has one cell.
- **Independent checks:** the checkers compare the actual PILout constraints,
  fixed columns, symbols, hints and certificate fields with what the source
  requires. They never call the compiler's code;
  [Architecture](docs/ARCHITECTURE.md) explains the separation.

Limits:

- This is an experimental compiler, not a virtual machine or a prover. It has
  no buses or lookups.
- The checks are run by a program, not proved formally.
  [The specification](SPEC.md) argues on paper that every accepted artifact is
  sound and complete.
- Some components are trusted rather than checked: the R1CS and WTNS parser,
  the field arithmetic, the protobuf schema and decoder, SHA-256, the Rust
  toolchain and the proving backend.
- Inputs are assumed not to be adversarial. Sizes are limited, but total time
  and memory are not, so run the checker on untrusted artifacts only under
  external resource limits.

## More documentation

- [CLI](docs/CLI.md): commands, files, layout settings and JSON reports.
- [API](docs/API.md): the Rust functions, types and errors.
- [Architecture](docs/ARCHITECTURE.md): modules and how they depend on each other.
- [Specification](SPEC.md): the relations, the certificates and the
  correctness argument.
- [Testing](docs/TESTING.md): the test suite, mutation testing and the optional
  check against a real prover.
- [Scale](docs/SCALE.md): sizes, run times and prover times.
- [Changelog](CHANGELOG.md): changes and the versioning policy.
- [Provenance](docs/provenance.json): hashes of the schema, the vendored
  circomlib files and the fixtures.
- [Third-party notices](THIRD_PARTY.md): origin and licenses of the schema and
  the circomlib templates.

## License

The compiler is licensed under either the [Apache License, Version 2.0](LICENSE-APACHE)
or the [MIT license](LICENSE-MIT), at your option. Third-party files keep their
own licenses: the PILout schema in `vendor/pilout` is `MIT OR Apache-2.0`, and
the circomlib templates in `vendor/circomlib`, used only by the fixtures, are
GPL-3.0-or-later. See [THIRD_PARTY.md](THIRD_PARTY.md).
