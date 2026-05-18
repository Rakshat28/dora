use std::{collections::HashSet, path::Path, sync::Arc};

use tree_sitter::{Node, Query, QueryCursor, Tree};

use crate::types::{AppError, MatchResult, Result};

#[derive(Debug)]
pub struct CompiledQuery {
    pub query: Arc<Query>,
    pub kind_ids: HashSet<u16>,
    #[allow(dead_code)]
    pub language: tree_sitter::Language,
}

fn build_kind_ids(language: &tree_sitter::Language, query_source: &str) -> HashSet<u16> {
    let count = language.node_kind_count();
    let max_id = u16::try_from(count).unwrap_or(u16::MAX);
    (0..max_id)
        .filter_map(|id| {
            language
                .node_kind_for_id(id)
                .filter(|kind| !kind.is_empty() && query_source.contains(*kind))
                .map(|_| id)
        })
        .collect()
}

#[allow(clippy::missing_errors_doc)]
pub fn compile_query(
    language: &tree_sitter::Language,
    query_source: &str,
) -> Result<Arc<CompiledQuery>> {
    let query = Query::new(language, query_source)
        .map_err(|error| AppError::QueryCompileError(error.to_string()))?;

    let kind_ids = build_kind_ids(language, query_source);

    Ok(Arc::new(CompiledQuery { query: Arc::new(query), kind_ids, language: language.clone() }))
}

