# Hotspot 01: Regex Predicate Evaluation in extract_multi_matches

## Observed in flamegraph
Frame: `dora::query::extract_multi_matches`
Child frame: `regex::exec::ExecNoSync::exec_nfa`
Approximate share of CPU time: 25-40% when queries contain #match? predicates

## Description
Even with pre-compiled Arc<Regex> objects from issue #40, the regex NFA
execution itself dominates when #match? predicates are evaluated per-capture
per-match across millions of nodes. The bottleneck is not compilation —
it is evaluation frequency.

## Proposed fix
Cache the is_match result per (capture_text, regex_ptr) pair within a single
file traversal. If the same identifier text is captured multiple times in one
file (common in large files), the regex evaluation result is memoized.

## Child issue
#43a — Memoize regex evaluation results within file traversal scope
