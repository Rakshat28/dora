# Profiling ast-search

## Prerequisites

cargo install flamegraph

On Linux, also install perf:
  sudo apt install linux-perf   # Debian/Ubuntu
  sudo pacman -S perf           # Arch

On macOS, flamegraph uses DTrace (available by default).

## Quickstart

make profile

This downloads the Rust compiler source (~50,000 .rs files), runs a
baseline timing measurement, then generates a flamegraph SVG.

## Manual steps

Build with debug symbols:
  cargo build --release

Run baseline timing:
  make baseline

Generate flamegraph:
  make flamegraph
  open profiling-output/flamegraph.svg

## Interpreting the flamegraph

Wide frames = more CPU time spent there.
Look for wide frames in:
  ast_search::query::extract_multi_matches  — traversal hot path
  ast_search::parser::parse_file            — I/O and FFI boundary
  tree_sitter::query::QueryCursor::matches  — tree-sitter internals

## Performance target

Under 1000ms for a 10,000-file Rust repository on a modern 8-core machine.

Measure with:
  time ./target/release/ast-search \
    -q '(function_item name: (identifier) @fn_name)' \
    -p /path/to/10k-file-repo \
    --lang rust --no-color --quiet 2>/dev/null

## Known hotspots

See docs/hotspots/ for documented hotspots and proposed fixes.
