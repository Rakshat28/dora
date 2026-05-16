use std::{path::Path, sync::Arc};

use tree_sitter::{Node, Query, QueryCursor, Tree};

use crate::types::{AppError, MatchResult, Result};

#[allow(clippy::missing_errors_doc)]
pub fn compile_query(language: &tree_sitter::Language, query_source: &str) -> Result<Arc<Query>> {
    Query::new(language, query_source)
        .map(Arc::new)
        .map_err(|error| AppError::QueryCompileError(error.to_string()))
}

#[must_use]
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

    for query_match in cursor.matches(query, root_node, source.as_bytes()) {
        for capture in query_match.captures {
            let node = capture.node;

            let base_capture_name = capture_names[capture.index as usize];
            let capture_name = format_capture_name(base_capture_name, &node);

            let byte_range = node.byte_range();

            let matched_text = if let Some(slice) = source.get(byte_range.clone()) {
                slice.to_owned()
            } else {
                eprintln!(
                    "warning: capture byte range {:?} out of bounds for source of length {} — skipping capture '{}'",
                    byte_range,
                    source.len(),
                    capture_name
                );
                continue;
            };

            let start_position = node.start_position();
            let end_position = node.end_position();

            results.push(MatchResult {
                file_path: file_path.to_path_buf(),
                capture_name,
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

fn format_capture_name(base_name: &str, node: &Node<'_>) -> String {
    if node.is_named() {
        base_name.to_string()
    } else {
        format!("{}[{}]", base_name, node.kind())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn parse_inline(source: &str) -> (tree_sitter::Tree, String) {
        use crate::parser::parse_file;
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut file = NamedTempFile::new().unwrap();
        write!(file, "{}", source).unwrap();
        parse_file(file.path()).expect("inline parse failed")
    }

    fn dummy_path() -> PathBuf {
        PathBuf::from("test_file.rs")
    }

    #[test]
    fn test_compile_query_valid() {
        use crate::parser::get_language;

        let lang = get_language("rust").unwrap();
        let result = compile_query(&lang, "(function_item name: (identifier) @fn_name)");

        assert!(
            result.is_ok(),
            "Valid S-expression should compile without error, got: {:?}",
            result.err()
        );

        let query = result.unwrap();
        assert!(
            query.capture_names().contains(&"fn_name"),
            "Compiled query must expose the @fn_name capture name"
        );
    }

    #[test]
    fn test_compile_query_invalid_returns_error() {
        use crate::parser::get_language;

        let lang = get_language("rust").unwrap();

        let result = compile_query(&lang, "((( this is not valid");

        assert!(
            matches!(result, Err(AppError::QueryCompileError(_))),
            "Invalid S-expression must return QueryCompileError, got: {:?}",
            result
        );

        if let Err(AppError::QueryCompileError(msg)) = result {
            assert!(!msg.is_empty(), "QueryCompileError message must not be empty");
            assert!(msg.len() > 5, "QueryCompileError message should be descriptive, got: '{msg}'");
        }
    }

    #[test]
    fn test_compile_query_unknown_node_type() {
        use crate::parser::get_language;

        let lang = get_language("rust").unwrap();

        let result = compile_query(&lang, "(nonexistent_node_xyz @cap)");

        assert!(
            matches!(result, Err(AppError::QueryCompileError(_))),
            "Unknown node type must return QueryCompileError, got: {:?}",
            result
        );
    }

    #[test]
    fn test_compile_query_arc_is_shareable() {
        use crate::parser::get_language;

        let lang = get_language("rust").unwrap();
        let query = compile_query(&lang, "(identifier) @id").unwrap();

        let query2 = Arc::clone(&query);
        assert_eq!(
            query.capture_names(),
            query2.capture_names(),
            "Arc clones must share the same compiled query"
        );
        assert_eq!(Arc::strong_count(&query), 2);
    }

    #[test]
    fn test_extract_matches_function_definition() {
        use crate::parser::get_language;

        let source = r#"
fn authenticate(user: &str, password: &str) -> bool {
    true
}
fn logout() {}
"#;
        let lang = get_language("rust").unwrap();
        let query = compile_query(&lang, "(function_item name: (identifier) @fn_name)").unwrap();

        let (tree, src) = parse_inline(source);
        let results = extract_matches(&tree, &src, &query, &dummy_path());
        drop(tree);
        drop(src);

        assert_eq!(results.len(), 2, "Expected 2 function matches, got: {:?}", results);

        let names: Vec<&str> = results.iter().map(|r| r.matched_text.as_str()).collect();

        assert!(names.contains(&"authenticate"), "Must find 'authenticate'");
        assert!(names.contains(&"logout"), "Must find 'logout'");

        for result in &results {
            assert_eq!(
                result.capture_name, "fn_name",
                "Named node capture name must not have brackets appended"
            );
        }
    }

    #[test]
    fn test_extract_matches_no_matches_returns_empty() {
        use crate::parser::get_language;

        let source = "fn main() { let x = 1; }";
        let lang = get_language("rust").unwrap();
        let query =
            compile_query(&lang, "(struct_item name: (type_identifier) @struct_name)").unwrap();

        let (tree, src) = parse_inline(source);
        let results = extract_matches(&tree, &src, &query, &dummy_path());
        drop(tree);
        drop(src);

        assert!(
            results.is_empty(),
            "Expected no matches for struct query on fn-only source, got: {:?}",
            results
        );
    }

    #[test]
    fn test_extract_matches_line_numbers_are_1_indexed() {
        use crate::parser::get_language;

        let source = "\nfn first() {}\nfn second() {}";
        let lang = get_language("rust").unwrap();
        let query = compile_query(&lang, "(function_item name: (identifier) @fn_name)").unwrap();

        let (tree, src) = parse_inline(source);
        let results = extract_matches(&tree, &src, &query, &dummy_path());
        drop(tree);
        drop(src);

        assert_eq!(results.len(), 2);

        let first = results.iter().find(|r| r.matched_text == "first").unwrap();
        assert_eq!(first.start_line, 2, "line numbers must be 1-indexed");

        let second = results.iter().find(|r| r.matched_text == "second").unwrap();
        assert_eq!(second.start_line, 3);
    }

    #[test]
    fn test_extract_matches_eq_predicate() {
        use crate::parser::get_language;

        let source = r#"
fn connect() {}
fn disconnect() {}
fn reconnect() {}
"#;
        let lang = get_language("rust").unwrap();

        let query = compile_query(
            &lang,
            r#"
            (function_item
              name: (identifier) @fn_name
              (#eq? @fn_name "connect"))
        "#,
        )
        .unwrap();

        let (tree, src) = parse_inline(source);
        let results = extract_matches(&tree, &src, &query, &dummy_path());
        drop(tree);
        drop(src);

        assert_eq!(
            results.len(),
            1,
            "Expected exactly 1 match for #eq? predicate, got: {:?}",
            results
        );
        assert_eq!(results[0].matched_text, "connect");
    }

    #[test]
    fn test_extract_matches_file_path_populated() {
        use crate::parser::get_language;

        let source = "fn foo() {}";
        let lang = get_language("rust").unwrap();
        let query = compile_query(&lang, "(function_item name: (identifier) @fn_name)").unwrap();

        let specific_path = PathBuf::from("src/auth/handler.rs");
        let (tree, src) = parse_inline(source);
        let results = extract_matches(&tree, &src, &query, &specific_path);
        drop(tree);
        drop(src);

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].file_path, specific_path,
            "MatchResult.file_path must match the path passed to extract_matches"
        );
    }

    #[test]
    fn test_extract_matches_multiple_captures_per_match() {
        use crate::parser::get_language;

        let source = "fn process(input: String) {}";
        let lang = get_language("rust").unwrap();

        let query = compile_query(
            &lang,
            r#"
            (function_item
              name: (identifier) @fn_name
              parameters: (parameters
                (parameter pattern: (identifier) @param_name)))
        "#,
        )
        .unwrap();

        let (tree, src) = parse_inline(source);
        let results = extract_matches(&tree, &src, &query, &dummy_path());
        drop(tree);
        drop(src);

        assert_eq!(
            results.len(),
            2,
            "Expected 2 captures (fn_name + param_name), got: {:?}",
            results
        );

        let fn_result = results
            .iter()
            .find(|r| r.capture_name == "fn_name")
            .expect("Must have fn_name capture");
        assert_eq!(fn_result.matched_text, "process");

        let param_result = results
            .iter()
            .find(|r| r.capture_name == "param_name")
            .expect("Must have param_name capture");
        assert_eq!(param_result.matched_text, "input");
    }

    #[test]
    fn test_format_capture_name_named_node() {
        use crate::parser::get_language;

        let source = "fn main() {}";
        let lang = get_language("rust").unwrap();
        let query = compile_query(&lang, "(function_item name: (identifier) @fn_name)").unwrap();

        let (tree, src) = parse_inline(source);
        let mut cursor = QueryCursor::new();
        let matches: Vec<_> = cursor.matches(&query, tree.root_node(), src.as_bytes()).collect();

        let node = matches[0].captures[0].node;
        assert!(node.is_named(), "identifier should be a named node");

        let result = format_capture_name("fn_name", &node);
        assert_eq!(result, "fn_name", "Named node must not have kind appended");

        drop(tree);
        drop(src);
    }

    #[test]
    fn test_format_capture_name_anonymous_node() {
        use crate::parser::get_language;

        let source = "fn main() {}";
        let lang = get_language("rust").unwrap();

        let query = compile_query(&lang, r#"("fn" @keyword)"#).unwrap();

        let (tree, src) = parse_inline(source);
        let mut cursor = QueryCursor::new();
        let matches: Vec<_> = cursor.matches(&query, tree.root_node(), src.as_bytes()).collect();

        if matches.is_empty() {
            drop(tree);
            drop(src);
            return;
        }

        let node = matches[0].captures[0].node;

        let result = format_capture_name("keyword", &node);

        if node.is_named() {
            assert_eq!(result, "keyword");
        } else {
            assert!(
                result.starts_with("keyword["),
                "Anonymous node capture name must start with 'keyword[', got: '{result}'"
            );
            assert!(
                result.ends_with(']'),
                "Anonymous node capture name must end with ']', got: '{result}'"
            );
        }

        drop(tree);
        drop(src);
    }

    #[test]
    fn test_arc_query_shared_across_threads() {
        use crate::parser::get_language;
        use std::thread;

        let lang = get_language("rust").unwrap();
        let query = compile_query(&lang, "(function_item name: (identifier) @fn_name)").unwrap();

        let query2 = Arc::clone(&query);

        let source1 = "fn alpha() {}";
        let source2 = "fn beta() {}";

        let handle = thread::spawn(move || {
            let (tree, src) = {
                use std::io::Write;
                use tempfile::NamedTempFile;
                let mut f = NamedTempFile::new().unwrap();
                write!(f, "{}", source2).unwrap();
                crate::parser::parse_file(f.path()).unwrap()
            };
            let results = extract_matches(&tree, &src, &query2, &PathBuf::from("b.rs"));
            drop(tree);
            drop(src);
            results
        });

        let (tree1, src1) = parse_inline(source1);
        let results1 = extract_matches(&tree1, &src1, &query, &PathBuf::from("a.rs"));
        drop(tree1);
        drop(src1);

        let results2 = handle.join().expect("Thread panicked");

        assert_eq!(results1.len(), 1);
        assert_eq!(results1[0].matched_text, "alpha");
        assert_eq!(results2.len(), 1);
        assert_eq!(results2[0].matched_text, "beta");
    }
} // mod tests
