#!/bin/bash

# Diff the built contract interface against the stored snapshot.
# Exits non-zero if they differ, exposing any accidental ABI changes.

set -euo pipefail

# Ensure we run from the project root
ROOT_DIR="$(git rev-parse --show-toplevel)"
cd "$ROOT_DIR"

WASM="target/wasm32v1-none/release/tricklepay_stream.wasm"
SNAPSHOT="docs/interface.txt"

if [ ! -f "$WASM" ]; then
    echo "Error: WASM not found at $WASM. Run 'make wasm' first." >&2
    exit 1
fi

if [ ! -f "$SNAPSHOT" ]; then
    echo "Error: Snapshot not found at $SNAPSHOT." >&2
    exit 1
fi

echo "Diffing built interface against $SNAPSHOT..."
stellar contract inspect --wasm "$WASM" | diff -u "$SNAPSHOT" -

echo "Interface matches."
