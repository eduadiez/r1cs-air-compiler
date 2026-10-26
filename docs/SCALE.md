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
| `certificate.json` | 6.9 MB |
| `map.bin` | 256 KiB |
| `trace.bin` | 512 KiB |

Most of the certificate is the converted constraints and the cell of each
wire.

## Run times

Measured with `/usr/bin/time` on an Intel Core Ultra 9 185H under Linux, with
Rust 1.96.1 in a release build; each value is the median of three runs. They
show the order of magnitude on one machine, not a guarantee.

| Command | Time | Peak memory |
| --- | --- | --- |
| `air compile --layout` (includes the check) | 1.0 s | 140 MB |
| `air check` | 0.5 s | 110 MB |
| `air plan` | 0.4 s | 60 MB |
| `air witness` | 0.8 s | 110 MB |

With pil2-proofman at commit `eda04742b64abe0e5322a8231f0155438ffb62fe`
(see [the backend check](../tools/backend-check/README.md)), the prover's
setup took 0.8 s, verifying every constraint on the witness 0.9 s, and
generating and verifying a STARK proof 0.9 s.

## Larger circuits

`Sha256(n)` hashes `n` bits in `⌊(n + 64) / 512⌋ + 1` blocks. With the same
settings except for the width (chunks of half the width), each of these
compiled and passed the packed checker except where noted; times and peak
memory are for `air compile` (one run each, same machine):

| Message bits | Constraints | Width | Rows | PILout | Time | Peak memory |
| --- | --- | --- | --- | --- | --- | --- |
| 1024 | 89 609 | 64 | 4096 | 11 MB | 1.9 s | 0.4 GB |
| 1024 | 89 609 | 256 | 1024 | 8.0 MB | 2.5 s | 0.5 GB |
| 2048 | 150 265 | 64 | 8192 | 25 MB | 3.4 s | 0.7 GB |
| 2048 | 150 265 | 256 | 2048 | 17 MB | 4.1 s | 0.8 GB |
| 4096 | 271 577 | 64 | 16384 | 60 MB | 6.5 s | 1.3 GB |
| 4096 | 271 577 | 256 | 4096 | 37 MB | 7.3 s | 1.3 GB |
| 8192 | 514 201 | 64 | 32768 | 127 MB | 13.4 s | 2.4 GB |
| 8192 | 514 201 | 128 | — | rejected: `PILout exceeds the size limit` | 10.2 s | 2.3 GB |
| 8192 | 514 201 | 256 | 8192 | 84 MB | 13.8 s | 2.5 GB |
| 8192 | 514 201 | 512 | — | rejected: `evaluated expressions exceed the term limit` | 11.8 s | 1.6 GB |

Around half a million constraints, the first limit reached is
`MAX_PILOUT_BYTES` (128 MB): most of the PILout is fixed columns, whose size
grows with the number of rows times the number of fixed columns. The limit is
checked on the finished PILout, so a rejected compilation does almost all the
work of a successful one. To reproduce a row, compile
`component main = Sha256(n);`, including `vendor/circomlib/sha256/sha256.circom`,
with the flags in `docs/provenance.json` (`fixtureToolchain`), and run
`air compile` with the layout below, changing `width` and `chunk`:

```json
{"options":{"width":64,"chunk":32,"reuse":"available","fold":true},"strategy":"local","minRows":64}
```

## Prover time

Proof times with pil2-proofman, measured with
`tools/backend-check/prove-time.sh` (median of 3 or 5 runs, same machine), for
`Sha256(n)` built and laid out as in "Larger circuits". Times are in
milliseconds and leave out the proof-of-work search (see the
[backend check](../tools/backend-check/README.md)), which took 20–400 ms per
proof regardless of the layout.

| Message bits | Layout (`width`, `chunk`, `strategy`, `minRows`) | Rows × columns | Slots | Selectors | Quotient | Proof without search |
| --- | --- | --- | --- | --- | --- | --- |
| 256 (fixture) | 64, 32, `local`, 64 | 1024 × 64 | 911 | 379 | 7 | 20 |
| 256 (fixture) | 64, 32, `columns`, 512 | 512 × 264 | 153 | 100 | 5 | 34 |
| 1024 | 64, 32, `local`, 64 | 4096 × 64 | 1881 | 986 | 39 | 96 |
| 1024 | 64, 32, `columns`, 4096 | 4096 × 132 | 148 | 130 | 13 | 64 |
| 1024 | 64, 32, `columns`, 2048 | 2048 × 223 | 169 | 129 | 11 | 60 |
| 4096 | 64, 32, `local`, 64 | 16384 × 64 | 2577 | 1435 | 175 | 348 |
| 4096 | 96, 16, `local`, 64 | 8192 × 96 | 2084 | 999 | 94 | 215 |
| 4096 | 64, 32, `columns`, 8192 | 8192 × 164 | 159 | 130 | 30 | 140 |
| 4096 | 32, 16, `columns`, 4096 | 4096 × 280 | 219 | 157 | 23 | 134 |

Most of the time of the row-filling strategies goes to evaluating the
constraints (the quotient), and that grows with the number of slots and
selectors: every slot is its own constraint, and every selector a fixed
column. `columns` gives each shape its own band of columns, so slots and
selectors stay near the number of shapes, at the cost of a wider trace with
more copies. On the two larger circuits it took 35–60% less time; on the
small fixture the wider copy argument costs more than it saves.

The same layouts also give a smaller PILout: `Sha256(8192)` with width 64,
chunks of 32 and `columns` compiles to a 26 MB PILout at 8192 rows and 47 MB
at 32768 rows, against 127 MB with `local`.

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
