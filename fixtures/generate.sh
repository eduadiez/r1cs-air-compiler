#!/bin/sh
# Regenerate the kernel.r1cs and input.wtns files of the named fixtures (all
# by default) from kernel.circom and input.json. With --check, compare them
# with the committed files byte for byte instead of overwriting them.
# Requires circom 2.2.3 and Node.js; normal builds and tests do not.
#
# Usage: fixtures/generate.sh [--check] [NAME...]
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
check=0
names=""
for argument in "$@"; do
    case "$argument" in
    --check) check=1 ;;
    -*)
        echo "usage: $0 [--check] [NAME...]" >&2
        exit 2
        ;;
    *)
        if [ ! -f "$root/fixtures/$argument/kernel.circom" ]; then
            echo "no fixture named '$argument'" >&2
            exit 2
        fi
        names="$names $argument"
        ;;
    esac
done
if [ -z "$names" ]; then
    for source in "$root"/fixtures/*/kernel.circom; do
        names="$names $(basename "$(dirname "$source")")"
    done
fi
for tool in circom node; do
    if ! command -v "$tool" >/dev/null; then
        echo "$tool is required" >&2
        exit 1
    fi
done
version=$(circom --version | awk '{print $3}')
if [ "$version" != 2.2.3 ]; then
    echo "circom 2.2.3 required, found $version" >&2
    exit 1
fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
for name in $names; do
    dir="$root/fixtures/$name"
    circom "$dir/kernel.circom" --r1cs --wasm --prime goldilocks --O2 -o "$work" >/dev/null
    node "$work/kernel_js/generate_witness.js" "$work/kernel_js/kernel.wasm" \
        "$dir/input.json" "$work/input.wtns"
    if [ "$check" = 1 ]; then
        if cmp -s "$work/kernel.r1cs" "$dir/kernel.r1cs" &&
            cmp -s "$work/input.wtns" "$dir/input.wtns"; then
            echo "$name: reproduced"
        else
            echo "$name: differs from committed files" >&2
            exit 1
        fi
    else
        cp "$work/kernel.r1cs" "$work/input.wtns" "$dir/"
        echo "$name: generated"
    fi
    rm -rf "${work:?}"/*
done
