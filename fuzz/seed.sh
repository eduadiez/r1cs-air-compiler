#!/bin/sh
# Fill the fuzz corpora with valid inputs: the R1CS files of every fixture that
# fits the reference layout, and the xor fixture's reference and packed PILout
# and packed certificate.
#
# Usage: fuzz/seed.sh
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
corpus="$root/fuzz/corpus"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
(cd "$root" && cargo build --locked --release --quiet)
air="$root/target/release/air"
mkdir -p "$corpus/r1cs" "$corpus/reference_pilout" "$corpus/packed_pilout" \
    "$corpus/packed_certificate"
for source in "$root"/fixtures/*/kernel.r1cs; do
    fixture=$(dirname "$source")
    if grep -q '"reference"' "$fixture/expected.json"; then
        cp "$source" "$corpus/r1cs/$(basename "$fixture")"
    fi
done
xor="$root/fixtures/xor"
"$air" compile "$xor/kernel.r1cs" "$work/reference" --rows 64 >/dev/null
"$air" compile "$xor/kernel.r1cs" "$work/packed" --layout "$xor/layout.json" >/dev/null
cp "$work/reference/kernel.pilout" "$corpus/reference_pilout/xor"
cp "$work/packed/kernel.pilout" "$corpus/packed_pilout/xor"
cp "$work/packed/certificate.json" "$corpus/packed_certificate/xor"
echo "corpora written to $corpus"
