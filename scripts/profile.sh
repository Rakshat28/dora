#!/usr/bin/env bash
set -euo pipefail

REPO_URL="https://github.com/rust-lang/rust"
FIXTURE_DIR="/tmp/doora-profile-fixture"
OUTPUT_DIR="./profiling-output"
QUERY='(function_item name: (identifier) @fn_name)'
LANG="rust"
FILE_LIMIT=10000

mkdir -p "$OUTPUT_DIR"

if [ ! -d "$FIXTURE_DIR" ]; then
    git clone --depth=1 "$REPO_URL" "$FIXTURE_DIR"
fi

ACTUAL_FILES=$(find "$FIXTURE_DIR" -name "*.rs" | wc -l | tr -d ' ')
echo "fixture: $ACTUAL_FILES .rs files in $FIXTURE_DIR"

cargo build --release 2>/dev/null

START=$(date +%s%3N)
./target/release/doora \
    -q "$QUERY" \
    -p "$FIXTURE_DIR" \
    --lang "$LANG" \
    --no-color \
    --quiet \
    2>/dev/null | wc -l
END=$(date +%s%3N)
ELAPSED=$((END - START))
echo "baseline wall time: ${ELAPSED}ms"

if command -v cargo-flamegraph &>/dev/null; then
    cargo flamegraph \
        --bin doora \
        --output "$OUTPUT_DIR/flamegraph.svg" \
        -- \
        -q "$QUERY" \
        -p "$FIXTURE_DIR" \
        --lang "$LANG" \
        --no-color \
        --quiet \
        2>/dev/null
    echo "flamegraph written to $OUTPUT_DIR/flamegraph.svg"
else
    echo "cargo-flamegraph not installed"
    echo "install: cargo install flamegraph"
    echo "on linux: also install linux-perf or use CARGO_PROFILE_RELEASE_DEBUG=1"
fi
