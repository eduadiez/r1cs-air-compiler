# Rust API

Every public item is at the crate root (`r1cs_air_compiler::`). The parser, the
field arithmetic, the protobuf types and the placement internals are private.
`cargo doc --no-deps` builds the full reference.

## Functions

Each function takes the R1CS file as bytes (`source`).

| Function | Returns | What it does |
| --- | --- | --- |
| `compile_reference(source, rows)` | `ReferenceArtifact` | Builds the reference artifact, then runs the reference checker on it |
| `compile_optimized_reference(source, rows)` | `ReferenceArtifact` | The same, with expressions simplified and repeated subexpressions reused |
| `compile_packed(source, &config)` | `PackedArtifact` | Builds the packed artifact, then runs the packed checker on it |
| `plan_layout(source, &config)` | `LayoutPlan` | Builds the helpers and the placement and runs the placement check; no PILout |
| `check_reference(source, pilout, &certificate)` | `ReferenceCheckReport` | Checks a reference artifact from any origin |
| `check_packed(source, pilout, &certificate)` | `PackedCheckReport` | Checks a packed artifact from any origin |
| `project_witness(source, pilout, &certificate, wtns)` | `Witness` | Runs `check_packed`, checks that the witness satisfies every equation, then fills the trace |

`rows` must be a power of two from 64 to 65536. `config` is a `LayoutConfig`;
[the CLI reference](CLI.md#layout-configuration) lists its fields and values.
No public function skips a check.

## Example

```rust
use r1cs_air_compiler::{compile_reference, Result};

fn main() -> Result<()> {
    let source = std::fs::read("fixtures/xor/kernel.r1cs")?;
    let artifact = compile_reference(&source, 64)?;
    // compile_reference has already run the checker on this artifact.
    println!("{} PILout bytes", artifact.pilout.len());
    Ok(())
}
```

`cargo run --example compile` runs a similar example.

## Types

| Type | Content |
| --- | --- |
| `ReferenceArtifact` | `pilout` (bytes) and `certificate` |
| `PackedArtifact` | `pilout`, `certificate` and `map`, the wire in each cell row by row (0 for an unused cell) |
| `ReferenceCertificate` | Private fields; methods `version`, `relation`, `source_sha256`, `pilout_sha256`, `rows`, `width`, `public_wires`, `wire_columns` |
| `PackedCertificate` | Private fields; methods `version`, `relation`, `source_sha256`, `pilout_sha256`, `rows`, `width`, `requires_copy`, `helper_count` |
| `LayoutPlan` | Methods `rows`, `width`, `helper_count`, `requires_copy`, `checked_equations`, `map` |
| `ReferenceCheckReport`, `PackedCheckReport` | The fields of the CLI's `check` report, in snake case |
| `Witness` | `trace` (stage one, row by row) and `publics`, as `Vec<u64>` |
| `LayoutConfig`, `ConversionOptions`, `Strategy`, `Reuse` | The packed layout settings |
| `Error`, `ErrorKind`, `Result<T>` | See [Errors](#errors) |

The certificates serialize to the certificate JSON of
[the specification](../SPEC.md#certificates). Constants: `REFERENCE_RELATION`
and `PACKED_RELATION` are the values of a certificate's `relation` field, and
`GOLDILOCKS_PRIME` is the field modulus.

Every public type except `Error` implements `Debug`, `Clone` and `PartialEq`.
`Strategy`, `Reuse`, `ErrorKind`, the artifacts, the check reports and
`Witness` are `#[non_exhaustive]`: later versions may add variants and fields,
so a `match` on these enums needs a wildcard arm. `LayoutConfig` and
`ConversionOptions` have public fields and are built by the caller.

## Errors

Every fallible function returns `Result<T>`, whose error type `Error` is `Send`
and `Sync`.

- `{}` prints the outermost message, which names the stage that failed.
- `{:#}` prints the whole chain of causes, separated by `": "`.
- `Error::source()` returns the next cause.

Messages name the zero-based wire, equation or expression index where one
applies, and the condition that failed. For example, a wire number out of
range in a four-wire R1CS file gives, with `{:#}`:

```text
reference source: equation 0, factor A: wire 999 outside declared range 0..4
```

`Error::kind()` tells why the call failed:

| `ErrorKind` | Meaning |
| --- | --- |
| `Invalid` | An input is malformed, unsupported or inconsistent, or an artifact does not encode its source's relation |
| `Limit` | An input exceeds a size limit, for example a circuit too large for the reference layout ([Scale](SCALE.md)); it may still fit a packed layout |

Message text is meant for people and may change. Programs should rely on
`Error::kind()` (or, in the CLI, on `ok`, `kind` and the exit status).