#[must_use]
pub fn extract_matches(
    tree: &Tree,
    source: &str,
    compiled: &CompiledQuery,
    file_path: &Path,
) -> Vec<MatchResult> {
    let mut cursor = QueryCursor::new();
    let root_node = tree.root_node();

    let query = compiled.query.as_ref();
    let capture_names = query.capture_names();
    let mut results = Vec::new();

    for query_match in cursor.matches(query, root_node, source.as_bytes()) {
        let any_capture_matches =
            query_match.captures.iter().any(|c| compiled.kind_ids.contains(&c.node.kind_id()));

        if !any_capture_matches {
            continue;
        }

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
    use rayon::prelude::*;
    use std::path::PathBuf;

    fn parse_inline(source: &str) -> (tree_sitter::Tree, String) {
        use crate::parser::parse_file;
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut file = NamedTempFile::new().unwrap();
        write!(file, "{}", source).unwrap();
        parse_file(file.path(), &crate::parser::get_language("rust").unwrap())
            .expect("inline parse failed")
    }

    fn dummy_path() -> PathBuf {
        PathBuf::from("test_file.rs")
    }

    #[test]
    fn test_exact_fields_function_name_capture() {
        use crate::parser::get_language;

        let source = "   fn target() {}";
        let lang = get_language("rust").expect("language lookup should succeed");
        let compiled = compile_query(&lang, "(function_item name: (identifier) @fn_name)")
            .expect("query compiles");

        let (tree, src) = parse_inline(source);
        let results = extract_matches(&tree, &src, compiled.as_ref(), &dummy_path());
        // '   ' is three bytes, then 'fn ' is 3 bytes, so identifier starts at column 6
        // name length is 6 for 'target', so end_col == 12
        drop(tree);
        drop(src);

        assert_eq!(results.len(), 1, "Expected exactly one match, got: {:?}", results);

        let r = &results[0];
        assert_eq!(
            r.file_path,
            dummy_path(),
            "file_path must equal the path passed into extract_matches"
        );
        assert_eq!(r.capture_name, "fn_name", "capture must be named 'fn_name'");
        assert_eq!(r.matched_text, "target", "matched_text must be the identifier text");
        assert_eq!(r.start_line, 1, "single-line source so start_line is 1 (1-indexed)");
        assert_eq!(
            r.start_col, 6,
            "three leading spaces + 'fn ' (3 bytes) => identifier starts at column 6"
        );
        assert_eq!(r.end_line, 1, "end_line matches start_line for single-line source");
        assert_eq!(r.end_col, 12, "end_col == start_col + name.len() => 6 + 6 == 12");
    }

    #[test]
    fn test_exact_fields_eq_predicate() {
        use crate::parser::get_language;

        let source = "fn first() {}\n  fn target_fn() {}\nfn third() {}";
        let lang = get_language("rust").expect("language lookup should succeed");
        let compiled = compile_query(
            &lang,
            r#"(function_item name: (identifier) @fn_name (#eq? @fn_name "target_fn"))"#,
        )
        .expect("query compiles");

        let (tree, src) = parse_inline(source);
        let results = extract_matches(&tree, &src, compiled.as_ref(), &dummy_path());
        // drop parse artifacts per memory contract
        drop(tree);
        drop(src);

        // #eq? predicate must eliminate the non-matching functions
        assert_eq!(
            results.len(),
            1,
            "Expected exactly 1 match for #eq? predicate, got: {:?}",
            results
        );

        let r = &results[0];
        assert_eq!(r.capture_name, "fn_name", "capture must be 'fn_name'");
        assert_eq!(
            r.matched_text, "target_fn",
            "matched_text must be exactly the target function name"
        );
        // The target function is on the second line of the source string
        assert_eq!(r.start_line, 2, "target function is on line 2 in the source string");
        // two leading spaces then 'fn ' so identifier starts at column 5
        assert_eq!(r.start_col, 5, "two leading spaces + 'fn ' => identifier starts at column 5");
        assert_eq!(r.end_col, 5 + "target_fn".len(), "end_col equals start_col + name.len()");
    }

    #[test]
    fn test_exact_fields_match_predicate() {
        use crate::parser::get_language;

        let source = "fn other() {}\nfn handle_one() {}\nfn noop() {}\n    fn handle_two() {}";
        let lang = get_language("rust").expect("language lookup should succeed");
        let compiled = compile_query(
            &lang,
            r#"(function_item name: (identifier) @fn_name (#match? @fn_name "^handle_"))"#,
        )
        .expect("query compiles");

        let (tree, src) = parse_inline(source);
        let mut results = extract_matches(&tree, &src, compiled.as_ref(), &dummy_path());
        drop(tree);
        drop(src);

        // #match? applies a regex to the captured node's text.
        // Unlike #eq? which requires exact equality, #match? tests whether
        // the text matches the pattern (here anchored with ^ for prefix).
        // Sort by start_line because query match ordering is not guaranteed.
        results.sort_by_key(|r| r.start_line);

        assert_eq!(
            results.len(),
            2,
            "Expected exactly 2 matches for ^handle_ prefix, got: {:?}",
            results
        );

        // first handle_ is on line 2
        let a = &results[0];
        assert_eq!(a.capture_name, "fn_name");
        assert!(a.matched_text.starts_with("handle_"), "matched_text must start with 'handle_'");
        assert_eq!(a.start_line, 2, "the first handle_ function is on line 2");

        // second handle_ is on line 4 with four leading spaces
        let b = &results[1];
        assert_eq!(b.capture_name, "fn_name");
        assert!(b.matched_text.starts_with("handle_"), "matched_text must start with 'handle_'");
        assert_eq!(b.start_line, 4, "the second handle_ function is on line 4");
    }

    #[test]
    fn test_exact_fields_no_matches_idempotent() {
        use crate::parser::get_language;

        let source = "struct S { a: i32 }\nfn foo() {}\nlet x = 3;";
        let lang = get_language("rust").expect("language lookup should succeed");
        let compiled = compile_query(&lang, "(impl_item) @impl").expect("query compiles");

        let (tree, src) = parse_inline(source);
        let results1 = extract_matches(&tree, &src, compiled.as_ref(), &dummy_path());
        drop(tree);
        drop(src);

        assert_eq!(results1.len(), 0, "Expected zero matches for impl_item on this source");
        assert_eq!(
            results1,
            Vec::new(),
            "Must return exactly an empty Vec<MatchResult>, not just any empty collection"
        );

        let (tree2, src2) = parse_inline(source);
        let results2 = extract_matches(&tree2, &src2, compiled.as_ref(), &dummy_path());
        drop(tree2);
        drop(src2);

        assert_eq!(results2, Vec::new(), "Second call must also return an empty Vec<MatchResult>");
    }

    #[test]
    fn test_rayon_arc_query_concurrent_scope() {
        use crate::parser::get_language;
        use std::sync::Mutex;

        let lang = get_language("rust").expect("language lookup should succeed");
        let compiled = compile_query(&lang, "(function_item name: (identifier) @fn_name)")
            .expect("query compiles");

        let n = 8usize;
        let mut sources = Vec::new();
        for i in 0..n {
            let mut s = String::new();
            for j in 0..(i + 1) {
                s.push_str(&format!("fn f{}_{}() {{}}\n", i, j));
            }
            sources.push(s);
        }

        use std::sync::Arc;
        let counts = Arc::new(Mutex::new(Vec::new()));

        rayon::scope(|s| {
            for src in sources.clone() {
                let q_cl = Arc::clone(&compiled);
                let counts_cl = Arc::clone(&counts);
                s.spawn(move |_| {
                    use std::io::Write;
                    use tempfile::NamedTempFile;

                    let src_owned = src;

                    let mut f = NamedTempFile::new().expect("tempfile creation");
                    write!(f, "{}", src_owned).expect("writing tempfile");

                    let (tree, ssrc) = crate::parser::parse_file(
                        f.path(),
                        &crate::parser::get_language("rust").unwrap(),
                    )
                    .expect("parse_file");
                    let results = extract_matches(&tree, &ssrc, &*q_cl, f.path());
                    let cnt = results.len();
                    drop(tree);
                    drop(ssrc);

                    let mut guard = counts_cl.lock().expect("mutex lock");
                    guard.push(cnt);
                });
            }
        });

        let mut final_counts = counts.lock().expect("mutex lock").clone();
        final_counts.sort();
        assert_eq!(final_counts.len(), n, "All tasks must have reported their counts");
        assert_eq!(
            final_counts,
            (1..=n).collect::<Vec<usize>>(),
            "Counts must be 1..=n after sorting"
        );

        assert_eq!(
            Arc::strong_count(&compiled),
            1,
            "Arc strong_count must be 1 after scope closes"
        );
    }

    #[test]
    fn test_rayon_par_iter_mirrors_production() {
        use crate::parser::get_language;
        use std::collections::HashSet;

        let lang = get_language("rust").expect("language lookup should succeed");
        let query = compile_query(&lang, "(function_item name: (identifier) @fn_name)")
            .expect("query compiles");

        let mut temp_files = Vec::new();
        let mut paths = Vec::new();
        for i in 0..20usize {
            use std::io::Write;
            use tempfile::NamedTempFile;
            let mut f = NamedTempFile::new().expect("tempfile");
            write!(f, "fn func_{}() {{}}", i).expect("write");
            paths.push(f.path().to_path_buf());
            temp_files.push(f);
        }

        let results: Vec<crate::types::MatchResult> = paths
            .par_iter()
            .map(|p| {
                let (tree, src) =
                    crate::parser::parse_file(p, &crate::parser::get_language("rust").unwrap())
                        .expect("parse_file");
                let res = extract_matches(&tree, &src, &*query, p);
                drop(tree);
                drop(src);
                res
            })
            .flat_map_iter(|v| v.into_iter())
            .collect();

        assert_eq!(results.len(), 20, "Expected one match per file");

        for r in &results {
            assert_eq!(r.capture_name, "fn_name", "All captures must be fn_name");
            assert_eq!(r.start_line, 1, "Single-line sources => start_line == 1");
        }

        let names: HashSet<String> = results.into_iter().map(|r| r.matched_text).collect();
        let expected: HashSet<String> = (0..20usize).map(|i| format!("func_{}", i)).collect();
        assert_eq!(names, expected, "Matched names must equal expected set irrespective of order");
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

        let compiled = result.unwrap();
        assert!(
            compiled.query.capture_names().contains(&"fn_name"),
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
        let compiled = compile_query(&lang, "(identifier) @id").unwrap();

        let compiled2 = Arc::clone(&compiled);
        assert_eq!(
            compiled.query.capture_names(),
            compiled2.query.capture_names(),
            "Arc clones must share the same compiled query"
        );
        assert_eq!(Arc::strong_count(&compiled), 2);
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
        let compiled = compile_query(&lang, "(function_item name: (identifier) @fn_name)").unwrap();

        let (tree, src) = parse_inline(source);
        let results = extract_matches(&tree, &src, compiled.as_ref(), &dummy_path());
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
        let compiled =
            compile_query(&lang, "(struct_item name: (type_identifier) @struct_name)").unwrap();

        let (tree, src) = parse_inline(source);
        let results = extract_matches(&tree, &src, compiled.as_ref(), &dummy_path());
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
        let compiled = compile_query(&lang, "(function_item name: (identifier) @fn_name)").unwrap();

        let (tree, src) = parse_inline(source);
        let results = extract_matches(&tree, &src, compiled.as_ref(), &dummy_path());
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

        let compiled = compile_query(
            &lang,
            r#"
            (function_item
              name: (identifier) @fn_name
              (#eq? @fn_name "connect"))
        "#,
        )
        .unwrap();

        let (tree, src) = parse_inline(source);
        let results = extract_matches(&tree, &src, compiled.as_ref(), &dummy_path());
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
        let compiled = compile_query(&lang, "(function_item name: (identifier) @fn_name)").unwrap();

        let specific_path = PathBuf::from("src/auth/handler.rs");
        let (tree, src) = parse_inline(source);
        let results = extract_matches(&tree, &src, compiled.as_ref(), &specific_path);
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

        let compiled = compile_query(
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
        let results = extract_matches(&tree, &src, compiled.as_ref(), &dummy_path());
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
        let compiled = compile_query(&lang, "(function_item name: (identifier) @fn_name)").unwrap();

        let (tree, src) = parse_inline(source);
        let mut cursor = QueryCursor::new();
        let matches: Vec<_> =
            cursor.matches(compiled.query.as_ref(), tree.root_node(), src.as_bytes()).collect();

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

        let compiled = compile_query(&lang, r#"("fn" @keyword)"#).unwrap();

        let (tree, src) = parse_inline(source);
        let mut cursor = QueryCursor::new();
        let matches: Vec<_> =
            cursor.matches(compiled.query.as_ref(), tree.root_node(), src.as_bytes()).collect();

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
        let compiled = compile_query(&lang, "(function_item name: (identifier) @fn_name)").unwrap();

        let compiled2 = Arc::clone(&compiled);

        let source1 = "fn alpha() {}";
        let source2 = "fn beta() {}";

        let handle = thread::spawn(move || {
            let (tree, src) = {
                use std::io::Write;
                use tempfile::NamedTempFile;
                let mut f = NamedTempFile::new().unwrap();
                write!(f, "{}", source2).unwrap();
                crate::parser::parse_file(f.path(), &crate::parser::get_language("rust").unwrap())
                    .unwrap()
            };
            let results = extract_matches(&tree, &src, compiled2.as_ref(), &PathBuf::from("b.rs"));
            drop(tree);
            drop(src);
            results
        });

        let (tree1, src1) = parse_inline(source1);
        let results1 = extract_matches(&tree1, &src1, compiled.as_ref(), &PathBuf::from("a.rs"));
        drop(tree1);
        drop(src1);

        let results2 = handle.join().expect("Thread panicked");

        assert_eq!(results1.len(), 1);
        assert_eq!(results1[0].matched_text, "alpha");
        assert_eq!(results2.len(), 1);
        assert_eq!(results2[0].matched_text, "beta");
    }

    #[test]
    fn test_compiled_query_has_non_empty_kind_ids() {
        use crate::parser::get_language;
        let lang = get_language("rust").unwrap();
        let compiled = compile_query(&lang, "(function_item name: (identifier) @fn_name)").unwrap();
        assert!(!compiled.kind_ids.is_empty());
    }

    #[test]
    fn test_kind_ids_contains_function_item() {
        use crate::parser::get_language;
        let lang = get_language("rust").unwrap();
        let compiled = compile_query(&lang, "(function_item name: (identifier) @fn_name)").unwrap();
        let fn_item_id = lang.id_for_node_kind("function_item", true);
        assert!(compiled.kind_ids.contains(&fn_item_id));
    }

    #[test]
    fn test_kind_ids_contains_identifier() {
        use crate::parser::get_language;
        let lang = get_language("rust").unwrap();
        let compiled = compile_query(&lang, "(function_item name: (identifier) @fn_name)").unwrap();
        let id_id = lang.id_for_node_kind("identifier", true);
        assert!(compiled.kind_ids.contains(&id_id));
    }

    #[test]
    fn test_extract_matches_correct_with_kind_filter() {
        use crate::parser::{get_language, parse_file};
        use std::io::Write;
        use tempfile::NamedTempFile;

        let source = "fn alpha() {}\nfn beta() {}";
        let lang = get_language("rust").unwrap();
        let compiled = compile_query(&lang, "(function_item name: (identifier) @fn_name)").unwrap();
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "{}", source).unwrap();
        let (tree, src) = parse_file(file.path(), &lang).unwrap();
        let results = extract_matches(&tree, &src, &compiled, file.path());
        drop(tree);
        drop(src);
        assert_eq!(results.len(), 2);
        let names: Vec<&str> = results.iter().map(|r| r.matched_text.as_str()).collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
    }

    #[test]
    fn test_kind_filter_preserves_eq_predicate_results() {
        use crate::parser::{get_language, parse_file};
        use std::io::Write;
        use tempfile::NamedTempFile;

        let source = "fn connect() {}\nfn disconnect() {}";
        let lang = get_language("rust").unwrap();
        let compiled = compile_query(
            &lang,
            r#"(function_item name: (identifier) @fn_name (#eq? @fn_name "connect"))"#,
        )
        .unwrap();
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "{}", source).unwrap();
        let (tree, src) = parse_file(file.path(), &lang).unwrap();
        let results = extract_matches(&tree, &src, &compiled, file.path());
        drop(tree);
        drop(src);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].matched_text, "connect");
    }

    #[test]
    fn test_build_kind_ids_is_superset() {
        use crate::parser::{get_language, parse_file};
        use std::io::Write;
        use tempfile::NamedTempFile;

        let source = "fn foo() { let x = 1; }";
        let lang = get_language("rust").unwrap();
        let compiled = compile_query(&lang, "(function_item name: (identifier) @fn_name)").unwrap();
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "{}", source).unwrap();
        let (tree, src) = parse_file(file.path(), &lang).unwrap();
        let results = extract_matches(&tree, &src, &compiled, file.path());
        drop(tree);
        drop(src);
        for result in &results {
            let node_kind_name = result.matched_text.as_str();
            assert_eq!(node_kind_name, "foo");
        }
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_compiled_query_language_field() {
        use crate::parser::get_language;
        let rust_lang = get_language("rust").unwrap();
        let compiled = compile_query(&rust_lang, "(identifier) @id").unwrap();
        assert_eq!(compiled.language.node_kind_count(), rust_lang.node_kind_count());
    }
} // mod tests
