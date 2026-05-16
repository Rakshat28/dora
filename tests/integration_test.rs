use ast_search::parser::{get_language, parse_file};
use ast_search::query::{compile_query, extract_matches};
use ast_search::types::{Language, MatchResult};
use ast_search::walker::build_walker;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

fn run_pipeline(fixture_dir: &Path, query_str: &str) -> Vec<MatchResult> {
    let lang_str = "rust";
    let ts_lang = get_language(lang_str).unwrap();
    let query = compile_query(&ts_lang, query_str).unwrap();
    let results = Arc::new(Mutex::new(Vec::<MatchResult>::new()));
    let results_ref = Arc::clone(&results);
    let query_ref = Arc::clone(&query);

    build_walker(fixture_dir, &Language::Rust).for_each(|entry_result| {
        let entry = match entry_result {
            Ok(e) => e,
            Err(_) => return,
        };
        let (tree, source) = match parse_file(entry.path()) {
            Ok(pair) => pair,
            Err(_) => return,
        };
        let mut matches = extract_matches(&tree, &source, query_ref.as_ref(), entry.path());
        drop(tree);
        drop(source);
        if !matches.is_empty() {
            results_ref.lock().unwrap().append(&mut matches);
        }
    });

    drop(results_ref);
    drop(query_ref);

    let mut final_results = Arc::try_unwrap(results).unwrap().into_inner().unwrap();
    final_results.sort();
    final_results.dedup();
    final_results
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

#[test]
fn test_single_function_capture() {
    let query = "(function_item name: (identifier) @fn_name)";
    let results = run_pipeline(&fixtures_dir(), query);

    let simple_results: Vec<_> =
        results.iter().filter(|r| r.file_path == fixtures_dir().join("simple.rs")).collect();

    assert_eq!(simple_results.len(), 1);
    assert_eq!(simple_results[0].capture_name, "fn_name");
    assert_eq!(simple_results[0].matched_text, "add");
    assert_eq!(simple_results[0].start_line, 1);
    assert_eq!(simple_results[0].start_col, 3);
    assert_eq!(simple_results[0].end_line, 1);
    assert_eq!(simple_results[0].end_col, 6);
}

#[test]
fn test_multiple_functions_sorted_by_line() {
    let query = "(function_item name: (identifier) @fn_name)";
    let results = run_pipeline(&fixtures_dir(), query);

    let multi_results: Vec<_> =
        results.iter().filter(|r| r.file_path == fixtures_dir().join("multi_fn.rs")).collect();

    assert_eq!(multi_results.len(), 3);

    assert_eq!(multi_results[0].matched_text, "alpha");
    assert_eq!(multi_results[0].start_line, 1);
    assert_eq!(multi_results[0].start_col, 3);

    assert_eq!(multi_results[1].matched_text, "beta");
    assert_eq!(multi_results[1].start_line, 3);
    assert_eq!(multi_results[1].start_col, 3);

    assert_eq!(multi_results[2].matched_text, "gamma");
    assert_eq!(multi_results[2].start_line, 5);
    assert_eq!(multi_results[2].start_col, 3);
}

#[test]
fn test_struct_name_capture() {
    let query = "(struct_item name: (type_identifier) @struct_name)";
    let results = run_pipeline(&fixtures_dir(), query);

    let struct_results: Vec<_> =
        results.iter().filter(|r| r.file_path == fixtures_dir().join("structs.rs")).collect();

    assert_eq!(struct_results.len(), 2);

    assert_eq!(struct_results[0].matched_text, "Point");
    assert_eq!(struct_results[0].start_line, 1);
    assert_eq!(struct_results[0].start_col, 7);
    assert_eq!(struct_results[0].capture_name, "struct_name");

    assert_eq!(struct_results[1].matched_text, "Color");
    assert_eq!(struct_results[1].start_line, 6);
    assert_eq!(struct_results[1].start_col, 7);
    assert_eq!(struct_results[1].capture_name, "struct_name");
}

#[test]
fn test_nested_module_functions() {
    let query = "(function_item name: (identifier) @fn_name)";
    let results = run_pipeline(&fixtures_dir(), query);

    let nested_results: Vec<_> =
        results.iter().filter(|r| r.file_path == fixtures_dir().join("nested.rs")).collect();

    assert_eq!(nested_results.len(), 3);

    let texts: HashSet<_> = nested_results.iter().map(|r| r.matched_text.as_str()).collect();
    assert!(texts.contains("inner_one"));
    assert!(texts.contains("inner_two"));
    assert!(texts.contains("deepest"));

    let deepest = nested_results.iter().find(|r| r.matched_text == "deepest").unwrap();
    let inner_lines: Vec<_> = nested_results
        .iter()
        .filter(|r| r.matched_text != "deepest")
        .map(|r| r.start_line)
        .collect();

    assert!(deepest.start_line > inner_lines[0]);
    assert!(deepest.start_line > inner_lines[1]);
}

#[test]
fn test_empty_file_produces_no_results() {
    let temp_dir = TempDir::new().unwrap();
    let empty_fixture = fixtures_dir().join("empty.rs");
    let temp_file = temp_dir.path().join("empty.rs");
    std::fs::copy(empty_fixture, temp_file).unwrap();

    let query = "(function_item name: (identifier) @fn_name)";
    let results = run_pipeline(temp_dir.path(), query);

    assert!(results.is_empty());
}

#[test]
fn test_eq_predicate_filters_to_exact_match() {
    let query = r#"(function_item name: (identifier) @fn_name (#eq? @fn_name "beta"))"#;
    let results = run_pipeline(&fixtures_dir(), query);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].matched_text, "beta");
    assert_eq!(results[0].file_path, fixtures_dir().join("multi_fn.rs"));
    assert_eq!(results[0].start_line, 3);
}

