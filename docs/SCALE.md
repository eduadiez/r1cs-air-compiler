# Scale

Sizes, run times and prover times for large circuits. All times were measured
on one machine (Intel Core Ultra 9 185H, Linux, Rust 1.96.1, release build);
read them as orders of magnitude and relative differences, not guarantees.

## The sha256 fixture

The fixture is circomlib's `Sha256(256)`: 29 380 constraints, 29 325 wires,
256 private inputs (the message bytes `0, 1, …, 31`, as bits) and 256 public
outputs (the bits of their SHA-256 hash).

**Reference layout: too large.** The reference layout uses one column per wire
and one AIR constraint per equation: here 29 324 columns and more than
`MAX_EXPRESSIONS` expressions. `air compile --rows 64` fails with
`too many expressions`, and with `--optimize` it fails with
`evaluated expressions exceed the term limit`. These limits (in
`src/contract.rs`) bound the checker's memory, so a circuit of this size needs
a packed layout.

**Packed layout.** `fixtures/sha256/layout.json` (width 64, `auxiliaryTerms` 32,
`available` reuse, folding, 1024 rows) gives an AIR with:

- 1024 rows;
- 105 stage-one columns, 65 of them in the permutation argument;
- 33 stage-two columns, 249 fixed columns and 355 constraints.

| Output | Size |
| --- | --- |
| `kernel.pilout` | 1.3 MB |
| `certificate.json` | 6.9 MB (mostly the rewritten equations and each wire's cell) |
| `map.bin` | 420 KiB |
| `trace.bin` | 840 KiB |

**Run times** (median of three runs, measured with `/usr/bin/time`):

| Command | Time | Peak memory |
| --- | --- | --- |
| `air compile --layout` (includes the check) | 1.4 s | 140 MB |
| `air check` | 0.5 s | 100 MB |
| `air plan` | 0.8 s | 60 MB |
| `air witness` | 0.8 s | 100 MB |

`plan` and `compile` (which plans first) spend most of their time joining
equations into row groups.

With pil2-proofman at commit `eda04742b64abe0e5322a8231f0155438ffb62fe`
([backend check](../tools/backend-check/README.md)), each whole command took
about 0.5 s for the setup, 0.9 s to verify every constraint on the witness and
1.0 s to generate and verify a STARK proof (measured with a slightly earlier
layout of the fixture). Most of that is starting the prover and
loading the proving key.

## Larger circuits

`Sha256(n)` hashes `n` bits in `⌊(n + 64) / 512⌋ + 1` blocks. Each row below
was compiled with the fixture's settings (width 64, `auxiliaryTerms` 32,
`available` reuse, folding) and the given `minRows`. Every compiled artifact
passed the packed checker.

| Message bits | Constraints | Rows | Width | PILout | Time of `air compile` | Peak memory |
| --- | --- | --- | --- | --- | --- | --- |
| 1024 | 89 609 | 1024 | 238 | 2.7 MB | 2.2 s | 0.4 GB |
| 1024 | 89 609 | 4096 | 75 | 5.1 MB | 2.3 s | 0.4 GB |
| 2048 | 150 265 | 1024 | 368 | 4.0 MB | 4.4 s | 0.7 GB |
| 2048 | 150 265 | 4096 | 126 | 5.9 MB | 3.7 s | 0.7 GB |
| 4096 | 271 577 | 2048 | 327 | 6.9 MB | 7.1 s | 1.3 GB |
| 4096 | 271 577 | 8192 | 122 | 11 MB | 6.9 s | 1.3 GB |
| 8192 | 514 201 | 4096 | 295 | 13 MB | 14 s | 2.4 GB |
| 8192 | 514 201 | 8192 | 174 | 16 MB | 14 s | 2.4 GB |
| 8192 | 514 201 | 32768 | 64 | 35 MB | 15 s | 2.4 GB |

Time and memory grow with the number of constraints. The PILout stays far
below `MAX_PILOUT_BYTES` (128 MB): most of it is fixed columns, so it grows
with the row count. Fewer rows give a wider trace: at 1024 rows the
8192-bit circuit needs 1037 columns, more than 1024, and is rejected.

To reproduce a row:

1. Write a Circom file with `component main = Sha256(n);` that includes
   `vendor/circomlib/sha256/sha256.circom`.
2. Compile it with the flags in `docs/provenance.json` (`fixtureToolchain`).
3. Run `air compile` with this layout, setting `minRows`:

```json
{"options":{"width":64,"auxiliaryTerms":32,"reuseAuxiliaries":"available","foldConstants":true},"minRows":4096}
```

## Prover time

