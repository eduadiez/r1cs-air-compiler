#!/bin/sh
# Time pil2-proofman on one packed artifact: compile SOURCE with LAYOUT,
# project the witness INPUT, run the prover's setup, generate and verify RUNS
# proofs (default 3), and print the median time of each prover phase as JSON.
# Needs a WORK_DIR in which run.sh has already built pil2-proofman.
#
# Compare layouts by "withoutGrinding", the proof time minus the FRI query
# phase. That phase includes a proof-of-work search whose length depends on the
# transcript: the same on every run of one artifact, but different between
# artifacts.
#
# STARK_STRUCTS, if given, is a starkstructs.json file for the setup, for
# example {"Kernel": {"powBits": 20}} to set the proof-of-work bits of the AIR
# (named Kernel). The setup then derives the number of FRI queries; both are
# printed.
#
# Usage: tools/backend-check/prove-time.sh WORK_DIR SOURCE.r1cs INPUT.wtns LAYOUT.json [RUNS [STARK_STRUCTS]]
set -eu
if [ "$#" -lt 4 ] || [ "$#" -gt 6 ]; then
    echo "usage: $0 WORK_DIR SOURCE.r1cs INPUT.wtns LAYOUT.json [RUNS [STARK_STRUCTS]]" >&2
    exit 2
fi
unset CARGO_TARGET_DIR
root=$(cd "$(dirname "$0")/../.." && pwd)
work=$(cd "$1" && pwd)
source=$2
input=$3
layout=$4
runs=${5:-3}
structs=${6:-}
release="$work/pil2-proofman/target/release"
for binary in "$release/proofman-setup" "$release/proofman-cli" "$release/libr1cs_air_witness.so"; do
    if [ ! -e "$binary" ]; then
        echo "$binary is missing; run tools/backend-check/run.sh $work first" >&2
        exit 1
    fi
done
(cd "$root" && cargo build --locked --release -q)
air="$root/target/release/air"
out=$(mktemp -d "$work/prove-time.XXXXXX")
"$air" compile "$source" "$out/artifact" --layout "$layout" >/dev/null
"$air" witness "$source" "$out/artifact" "$input" "$out/witness" >/dev/null
if [ -n "$structs" ]; then
    "$release/proofman-setup" setup -a "$out/artifact/kernel.pilout" -b "$out/build" \
        -s "$structs" >"$out/setup.log" 2>&1
else
    "$release/proofman-setup" setup -a "$out/artifact/kernel.pilout" -b "$out/build" >"$out/setup.log" 2>&1
fi
run=1
while [ "$run" -le "$runs" ]; do
    mkdir "$out/proof$run"
    R1CS_AIR_WITNESS="$out/witness" "$release/proofman-cli" prove \
        -w "$release/libr1cs_air_witness.so" -k "$out/build/provingKey" \
        -o "$out/proof$run" -y -vv >"$out/prove$run.log" 2>&1
    run=$((run + 1))
done
python3 -I - "$out" "$runs" <<'EOF'
import glob, json, re, statistics, sys

out, runs = sys.argv[1], int(sys.argv[2])
phases = {
    "stage1": "STARK_STEP_1",
    "stage2": "STARK_STEP_2",
    "quotient": "STARK_CALCULATE_QUOTIENT_POLYNOMIAL",
    "evaluations": "STARK_STEP_EVALS",
    "friQueries": "STARK_FRI_QUERIES",
    "proof": "STARK_PROOF",
}
times = {name: [] for name in phases}
for run in range(1, runs + 1):
    log = open(f"{out}/prove{run}.log").read()
    if "VERIFYING_PROOFS" not in log:
        sys.exit(f"proof {run} failed; see {out}/prove{run}.log")
    for name, timer in phases.items():
        match = re.search(rf"<-- {timer} done: ([0-9.]+) s", log)
        times[name].append(float(match.group(1)) * 1000)
median = {name: round(statistics.median(values)) for name, values in times.items()}
median["withoutGrinding"] = median["proof"] - median["friQueries"]
layout = json.load(open(f"{out}/artifact/certificate.json"))["layout"]
(info,) = glob.glob(f"{out}/build/provingKey/*/*/airs/*/air/*.starkinfo.json")
stark = json.load(open(info))["starkStruct"]
print(json.dumps({
    "rows": layout["rows"],
    "width": layout["width"],
    "gates": len(layout["gates"]),
    "selectors": len(layout["selectors"]),
    "powBits": stark["powBits"],
    "queries": stark["nQueries"],
    "milliseconds": median,
    "logs": out,
}))
EOF