#[test]
fn test_match_predicate_regex_filter() {
    let query =
        r#"(function_item name: (identifier) @fn_name (#match? @fn_name "^(alpha|gamma)$"))"#;
    let results = run_pipeline(&fixtures_dir(), query);

    let texts: HashSet<_> = results.iter().map(|r| r.matched_text.as_str()).collect();
    assert_eq!(texts, HashSet::from(["alpha", "gamma"]));

    assert_eq!(results.len(), 2);

    for result in &results {
        assert_eq!(result.file_path, fixtures_dir().join("multi_fn.rs"));
    }
}

#[test]
fn test_query_with_no_matches_returns_empty() {
    let query = r#"(struct_item name: (type_identifier) @s (#eq? @s "DoesNotExistXyz999"))"#;
    let results = run_pipeline(&fixtures_dir(), query);

    assert!(results.is_empty());
    assert_eq!(results, Vec::<MatchResult>::new());
}

#[test]
fn test_pipeline_results_are_deterministic() {
    let query = "(function_item name: (identifier) @fn_name)";

    let run1 = run_pipeline(&fixtures_dir(), query);
    let run2 = run_pipeline(&fixtures_dir(), query);

    assert_eq!(run1, run2);
    assert_eq!(run1.len(), run2.len());
}

#[test]
fn test_results_sorted_by_file_then_line() {
    let query = "(function_item name: (identifier) @fn_name)";
    let results = run_pipeline(&fixtures_dir(), query);

    for window in results.windows(2) {
        let curr = &window[0];
        let next = &window[1];

        if curr.file_path == next.file_path {
            assert!(curr.start_line <= next.start_line);
        } else {
            assert!(curr.file_path <= next.file_path);
        }
    }
}

#[test]
fn test_walker_filters_non_rust_extensions() {
    let temp_dir = TempDir::new().unwrap();

    std::fs::write(temp_dir.path().join("code.rs"), "fn rust_fn() {}").unwrap();
    std::fs::write(temp_dir.path().join("script.py"), "def py_fn(): pass").unwrap();
    std::fs::write(temp_dir.path().join("readme.txt"), "fn fake_fn() {}").unwrap();

    let query = "(function_item name: (identifier) @fn_name)";
    let results = run_pipeline(temp_dir.path(), query);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].matched_text, "rust_fn");

    for result in &results {
        assert!(!result.file_path.to_string_lossy().contains("script.py"));
        assert!(!result.file_path.to_string_lossy().contains("readme.txt"));
    }
}