Proof times with pil2-proofman, in milliseconds, measured with
`tools/backend-check/prove-time.sh` (median of 7 proofs). They leave out the
proof-of-work search, which took 20–400 ms per proof whatever the layout (see
[Proof-of-work bits](#proof-of-work-bits)).

The circuits are built like the SHA-256 ones above:

- SHA-256 of 256 (the fixture), 1024 and 4096 bits;
- a chain of 400 `Poseidon(2)` hashes (96 000 constraints);
- a chain of 300 `MiMC7(91)` (109 200 constraints);
- 2 500 `LessThan(32)` comparisons (82 500 constraints);
- 40 `Pedersen(250)` hashes (17 680 constraints);
- 16 Poseidon Merkle paths of depth 24 with `MultiMux1` (93 312 constraints);
- 64 `Multiplexer(8, 64)` (36 864 constraints).

Each layout is the one `tools/backend-check/choose-layout.sh` picked, with
`options.width` 64 and `auxiliaryTerms` 32:

| Circuit | `minRows` | `rowGroupWidth` | Proof time |
| --- | --- | --- | --- |
| SHA-256, 256 bits (fixture) | 512 | 64 | 19 |
| SHA-256, 1024 bits | 1024 | 128 | 35 |
| SHA-256, 4096 bits | 2048 | 128 | 70 |
| Poseidon chain | 512 | 128 | 22 |
| MiMC7 chain | 1024 | 64 | 13 |
| `LessThan(32)` batch | 512 | 64 | 10 |
| Pedersen hashes | 128 | 64 | 10 |
| Poseidon Merkle paths | 512 | 128 | 17 |
| Multiplexers | 1024 | 64 | 19 |

Notes on the table:

- In the Poseidon and MiMC7 chains most wires stay inside one row group, so most
  columns hold no copies and stay out of the permutation argument. In the SHA-256
  circuits about half the cells are copies, because each bit of the state is
  used in several rounds.
- Each `LessThan(32)` comparison fits in one row group, so that layout needs
  no permutation argument.
- At these sizes the proof-of-work search takes longer than the rest of the
  proof.

### What the time depends on

A proof's time is roughly the number of rows of the extended domain times a
weighted count of columns and constraint operations. In the cost model of
`choose-layout.sh`, one committed column (stage one, stage two or quotient)
costs about as much as 8 fixed columns or 46 constraint operations.

- A column in the permutation argument also brings about 1.5 stage-two columns and a
  fixed column of labels. When the compiler decides which row groups to join, it
  counts one trace column as 22 gate constraints (`GATES_PER_COLUMN` in
  `src/layout.rs`).
- Every gate (a gate type placed on specific columns, see `SPEC.md`) adds a
  constraint, and every distinct set of active rows a selector column. Stacking
  equal row groups keeps both near the number of row group kinds, at the cost
  of a wider trace than one that keeps each wire in few cells.

The best `minRows` and `rowGroupWidth` depend on the circuit: in the table above
`minRows` ranges from 128 to 2048. Wider row groups share more cells but leave
more of the trace empty: row groups of 128 wires instead of 64 proved the Poseidon
chain 29% faster, but the 271 577-constraint SHA-256 circuit 7% slower.
`choose-layout.sh` estimates the best layout for a circuit by running only the
prover's setup on each candidate; check its best few with `prove-time.sh`.

### Proof-of-work bits

The prover's setup picks the number of proof-of-work bits (`powBits`, 24 by
default with its default hash) and then the number of FRI queries needed for
its target security level: each bit fewer costs about one more query. The
search takes `2^powBits` hashes on average, so fewer bits save time
exponentially, while the extra queries make the proof and its verification
grow only linearly.

Measured on the 4096-bit circuit at 4096 rows (with an earlier version of the
layout, in which every column was in the permutation argument; the effect of
`powBits` does not depend on the layout), using
`prove-time.sh` with a `starkstructs.json` such as `{"Kernel": {"powBits": 20}}`
(median of 5 proofs, milliseconds):

| `powBits` | FRI queries | FRI query phase (with the search) | Whole proof |
| --- | --- | --- | --- |
| 24 | 211 | 315 | 435 |
| 20 | 219 | 17 | 99 |
| 16 | 228 | 16 | 101 |
| 12 | 236 | 17 | 120 |
| 8 | 244 | 24 | 142 |

- Below about 20 bits the search no longer matters; only the extra queries
  remain.
- More bits pay off when proofs are verified inside another proof (recursion
  or aggregation), where every query makes the verifying circuit larger. For
  proofs verified directly, 16–20 bits reach the same security level in a
  fraction of the time.
- With `powBits` 0 this prover version aborted.

This setting belongs to the prover's deployment, not to the compiler.

## Reproducing

The fixture run times:

```sh
cargo build --locked --release
air=target/release/air
f=fixtures/sha256
mkdir out  # each output directory must not exist yet
/usr/bin/time -v $air compile $f/kernel.r1cs out/packed --layout $f/layout.json
/usr/bin/time -v $air check $f/kernel.r1cs out/packed/kernel.pilout out/packed/certificate.json
/usr/bin/time -v $air plan $f/kernel.r1cs out/plan --layout $f/layout.json
/usr/bin/time -v $air witness $f/kernel.r1cs out/packed $f/input.wtns out/witness
```

The prover steps: `tools/backend-check/run.sh` runs them for every fixture,
and `prove-time.sh` and `choose-layout.sh` give the prover times above (see
the [backend check](../tools/backend-check/README.md)).
