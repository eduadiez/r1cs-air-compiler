# Rust API

All public items are at the crate root. The parser, field arithmetic, protobuf
types and placement internals are private.

| Function | Returns | What it checks |
| --- | --- | --- |
| `compile_reference(source, rows)` | `ReferenceArtifact` | Builds the reference relation, then runs the reference checker |
| `compile_optimized_reference(source, rows)` | `ReferenceArtifact` | Same relation, with expressions simplified and reused, then checked |
| `plan_layout(source, &config)` | `LayoutPlan` | Builds helpers and placement and runs the placement check; no PILout |
| `compile_packed(source, &config)` | `PackedArtifact` | Builds the packed relation, then runs the packed checker |
| `check_reference(source, pilout, certificate)` | `ReferenceCheckReport` | Checks a reference artifact from any origin |
| `check_packed(source, pilout, certificate)` | `PackedCheckReport` | Checks a packed artifact from any origin |
| `project_witness(source, pilout, certificate, wtns)` | `Witness` | Checks the artifact, then the witness, then fills the trace |

Types:

- `ReferenceCertificate` and `PackedCertificate` serialize to the certificate
  JSON described in [the specification](../SPEC.md). Their fields are private;
  methods give the version, relation, hashes, row and column counts and, for
  packed certificates, whether a copy argument is needed and the number of
  helpers.
- `ReferenceArtifact` and `PackedArtifact` hold the PILout bytes and the
  certificate; a packed artifact also holds the wire in each cell (`map`).
- `LayoutPlan` gives the row and column counts, the number of helpers and
  checked equations, whether a copy argument is needed, and the wire in each cell.
- `Witness` holds the stage-one trace, row by row, and the public values.
- `LayoutConfig`, `ConversionOptions`, `Strategy` and `Reuse` are the packed
  layout settings; [the CLI reference](CLI.md) lists their values.
- `REFERENCE_RELATION` and `PACKED_RELATION` are the relation names found in a
  certificate's `relation` field; `GOLDILOCKS_PRIME` is the field modulus.

All public types except `Error` implement `Debug`, `Clone` and `PartialEq`.
`Strategy`, `Reuse`, `ErrorKind`, the artifacts, the check reports and `Witness`
are `#[non_exhaustive]`: later versions may add variants and fields, so a
`match` on these enums needs a wildcard arm. `LayoutConfig` and
`ConversionOptions` are built by the caller and have no such attribute.

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

`cargo run --example compile` runs a similar example. Generate the API
documentation with `cargo doc --no-deps`.

## Errors

Every fallible function returns `Result<T>`, whose error type `Error` is `Send`
and `Sync`. `{}` prints the outermost message, which names the stage that
failed; `{:#}` prints the whole chain of causes, and `Error::source()` returns
the next cause. Messages name the zero-based wire, equation or expression index
where one applies, and the condition that failed. For example, a wire number
out of range in a four-wire R1CS file gives, with `{:#}`,
`reference source: equation 0, factor A: wire 999 outside declared range 0..4`.

`Error::kind()` tells why the call failed:

- `ErrorKind::Invalid`: an input is malformed, unsupported or inconsistent, or
  an artifact does not encode its source's relation;
- `ErrorKind::Limit`: an input exceeds a size limit, for example a circuit too
  large for the reference layout (see [Scale](SCALE.md)), which may still fit a
  packed layout.

Message text is for people and may change; programs should rely on the success
of the call and on `Error::kind()` (or the CLI's `ok` field and exit status)
rather than on the text.

Reference row counts must be a power of two in `64..=65536`. Packed widths,
helper chunk sizes and minimum row counts are validated, and the checkers have
size limits. The public functions do not skip any check.
