# Backend check

Optional scripts, run by hand, that feed this compiler's output to a real
prover, [pil2-proofman](https://github.com/0xPolygonHermez/pil2-proofman), at a
fixed commit. They are not part of the build, the tests or CI, and the crate
does not depend on them.

| Script | What it does |
| --- | --- |
| `run.sh WORK_DIR` | Builds the prover, then sets up, checks and proves every fixture |
| `prove-time.sh WORK_DIR SOURCE INPUT LAYOUT [RUNS [STARK_STRUCTS]]` | Times the prover on one packed artifact |
| `choose-layout.sh WORK_DIR SOURCE [WIDTH]` | Estimates which layout proves fastest, without proving |

Requirements: Linux, network access, git, python3, Rust, and a C++ toolchain
with libgmp, libsodium, libomp, nlohmann-json and nasm. Run `run.sh` first:
the other two scripts reuse the prover it builds in `WORK_DIR`.

## `run.sh`

`run.sh WORK_DIR` fetches pil2-proofman into `WORK_DIR`, builds its setup tool,
its CLI and the witness library in `witness/`, and builds `air`. Then, for the
reference and packed artifacts of every fixture (only the packed one when the
fixture's `expected.json` has no `reference` hash, such as `sha256`), it:

1. runs the prover's setup on the PILout, which also reads the fixed columns;
2. verifies every constraint, stage two included, on the witness;
3. generates a STARK proof without aggregation and verifies it.

Where the inputs come from:

- **Stage-one trace:** `air witness` for packed artifacts; for reference
  artifacts, `reference_trace.py` writes the circuit's assignment on every row
  and reads the public values from the certificate's public wires.
- **Stage two:** the witness library computes it from the artifact's own
  `im_col`, `gprod_col` and `gsum_col` hints. The prover's standard library
  would do this only for AIRs listed in its global `std_prod_users` and
  `std_sum_users` hints, which these artifacts do not contain.

The script stops at the first failing step and keeps every log in `WORK_DIR`
(`build.log`, and `NAME-MODE/` per fixture and mode). A later run may reuse the
pil2-proofman checkout in `WORK_DIR`, but fails if a per-fixture output
directory already exists; use a fresh `WORK_DIR` or delete them.
pil2-proofman is built from its own `Cargo.lock`; the script only adds the
witness library to it.

A passing run shows that this prover version accepts the artifacts and their
witnesses. It says nothing about the prover's soundness, and nothing about the
compiler beyond what [the specification](../../SPEC.md) states.

## `prove-time.sh`

`prove-time.sh WORK_DIR SOURCE INPUT LAYOUT [RUNS [STARK_STRUCTS]]` compiles
the R1CS file `SOURCE` with the layout file `LAYOUT`, projects the witness
`INPUT` (a WTNS file), runs the setup, and generates and verifies `RUNS` proofs
(default 3). It prints one JSON object with the layout's `rows`, `width`,
`gates` and `selectors`, the prover's `powBits` and `queries`, and the median
time of each prover phase in `milliseconds`.

Compare layouts by `withoutGrinding`: the proof time minus the FRI query
phase. That phase includes a proof-of-work search ("grinding") whose length
depends on the transcript, so it varies between artifacts by chance.

`STARK_STRUCTS` is an optional `starkstructs.json` for the setup that sets the
prover's parameters for the AIR, which is named `Kernel`. The one that matters
most for small proofs is the number of proof-of-work bits, for example
`{"Kernel": {"powBits": 20}}`; the setup then derives the number of FRI queries
that keeps its target security level. [Scale](../../docs/SCALE.md#proof-of-work-bits)
shows the effect.

## `choose-layout.sh`

`choose-layout.sh WORK_DIR SOURCE [WIDTH]` compiles these candidates, all with
`available` reuse, folding and chunks of half the width:

- `local` at widths 32, 64 and 96, `minRows` 64;
- `columns` at width `WIDTH` (default 64), with `rowGroupWidth` `WIDTH` and
  `2 × WIDTH`, at `minRows` 64, 128, … up to 65536. Each sweep skips sizes
  that do not fit and stops when the estimate has risen twice in a row or a
  candidate no longer fits.

It runs only the prover's setup on each candidate and estimates the proof time
(without the proof-of-work search) from the counts the setup reports:
constraint operations, committed and fixed columns, and extended rows. The cost
model was fitted to 90 `prove-time.sh` runs of six circuits, with a median
error of 10%. It prints one JSON line per candidate, best first, with
`estimateMs`. The estimate is for ranking; time the best few with
`prove-time.sh`.

## Files

- `witness/`: the witness library. It builds only inside the pil2-proofman
  workspace, which supplies its dependencies (`MIT OR Apache-2.0`);
  `add_member.py` adds it to that workspace. Nothing else builds it, so it is
  only exercised when `run.sh` runs.
- `reference_trace.py`: writes the trace and public values of a reference
  artifact.
