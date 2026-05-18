# Hotspot 02: PathBuf Allocation Per MatchResult

## Observed in flamegraph
Frame: `ast_search::query::extract_multi_matches`
Child frame: `std::path::PathBuf::clone` → `alloc::string::String::push_str`
Approximate share of CPU time: 10-20% on repos with high match density

## Description
Every MatchResult stores a PathBuf clone of the file path. For a file with
500 matches, 500 PathBuf heap allocations occur — one per result. On a
10,000-file repo with high match density, this produces millions of short-lived
allocations that pressure the allocator.

## Proposed fix
Intern file paths: store paths in a global or per-search-run interner that
maps PathBuf to a numeric ID (u32). MatchResult stores the u32. The path is
reconstructed for output only. Alternatively, use Arc<PathBuf> shared across
all MatchResult entries for the same file.

## Child issue
#43b — Intern file paths in MatchResult to eliminate per-result PathBuf clones
