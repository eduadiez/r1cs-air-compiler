#!/bin/sh
# Measure how long pil2-proofman takes to prove one packed artifact: compile
# SOURCE with LAYOUT, project INPUT, run the prover's setup, then generate and
# verify RUNS proofs (default 3) and print the median time of each prover phase.
# Needs a WORK_DIR in which run.sh has already built pil2-proofman.
#
# The proof time includes a proof-of-work search whose length depends on the
# transcript: it is the same on every run of one artifact but varies from one
# artifact to another, so layouts are compared by the time without it
# ("withoutGrinding"), which is the proof time minus the FRI query phase.
#
# Usage: tools/backend-check/prove-time.sh WORK_DIR SOURCE.r1cs INPUT.wtns LAYOUT.json [RUNS]
set -eu
if [ "$#" -lt 4 ] || [ "$#" -gt 5 ]; then
    echo "usage: $0 WORK_DIR SOURCE.r1cs INPUT.wtns LAYOUT.json [RUNS]" >&2
    exit 2
fi
unset CARGO_TARGET_DIR
root=$(cd "$(dirname "$0")/../.." && pwd)
work=$(cd "$1" && pwd)
source=$2
input=$3
layout=$4
runs=${5:-3}
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
"$release/proofman-setup" setup -a "$out/artifact/kernel.pilout" -b "$out/build" >"$out/setup.log" 2>&1
run=1
while [ "$run" -le "$runs" ]; do
    mkdir "$out/proof$run"
    R1CS_AIR_WITNESS="$out/witness" "$release/proofman-cli" prove \
        -w "$release/libr1cs_air_witness.so" -k "$out/build/provingKey" \
        -o "$out/proof$run" -y -vv >"$out/prove$run.log" 2>&1
    run=$((run + 1))
done
python3 -I - "$out" "$runs" <<'EOF'
import json, re, statistics, sys

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
print(json.dumps({
    "rows": layout["rows"],
    "width": layout["width"],
    "slots": len(layout["slots"]),
    "selectors": len(layout["selectors"]),
    "milliseconds": median,
    "logs": out,
}))
EOF