#[test]
fn test_results_contain_absolute_file_paths() {
    let query = "(function_item name: (identifier) @fn_name)";
    let results = run_pipeline(&fixtures_dir(), query);

    for result in &results {
        assert!(result.file_path.is_absolute());
        assert!(result.file_path.exists());
        assert_eq!(result.file_path.extension().unwrap(), "rs");
    }
}

#[test]
fn test_multiple_captures_per_match() {
    let query = r#"(function_item
name: (identifier) @fn_name
parameters: (parameters
(parameter pattern: (identifier) @param_name)))"#;
    let results = run_pipeline(&fixtures_dir(), query);

    let simple_results: Vec<_> =
        results.iter().filter(|r| r.file_path == fixtures_dir().join("simple.rs")).collect();

    let fn_name_results: Vec<_> =
        simple_results.iter().filter(|r| r.capture_name == "fn_name").collect();
    assert!(!fn_name_results.is_empty());
    assert!(fn_name_results.iter().any(|r| r.matched_text == "add"));

    let param_results: Vec<_> =
        simple_results.iter().filter(|r| r.capture_name == "param_name").collect();
    assert!(param_results.iter().any(|r| r.matched_text == "a"));
    assert!(param_results.iter().any(|r| r.matched_text == "b"));
}

#[test]
fn test_invalid_query_compile_error() {
    let ts_lang = get_language("rust").unwrap();
    let result = compile_query(&ts_lang, "((( invalid");

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), ast_search::types::AppError::QueryCompileError(_)));
}

#[test]
fn test_all_fixture_functions_found() {
    let query = "(function_item name: (identifier) @fn_name)";
    let results = run_pipeline(&fixtures_dir(), query);

    let texts: HashSet<_> = results.iter().map(|r| r.matched_text.as_str()).collect();

    assert!(texts.contains("add"));
    assert!(texts.contains("alpha"));
    assert!(texts.contains("beta"));
    assert!(texts.contains("gamma"));
    assert!(texts.contains("inner_one"));
    assert!(texts.contains("inner_two"));
    assert!(texts.contains("deepest"));

    assert!(results.len() >= 7);
}

#[test]
fn test_invalid_lang_error_contains_hint() {
    let supported = ["rust", "python", "js", "ts", "go"];
    let invalid = "haskell";
    assert!(!supported.contains(&invalid));
}

#[test]
fn test_validate_error_ordering() {
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        ("", "/nonexistent", "cobol", "query must not be empty"),
        ("(f)", "/nonexistent", "cobol", "does not exist"),
    ];

    for (query, path, lang, expected_fragment) in cases {
        let result = validate_inputs(query, path, lang);
        assert!(result.is_err(), "expected error for query={query} path={path} lang={lang}");
        assert!(
            result.unwrap_err().contains(expected_fragment),
            "expected fragment '{}' for query={} path={} lang={}",
            expected_fragment,
            query,
            path,
            lang
        );
    }
}

fn validate_inputs(query: &str, path: &str, lang: &str) -> Result<(), String> {
    use std::path::PathBuf;

    if query.trim().is_empty() {
        return Err("query must not be empty".to_string());
    }
    let p = PathBuf::from(path);
    if !p.exists() {
        return Err(format!(
            "path does not exist: {}\n  hint: check for typos or run from the correct directory",
            p.display()
        ));
    }
    if !p.is_dir() {
        return Err(format!(
            "path is not a directory: {}\n  hint: --path must point to a directory, not a file",
            p.display()
        ));
    }
    let supported = ["rust", "python", "js", "ts", "go"];
    if !supported.contains(&lang) {
        return Err(format!(
            "unsupported language: '{}'\n  supported languages: rust, python, js, ts, go\n  example: --lang rust",
            lang
        ));
    }
    Ok(())
}
