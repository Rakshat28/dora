use std::{path::Path, sync::Arc};

use tree_sitter::{Query, QueryCursor, Tree};

use crate::types::{AppError, MatchResult, Result};

pub fn compile_query(language: &tree_sitter::Language, query_source: &str) -> Result<Arc<Query>> {
    Query::new(language, query_source)
        .map(Arc::new)
        .map_err(|error| AppError::QueryCompileError(error.to_string()))
}

pub fn extract_matches(
    tree: &Tree,
    source: &str,
    query: &Query,
    file_path: &Path,
) -> Vec<MatchResult> {
    let mut cursor = QueryCursor::new();
    let root_node = tree.root_node();
    let capture_names = query.capture_names();
    let mut results = Vec::new();

    // QueryCursor walks the tree without retaining the Tree, which keeps the
    // per-file memory footprint ephemeral as required by the MVP design.
    for query_match in cursor.matches(query, root_node, source.as_bytes()) {
        for capture in query_match.captures {
            let node = capture.node;
            let capture_name = capture_names[capture.index as usize];
            let byte_range = node.byte_range();
            let matched_text = source[byte_range].to_owned();
            let start_position = node.start_position();
            let end_position = node.end_position();

            results.push(MatchResult {
                file_path: file_path.to_path_buf(),
                capture_name: capture_name.to_string(),
                matched_text,
                start_line: start_position.row + 1,
                start_col: start_position.column,
                end_line: end_position.row + 1,
                end_col: end_position.column,
            });
        }
    }

    results
}
