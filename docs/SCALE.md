# Scale

The `sha256` fixture is circomlib's `Sha256(256)`: 29 380 constraints, 29 325
wires, 256 private inputs and 256 public outputs (the bits of the hash). Its
message is the bytes `0, 1, …, 31`, and the public values of its witness are
the SHA-256 of those bytes.

## Reference layout

The reference layout needs one column per wire and one AIR constraint per
equation. For this circuit that is 29 324 columns and more expressions than
`MAX_EXPRESSIONS` allows, so `air compile --rows 64` fails with
`too many expressions`, and `--optimize` fails with
`evaluated expressions exceed the term limit`. The limits bound the checker's
memory, so circuits of this size need a packed layout.

## Packed layout

With `fixtures/sha256/layout.json` (width 64, chunks of 32, `available` reuse,
folding, `local` strategy) the AIR has 1024 rows, 64 stage-one and 16
stage-two columns, 483 fixed columns and 960 constraints.

| Output | Size |
| --- | --- |
| `kernel.pilout` | 1.8 MB |
| `certificate.json` | 23 MB |
| `map.bin` | 256 KiB |
| `trace.bin` | 512 KiB |

Most of the certificate is the converted constraints and the cell of each
wire, written as indented JSON.

## Run times

Measured with `/usr/bin/time` on an Intel Core Ultra 9 185H under Linux, with
Rust 1.96.1 in a release build; each value is the median of three runs. They
show the order of magnitude on one machine, not a guarantee.

| Command | Time | Peak memory |
| --- | --- | --- |
| `air compile --layout` (includes the check) | 2.7 s | 140 MB |
| `air check` | 1.4 s | 120 MB |
| `air plan` | 1.4 s | 70 MB |
| `air witness` | 2.5 s | 120 MB |

With pil2-proofman at commit `eda04742b64abe0e5322a8231f0155438ffb62fe`
(see [the backend check](../tools/backend-check/README.md)), the prover's
setup took 0.8 s, verifying every constraint on the witness 0.9 s, and
generating and verifying a STARK proof 0.9 s.

## Reproducing

```sh
cargo build --locked --release
air=target/release/air
f=fixtures/sha256
/usr/bin/time -v $air compile $f/kernel.r1cs out/packed --layout $f/layout.json
/usr/bin/time -v $air check $f/kernel.r1cs out/packed/kernel.pilout out/packed/certificate.json
/usr/bin/time -v $air plan $f/kernel.r1cs out/plan --layout $f/layout.json
/usr/bin/time -v $air witness $f/kernel.r1cs out/packed $f/input.wtns out/witness
```

`tools/backend-check/run.sh` runs the prover steps for every fixture.
