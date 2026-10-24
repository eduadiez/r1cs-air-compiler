# Backend check

An optional check, run by hand, that a real prover accepts this compiler's
output. It is not part of the build, the tests or CI, and the crate does not
depend on it.

`run.sh WORK_DIR` fetches [pil2-proofman](https://github.com/0xPolygonHermez/pil2-proofman)
at a fixed commit into `WORK_DIR`, builds its setup tool, its CLI and the
witness library in `witness/`, and then, for the reference and packed artifact
of every fixture (only the packed one for a fixture without reference outputs,
such as `sha256`):

1. runs the prover's setup on the PILout, which also reads the fixed columns from it;
2. verifies every constraint, including stage two, on the witness;
3. generates a STARK proof without aggregation and verifies it.

The stage-one trace comes from `air witness` for packed artifacts, and from
`reference_trace.py` for reference artifacts: the circuit's assignment repeated
on every row, and the public values read from the certificate's public wires.
The witness library computes stage two from the artifact's own `im_col`,
`gprod_col` and `gsum_col` hints. The prover's standard library does the same,
but only for AIRs listed in its global `std_prod_users` and `std_sum_users`
hints, which these artifacts do not contain.

The script stops at the first failing step and keeps all logs in `WORK_DIR`. A
passing run shows that this prover version accepts the artifacts and their
witnesses. It says nothing about the prover's soundness, and nothing about the
compiler beyond what [the specification](../../SPEC.md) states.

The script runs on Linux only. It may reuse the pil2-proofman checkout in
`WORK_DIR` from an earlier run, but each run needs fresh per-fixture output
directories. pil2-proofman is built from its own `Cargo.lock`; the script only
adds the witness library to it.

The files under `witness/` build only inside the pil2-proofman workspace, which
supplies their dependencies (`MIT OR Apache-2.0`); `add_member.py` adds them to
that workspace. Nothing builds them automatically, so they are only exercised
when this script runs.
