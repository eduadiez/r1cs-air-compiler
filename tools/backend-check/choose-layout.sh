#!/bin/sh
# Estimate which layout of SOURCE pil2-proofman proves fastest, without
# proving. Each candidate is compiled and run through the prover's setup only;
# a cost model fitted to prove-time.sh runs turns the setup's counts
# (constraint operations, committed and fixed columns, extended rows) into an
# estimate. Prints one JSON line per candidate, best first.
#
# Candidates, all with width WIDTH and auxiliaryTerms half of it: rowGroupWidth
# WIDTH and 2 * WIDTH, at minRows 64, 128, ... 65536; each sweep stops when the
# estimate has risen twice in a row or a candidate no longer fits.
# Needs a WORK_DIR in which run.sh has already built pil2-proofman.
#
# The estimate ranks layouts; it is not a measured time. Time the best few
# with prove-time.sh before relying on them.
#
# Usage: tools/backend-check/choose-layout.sh WORK_DIR SOURCE.r1cs [WIDTH]
# WIDTH (default 64) is options.width of the columns candidates: the most
# distinct wires one equation may use.
set -eu
if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
    echo "usage: $0 WORK_DIR SOURCE.r1cs [WIDTH]" >&2
    exit 2
fi
unset CARGO_TARGET_DIR
root=$(cd "$(dirname "$0")/../.." && pwd)
work=$(cd "$1" && pwd)
source=$2
width=${3:-64}
setup="$work/pil2-proofman/target/release/proofman-setup"
if [ ! -e "$setup" ]; then
    echo "$setup is missing; run tools/backend-check/run.sh $work first" >&2
    exit 1
fi
(cd "$root" && cargo build --locked --release -q)
air="$root/target/release/air"
out=$(mktemp -d "$work/choose-layout.XXXXXX")

# Compile and set up one candidate; print its layout and the setup's counts.
# Return 1 when the layout exceeds a size limit (the candidate does not fit)
# and 2 on any other failure, after printing it.
candidate() {
    name=$1
    layout=$2
    printf '%s\n' "$layout" >"$out/$name.json"
    if ! "$air" compile "$source" "$out/$name" --layout "$out/$name.json" >"$out/$name.report" 2>&1; then
        if grep -q '"kind":"limit"' "$out/$name.report"; then
            return 1
        fi
        echo "compiling candidate $name failed:" >&2
        cat "$out/$name.report" >&2
        return 2
    fi
    if ! "$setup" setup -a "$out/$name/kernel.pilout" -b "$out/$name.build" >"$out/$name.log" 2>&1; then
        echo "the prover setup of candidate $name failed; see $out/$name.log" >&2
        return 2
    fi
    python3 -I - "$out/$name.json" "$out/$name.build" <<'EOF' || return 2
import glob, json, sys

layout, build = sys.argv[1], sys.argv[2]
(info,) = glob.glob(f"{build}/provingKey/*/*/airs/*/air/*.starkinfo.json")
stark = json.load(open(info))
expressions = json.load(open(info.replace("starkinfo", "expressionsinfo")))
(quotient,) = [e for e in expressions["expressionsCode"] if e["expId"] == stark["cExpId"]]
sections = stark["mapSectionsN"]
committed = sum(n for name, n in sections.items() if name.startswith("cm"))
extended = 2 ** stark["starkStruct"]["nBitsExt"] / 1e6
# Fitted to 90 prove-time.sh runs of six circuits (median error 10%), in
# milliseconds; the proof-of-work search is left out. The first terms are per
# million (item × extended row), the last ones per proof and per column.
columns = committed + sections["const"]
estimate = (
    extended * (
        0.229 * len(quotient["code"]) + 10.6 * committed + 1.38 * sections["const"] + 532
    )
    + 4.5
    + 0.0134 * columns
)
print(json.dumps({
    "layout": json.load(open(layout)),
    "rows": 2 ** stark["starkStruct"]["nBits"],
    "committedColumns": committed,
    "fixedColumns": sections["const"],
    "operations": len(quotient["code"]),
    "estimateMs": round(estimate),
}))
EOF
}

options() {
    echo "{\"width\":$1,\"auxiliaryTerms\":$(($1 / 2)),\"reuseAuxiliaries\":\"available\",\"foldConstants\":true}"
}

# Larger minRows give narrower traces. Too few rows make the trace wider than
# the limit; past the first minRows that fits, stop once the estimate has
# risen twice in a row or a candidate no longer fits. Each row group width
# (WIDTH and twice WIDTH) gets its own sweep.
for row_group in "$width" $((2 * width)); do
    rows=64
    rises=0
    previous=""
    while [ "$rows" -le 65536 ] && [ "$rises" -lt 2 ]; do
        status=0
        line=$(candidate "columns-$row_group-$rows" \
            "{\"options\":$(options "$width"),\"minRows\":$rows,\"rowGroupWidth\":$row_group}") ||
            status=$?
        if [ "$status" -gt 1 ]; then
            exit "$status"
        elif [ "$status" -eq 0 ]; then
            echo "$line"
            estimate=$(printf '%s' "$line" | python3 -I -c 'import json, sys; print(json.load(sys.stdin)["estimateMs"])')
            if [ -n "$previous" ] && [ "$estimate" -gt "$previous" ]; then
                rises=$((rises + 1))
            else
                rises=0
            fi
            previous=$estimate
        elif [ -n "$previous" ]; then
            break
        fi
        rows=$((rows * 2))
    done
done >"$out/columns.jsonl"
if [ ! -s "$out/columns.jsonl" ]; then
    echo "no candidate fits within the size limits; see $out" >&2
    exit 1
fi
python3 -I -c '
import json, sys
candidates = sorted((json.loads(line) for line in sys.stdin), key=lambda c: c["estimateMs"])
for c in candidates:
    print(json.dumps(c))
' <"$out/columns.jsonl"
echo "work files in $out" >&2
