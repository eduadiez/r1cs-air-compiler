#!/bin/sh
# Optional end-to-end check with the pil2-proofman prover. Fetches and builds
# pil2-proofman in WORK_DIR, then for the reference and packed artifacts of
# every fixture: runs the prover's setup, verifies every constraint on the
# witness, and generates and verifies a STARK proof. A fixture whose
# expected.json has no "reference" hash is checked in packed mode only.
#
# Not part of the build or CI. Linux only. Needs network access, git, python3,
# Rust, and a C++ toolchain with libgmp, libsodium, libomp, nlohmann-json and
# nasm. See tools/backend-check/README.md.
#
# Usage: tools/backend-check/run.sh WORK_DIR
# A pil2-proofman checkout left in WORK_DIR by an earlier run is reused; a
# per-fixture output directory left by one is an error.
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

for source in "$root"/fixtures/*/kernel.r1cs; do
    fixture=$(dirname "$source")
    name=$(basename "$fixture")
    modes="reference packed"
    if ! grep -q '"reference"' "$fixture/expected.json"; then
        modes=packed
    fi
    for mode in $modes; do
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
