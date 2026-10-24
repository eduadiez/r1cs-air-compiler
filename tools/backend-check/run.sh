#!/bin/sh
# Optional end-to-end check against the pil2-proofman prover: for every
# fixture, run the prover's setup on the reference and packed PILout, verify
# all constraints on the witness, and generate and verify a STARK proof.
# Not part of the build or CI. Linux only. Needs network access, git, a C++
# toolchain with libgmp, libsodium, libomp, nlohmann-json and nasm, and python3.
#
# Usage: tools/backend-check/run.sh WORK_DIR
# WORK_DIR may hold the pil2-proofman checkout of an earlier run, which is
# reused; any per-fixture output left by an earlier run is an error.
set -eu
PROOFMAN_REPO=https://github.com/0xPolygonHermez/pil2-proofman
PROOFMAN_REV=eda04742b64abe0e5322a8231f0155438ffb62fe

if [ "$#" -ne 1 ]; then
    echo "usage: $0 WORK_DIR" >&2
    exit 2
fi
if [ "$(uname -s)" != Linux ]; then
    echo "$0: only Linux is supported" >&2
    exit 2
fi

# The binaries are expected in each workspace's own target directory.
unset CARGO_TARGET_DIR
root=$(cd "$(dirname "$0")/../.." && pwd)
tool="$root/tools/backend-check"
work=$(mkdir -p "$1" && cd "$1" && pwd)
proofman="$work/pil2-proofman"
log="$work/build.log"
step="setup"
trap 'echo "failed during $step; see the logs in $work" >&2' EXIT

step="fetching pil2-proofman"
if [ ! -d "$proofman" ]; then
    git init -q "$proofman"
    git -C "$proofman" fetch -q --depth 1 "$PROOFMAN_REPO" "$PROOFMAN_REV"
    git -C "$proofman" checkout -q FETCH_HEAD
fi
if [ "$(git -C "$proofman" rev-parse HEAD)" != "$PROOFMAN_REV" ]; then
    echo "$proofman is not at $PROOFMAN_REV; use a fresh WORK_DIR" >&2
    exit 1
fi

step="building (see $log)"
# The witness library builds as a member of the pil2-proofman workspace.
mkdir -p "$proofman/examples/r1cs-air-witness"
cp -R "$tool/witness/." "$proofman/examples/r1cs-air-witness/"
if ! grep -q '"examples/r1cs-air-witness"' "$proofman/Cargo.toml"; then
    python3 -I "$tool/add_member.py" "$proofman/Cargo.toml" examples/r1cs-air-witness
fi
{
    # Adds only the witness library to Cargo.lock; no dependency changes.
    (cd "$proofman" && cargo update --workspace)
    make -C "$proofman/pil2-stark" -j"$(nproc)" starks_lib
    (cd "$proofman" && cargo build --locked --release --bin proofman-setup --bin proofman-cli)
    (cd "$proofman" && cargo build --locked --release -p r1cs-air-witness)
    (cd "$root" && cargo build --locked --release)
} >"$log" 2>&1

air="$root/target/release/air"
setup="$proofman/target/release/proofman-setup"
cli="$proofman/target/release/proofman-cli"
lib="$proofman/target/release/libr1cs_air_witness.so"

for name in xor range mux adder; do
    fixture="$root/fixtures/$name"
    for mode in reference packed; do
        out="$work/$name-$mode"
        step="$name $mode (logs in $out)"
        if [ -e "$out" ]; then
            echo "$out exists; use a fresh WORK_DIR" >&2
            exit 1
        fi
        if [ "$mode" = reference ]; then
            "$air" compile "$fixture/kernel.r1cs" "$out" --rows 64 >/dev/null
            mkdir "$out/witness"
            python3 -I "$tool/reference_trace.py" "$fixture/input.wtns" \
                "$out/certificate.json" "$out/witness"
        else
            "$air" compile "$fixture/kernel.r1cs" "$out" --layout "$fixture/layout.json" >/dev/null
            "$air" witness "$fixture/kernel.r1cs" "$out" "$fixture/input.wtns" "$out/witness" >/dev/null
        fi
        "$setup" setup -a "$out/kernel.pilout" -b "$out/build" >"$out/setup.log" 2>&1
        R1CS_AIR_WITNESS="$out/witness" "$cli" verify-constraints -w "$lib" \
            -k "$out/build/provingKey" >"$out/verify.log" 2>&1
        mkdir "$out/proof"
        R1CS_AIR_WITNESS="$out/witness" "$cli" prove -w "$lib" -k "$out/build/provingKey" \
            -o "$out/proof" -y >"$out/prove.log" 2>&1
        echo "$name $mode: constraints verified, proof verified"
    done
done
trap - EXIT
