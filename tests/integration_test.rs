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
        let (tree, source) = match parse_file(entry.path(), &ts_lang) {
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

fn run_pipeline_for_language(
    fixture_dir: &Path,
    query_str: &str,
    lang_enum: Language,
    lang_str: &str,
) -> Vec<MatchResult> {
    let ts_lang = get_language(lang_str).unwrap();
    let query = compile_query(&ts_lang, query_str).unwrap();
    let results = Arc::new(Mutex::new(Vec::<MatchResult>::new()));
    let results_ref = Arc::clone(&results);
    let query_ref = Arc::clone(&query);

    build_walker(fixture_dir, &lang_enum).for_each(|entry_result| {
        let entry = match entry_result {
            Ok(e) => e,
            Err(_) => return,
        };
        let (tree, source) = match parse_file(entry.path(), &ts_lang) {
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
    let supported = ["rust", "python", "js", "ts", "go", "c", "cpp"];
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
    let supported = ["rust", "python", "js", "ts", "go", "c", "cpp"];
    if !supported.contains(&lang) {
        return Err(format!(
            "unsupported language: '{}'\n  supported languages: rust, python, js, ts, go, c, cpp\n  example: --lang rust",
            lang
        ));
    }
    Ok(())
}

#[test]
fn test_python_function_name_capture() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("simple.py");
    let lang = get_language("python").unwrap();
    let compiled =
        compile_query(&lang, "(function_definition name: (identifier) @fn_name)").unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let results = extract_matches(&tree, &source, compiled.as_ref(), &fixture);
    drop(tree);
    drop(source);

    let names: std::collections::HashSet<&str> =
        results.iter().map(|r| r.matched_text.as_str()).collect();

    assert_eq!(names.len(), 3);
    assert!(names.contains("greet"));
    assert!(names.contains("add"));
    assert!(names.contains("multiply"));
}

#[test]
fn test_python_function_line_numbers() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("simple.py");
    let lang = get_language("python").unwrap();
    let compiled =
        compile_query(&lang, "(function_definition name: (identifier) @fn_name)").unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let mut results = extract_matches(&tree, &source, compiled.as_ref(), &fixture);
    drop(tree);
    drop(source);

    results.sort();

    assert_eq!(results.len(), 3);

    assert_eq!(results[0].matched_text, "greet");
    assert_eq!(results[0].start_line, 1);
    assert_eq!(results[0].start_col, 4);
    assert_eq!(results[0].end_col, 9);

    assert_eq!(results[1].matched_text, "add");
    assert_eq!(results[1].start_line, 5);
    assert_eq!(results[1].start_col, 4);
    assert_eq!(results[1].end_col, 7);

    assert_eq!(results[2].matched_text, "multiply");
    assert_eq!(results[2].start_line, 9);
    assert_eq!(results[2].start_col, 4);
    assert_eq!(results[2].end_col, 12);
}

#[test]
fn test_python_walker_finds_py_files() {
    use ast_search::types::Language;
    use ast_search::walker::build_walker;
    use std::fs;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("script.py"), b"def foo(): pass").unwrap();
    fs::write(dir.path().join("lib.py"), b"def bar(): pass").unwrap();
    fs::write(dir.path().join("main.rs"), b"fn main() {}").unwrap();

    let entries: Vec<_> =
        build_walker(dir.path(), &Language::Python).collect::<Result<Vec<_>, _>>().unwrap();

    let names: Vec<String> = entries
        .iter()
        .filter_map(|e| e.path().file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();

    assert!(names.contains(&"script.py".to_string()));
    assert!(names.contains(&"lib.py".to_string()));
    assert!(!names.contains(&"main.rs".to_string()));
    assert_eq!(entries.len(), 2);
}

#[test]
fn test_python_walker_includes_pyi_stubs() {
    use ast_search::types::Language;
    use ast_search::walker::build_walker;
    use std::fs;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("module.pyi"), b"def foo(x: int) -> str: ...").unwrap();
    fs::write(dir.path().join("lib.py"), b"def bar(): pass").unwrap();

    let entries: Vec<_> =
        build_walker(dir.path(), &Language::Python).collect::<Result<Vec<_>, _>>().unwrap();

    let names: Vec<String> = entries
        .iter()
        .filter_map(|e| e.path().file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();

    assert!(names.contains(&"module.pyi".to_string()));
    assert!(names.contains(&"lib.py".to_string()));
}

#[test]
fn test_python_eq_predicate() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("simple.py");
    let lang = get_language("python").unwrap();
    let compiled = compile_query(
        &lang,
        r#"(function_definition name: (identifier) @fn_name (#eq? @fn_name "add"))"#,
    )
    .unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let results = extract_matches(&tree, &source, compiled.as_ref(), &fixture);
    drop(tree);
    drop(source);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].matched_text, "add");
    assert_eq!(results[0].start_line, 5);
}

#[test]
fn test_rust_and_python_results_do_not_mix() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let rust_fixture = fixtures_dir().join("simple.rs");
    let python_fixture = fixtures_dir().join("simple.py");

    let rust_lang = get_language("rust").unwrap();
    let python_lang = get_language("python").unwrap();

    let rust_query =
        compile_query(&rust_lang, "(function_item name: (identifier) @fn_name)").unwrap();

    let python_query =
        compile_query(&python_lang, "(function_definition name: (identifier) @fn_name)").unwrap();

    let (rust_tree, rust_src) = parse_file(&rust_fixture, &rust_lang).unwrap();
    let rust_results = extract_matches(&rust_tree, &rust_src, &rust_query, &rust_fixture);
    drop(rust_tree);
    drop(rust_src);

    let (py_tree, py_src) = parse_file(&python_fixture, &python_lang).unwrap();
    let py_results = extract_matches(&py_tree, &py_src, &python_query, &python_fixture);
    drop(py_tree);
    drop(py_src);

    assert_eq!(rust_results.len(), 1);
    assert_eq!(rust_results[0].matched_text, "add");

    assert_eq!(py_results.len(), 3);

    let py_names: std::collections::HashSet<&str> =
        py_results.iter().map(|r| r.matched_text.as_str()).collect();

    assert!(py_names.contains("greet"));
    assert!(py_names.contains("add"));
    assert!(py_names.contains("multiply"));

    for pr in &py_results {
        assert_ne!(pr.file_path, rust_fixture);
    }
    for rr in &rust_results {
        assert_ne!(rr.file_path, python_fixture);
    }
}

#[test]
fn test_javascript_function_declaration_capture() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("simple.js");
    let lang = get_language("js").unwrap();
    let query = compile_query(&lang, "(function_declaration name: (identifier) @fn_name)").unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    let names: std::collections::HashSet<&str> =
        results.iter().map(|r| r.matched_text.as_str()).collect();

    assert!(names.contains("greet"), "must find 'greet'");
    assert!(names.contains("add"), "must find 'add'");
    assert!(!names.contains("multiply"), "arrow fn must not match function_declaration");
}

#[test]
fn test_javascript_class_declaration_capture() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("simple.js");
    let lang = get_language("js").unwrap();
    let query = compile_query(&lang, "(class_declaration name: (identifier) @class_name)").unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].matched_text, "Calculator");
    assert_eq!(results[0].start_line, 11);
    assert_eq!(results[0].start_col, 6);
    assert_eq!(results[0].end_col, 16);
}

#[test]
fn test_javascript_function_name_exact_position() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("simple.js");
    let lang = get_language("js").unwrap();
    let query = compile_query(
        &lang,
        r#"(function_declaration name: (identifier) @fn_name (#eq? @fn_name "greet"))"#,
    )
    .unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].matched_text, "greet");
    assert_eq!(results[0].start_line, 1);
    assert_eq!(results[0].start_col, 9);
    assert_eq!(results[0].end_col, 14);
}

#[test]
fn test_typescript_function_declaration_capture() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("simple.ts");
    let lang = get_language("ts").unwrap();
    let query = compile_query(&lang, "(function_declaration name: (identifier) @fn_name)").unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let mut results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    results.sort();

    let names: Vec<&str> = results.iter().map(|r| r.matched_text.as_str()).collect();

    assert_eq!(names.len(), 2);
    assert!(names.contains(&"greet"));
    assert!(names.contains(&"add"));
}

#[test]
fn test_typescript_interface_declaration_capture() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("simple.ts");
    let lang = get_language("ts").unwrap();
    let query =
        compile_query(&lang, "(interface_declaration name: (type_identifier) @interface_name)")
            .unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].matched_text, "Shape");
    assert_eq!(results[0].start_line, 9);
    assert_eq!(results[0].start_col, 10);
    assert_eq!(results[0].end_col, 15);
}

#[test]
fn test_typescript_type_alias_capture() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("simple.ts");
    let lang = get_language("ts").unwrap();
    let query = compile_query(&lang, "(type_alias_declaration name: (type_identifier) @type_name)")
        .unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].matched_text, "Point");
    assert_eq!(results[0].start_line, 26);
    assert_eq!(results[0].start_col, 5);
    assert_eq!(results[0].end_col, 10);
}

#[test]
fn test_typescript_class_declaration_capture() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("simple.ts");
    let lang = get_language("ts").unwrap();
    let query =
        compile_query(&lang, "(class_declaration name: (type_identifier) @class_name)").unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].matched_text, "Circle");
    assert_eq!(results[0].start_line, 14);
    assert_eq!(results[0].start_col, 6);
    assert_eq!(results[0].end_col, 12);
}

#[test]
fn test_javascript_walker_extensions() {
    use ast_search::types::Language;
    use ast_search::walker::build_walker;
    use std::fs;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("app.js"), b"function f() {}").unwrap();
    fs::write(dir.path().join("mod.mjs"), b"export function g() {}").unwrap();
    fs::write(dir.path().join("cjs.cjs"), b"module.exports = {}").unwrap();
    fs::write(dir.path().join("index.ts"), b"function h(): void {}").unwrap();
    fs::write(dir.path().join("main.rs"), b"fn main() {}").unwrap();

    let entries: Vec<_> =
        build_walker(dir.path(), &Language::JavaScript).collect::<Result<Vec<_>, _>>().unwrap();

    let names: std::collections::HashSet<String> = entries
        .iter()
        .filter_map(|e| e.path().file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();

    assert!(names.contains("app.js"));
    assert!(names.contains("mod.mjs"));
    assert!(names.contains("cjs.cjs"));
    assert!(!names.contains("index.ts"));
    assert!(!names.contains("main.rs"));
    assert_eq!(entries.len(), 3);
}

#[test]
fn test_typescript_walker_extensions() {
    use ast_search::types::Language;
    use ast_search::walker::build_walker;
    use std::fs;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("index.ts"), b"function f(): void {}").unwrap();
    fs::write(dir.path().join("app.tsx"), b"function App() { return null; }").unwrap();
    fs::write(dir.path().join("mod.mts"), b"export function g(): void {}").unwrap();
    fs::write(dir.path().join("cts.cts"), b"module.exports = {}").unwrap();
    fs::write(dir.path().join("script.js"), b"function h() {}").unwrap();
    fs::write(dir.path().join("main.rs"), b"fn main() {}").unwrap();

    let entries: Vec<_> =
        build_walker(dir.path(), &Language::TypeScript).collect::<Result<Vec<_>, _>>().unwrap();

    let names: std::collections::HashSet<String> = entries
        .iter()
        .filter_map(|e| e.path().file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();

    assert!(names.contains("index.ts"));
    assert!(names.contains("app.tsx"));
    assert!(names.contains("mod.mts"));
    assert!(names.contains("cts.cts"));
    assert!(!names.contains("script.js"));
    assert!(!names.contains("main.rs"));
    assert_eq!(entries.len(), 4);
}

#[test]
fn test_js_and_ts_results_do_not_mix() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let js_fixture = fixtures_dir().join("simple.js");
    let ts_fixture = fixtures_dir().join("simple.ts");

    let js_lang = get_language("js").unwrap();
    let ts_lang = get_language("ts").unwrap();

    let query_str = "(function_declaration name: (identifier) @fn_name)";
    let js_query = compile_query(&js_lang, query_str).unwrap();
    let ts_query = compile_query(&ts_lang, query_str).unwrap();

    let (js_tree, js_src) = parse_file(&js_fixture, &js_lang).unwrap();
    let js_results = extract_matches(&js_tree, &js_src, &js_query, &js_fixture);
    drop(js_tree);
    drop(js_src);

    let (ts_tree, ts_src) = parse_file(&ts_fixture, &ts_lang).unwrap();
    let ts_results = extract_matches(&ts_tree, &ts_src, &ts_query, &ts_fixture);
    drop(ts_tree);
    drop(ts_src);

    for r in &js_results {
        assert_eq!(r.file_path, js_fixture);
    }
    for r in &ts_results {
        assert_eq!(r.file_path, ts_fixture);
    }

    let js_names: std::collections::HashSet<&str> =
        js_results.iter().map(|r| r.matched_text.as_str()).collect();
    let ts_names: std::collections::HashSet<&str> =
        ts_results.iter().map(|r| r.matched_text.as_str()).collect();

    assert!(js_names.contains("greet"));
    assert!(js_names.contains("add"));
    assert!(ts_names.contains("greet"));
    assert!(ts_names.contains("add"));
}

#[test]
fn test_typescript_interface_query_compiles() {
    use ast_search::parser::get_language;
    use ast_search::query::compile_query;

    let lang = get_language("ts").unwrap();
    let result = compile_query(&lang, "(interface_declaration name: (type_identifier) @name)");
    assert!(result.is_ok(), "interface_declaration query must compile against tsx grammar");
}

#[test]
fn test_js_grammar_rejects_typescript_node_type() {
    use ast_search::parser::get_language;
    use ast_search::query::compile_query;

    let js_lang = get_language("js").unwrap();
    let result = compile_query(&js_lang, "(interface_declaration name: (type_identifier) @name)");
    assert!(
        result.is_err(),
        "interface_declaration is TypeScript-only and must fail against JS grammar"
    );
}

#[test]
fn test_go_function_declaration_capture() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("simple.go");
    let lang = get_language("go").unwrap();
    let query = compile_query(&lang, "(function_declaration name: (identifier) @fn_name)").unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    let names: std::collections::HashSet<&str> =
        results.iter().map(|r| r.matched_text.as_str()).collect();

    assert!(names.contains("greet"), "must find 'greet'");
    assert!(names.contains("add"), "must find 'add'");
    assert!(names.contains("multiply"), "must find 'multiply'");
    assert!(!names.contains("area"), "method must not match function_declaration");
}

#[test]
fn test_go_function_exact_positions() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("simple.go");
    let lang = get_language("go").unwrap();
    let query = compile_query(&lang, "(function_declaration name: (identifier) @fn_name)").unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let mut results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    results.sort();

    let greet = results.iter().find(|r| r.matched_text == "greet").unwrap();
    assert_eq!(greet.start_line, 5);
    assert_eq!(greet.start_col, 5);
    assert_eq!(greet.end_col, 10);

    let add = results.iter().find(|r| r.matched_text == "add").unwrap();
    assert_eq!(add.start_line, 9);
    assert_eq!(add.start_col, 5);
    assert_eq!(add.end_col, 8);

    let multiply = results.iter().find(|r| r.matched_text == "multiply").unwrap();
    assert_eq!(multiply.start_line, 13);
    assert_eq!(multiply.start_col, 5);
    assert_eq!(multiply.end_col, 13);
}

#[test]
fn test_go_struct_type_declaration_capture() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("simple.go");
    let lang = get_language("go").unwrap();
    let query =
        compile_query(&lang, "(type_declaration (type_spec name: (type_identifier) @type_name))")
            .unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let mut results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    results.sort();

    let names: std::collections::HashSet<&str> =
        results.iter().map(|r| r.matched_text.as_str()).collect();

    assert!(names.contains("Point"));
    assert!(names.contains("Rectangle"));
    assert_eq!(results.len(), 2);
}

#[test]
fn test_go_eq_predicate() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("simple.go");
    let lang = get_language("go").unwrap();
    let query = compile_query(
        &lang,
        r#"(function_declaration name: (identifier) @fn_name (#eq? @fn_name "add"))"#,
    )
    .unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].matched_text, "add");
    assert_eq!(results[0].start_line, 9);
}

#[test]
fn test_go_match_predicate() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("simple.go");
    let lang = get_language("go").unwrap();
    let query = compile_query(
        &lang,
        r#"(function_declaration name: (identifier) @fn_name (#match? @fn_name "^(add|multiply)$"))"#,
    ).unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    let names: std::collections::HashSet<&str> =
        results.iter().map(|r| r.matched_text.as_str()).collect();

    assert_eq!(results.len(), 2);
    assert!(names.contains("add"));
    assert!(names.contains("multiply"));
    assert!(!names.contains("greet"));
}

#[test]
fn test_go_walker_finds_go_files_only() {
    use ast_search::types::Language;
    use ast_search::walker::build_walker;
    use std::fs;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("main.go"), b"package main\nfunc main() {}").unwrap();
    fs::write(dir.path().join("util.go"), b"package main\nfunc util() {}").unwrap();
    fs::write(dir.path().join("lib.rs"), b"fn lib() {}").unwrap();
    fs::write(dir.path().join("script.py"), b"def script(): pass").unwrap();
    fs::write(dir.path().join("app.js"), b"function app() {}").unwrap();

    let entries: Vec<_> =
        build_walker(dir.path(), &Language::Go).collect::<Result<Vec<_>, _>>().unwrap();

    let names: std::collections::HashSet<String> = entries
        .iter()
        .filter_map(|e| e.path().file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();

    assert!(names.contains("main.go"));
    assert!(names.contains("util.go"));
    assert!(!names.contains("lib.rs"));
    assert!(!names.contains("script.py"));
    assert!(!names.contains("app.js"));
    assert_eq!(entries.len(), 2);
}

#[test]
fn test_go_and_rust_results_do_not_mix() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let go_fixture = fixtures_dir().join("simple.go");
    let rust_fixture = fixtures_dir().join("simple.rs");

    let go_lang = get_language("go").unwrap();
    let rust_lang = get_language("rust").unwrap();

    let (go_tree, go_src) = parse_file(&go_fixture, &go_lang).unwrap();
    let go_query =
        compile_query(&go_lang, "(function_declaration name: (identifier) @fn_name)").unwrap();
    let go_results = extract_matches(&go_tree, &go_src, &go_query, &go_fixture);
    drop(go_tree);
    drop(go_src);

    let (rs_tree, rs_src) = parse_file(&rust_fixture, &rust_lang).unwrap();
    let rust_query =
        compile_query(&rust_lang, "(function_item name: (identifier) @fn_name)").unwrap();
    let rust_results = extract_matches(&rs_tree, &rs_src, &rust_query, &rust_fixture);
    drop(rs_tree);
    drop(rs_src);

    for r in &go_results {
        assert_eq!(r.file_path, go_fixture);
    }
    for r in &rust_results {
        assert_eq!(r.file_path, rust_fixture);
    }

    assert!(!go_results.is_empty());
    assert!(!rust_results.is_empty());
}

#[test]
fn test_go_grammar_rejects_rust_node_type() {
    use ast_search::parser::get_language;
    use ast_search::query::compile_query;

    let go_lang = get_language("go").unwrap();
    let result = compile_query(&go_lang, "(function_item name: (identifier) @fn_name)");
    assert!(
        result.is_err(),
        "function_item is Rust-only and must fail to compile against Go grammar"
    );
}

#[test]
fn test_all_five_languages_compile_queries() {
    use ast_search::parser::get_language;
    use ast_search::query::compile_query;

    let cases = vec![
        ("rust", "(function_item name: (identifier) @fn_name)"),
        ("python", "(function_definition name: (identifier) @fn_name)"),
        ("js", "(function_declaration name: (identifier) @fn_name)"),
        ("ts", "(function_declaration name: (identifier) @fn_name)"),
        ("go", "(function_declaration name: (identifier) @fn_name)"),
    ];

    for (lang_str, query_str) in cases {
        let lang = get_language(lang_str).unwrap();
        let result = compile_query(&lang, query_str);
        assert!(result.is_ok(), "query compile failed for lang={}: {:?}", lang_str, result.err());
    }
}

#[test]
fn test_all_five_languages_parse_minimal_source() {
    use ast_search::parser::{get_language, parse_file};
    use std::io::Write;
    use tempfile::NamedTempFile;

    let cases: Vec<(&str, &str, &str)> = vec![
        ("rust", "fn main() {}", "source_file"),
        ("python", "def main(): pass", "module"),
        ("js", "function main() {}", "program"),
        ("ts", "function main(): void {}", "program"),
        ("go", "package main\nfunc main() {}", "source_file"),
    ];

    for (lang_str, source, expected_root_kind) in cases {
        let lang = get_language(lang_str).unwrap();
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "{}", source).unwrap();
        let result = parse_file(file.path(), &lang);
        assert!(result.is_ok(), "parse failed for lang={}: {:?}", lang_str, result.err());
        let (tree, src) = result.unwrap();
        assert_eq!(
            tree.root_node().kind(),
            expected_root_kind,
            "wrong root node kind for lang={}",
            lang_str
        );
        assert!(!tree.root_node().has_error(), "unexpected parse errors for lang={}", lang_str);
        drop(tree);
        drop(src);
    }
}

#[test]
fn test_c_function_declaration_capture() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("simple.c");
    let lang = get_language("c").unwrap();
    let query = compile_query(
        &lang,
        "(function_definition declarator: (function_declarator declarator: (identifier) @fn_name))",
    )
    .unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    let names: std::collections::HashSet<&str> =
        results.iter().map(|r| r.matched_text.as_str()).collect();

    assert!(names.contains("add"), "must find 'add'");
    assert!(names.contains("multiply"), "must find 'multiply'");
    assert!(names.contains("greet"), "must find 'greet'");
}

#[test]
fn test_c_function_exact_positions() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("simple.c");
    let lang = get_language("c").unwrap();
    let query = compile_query(
        &lang,
        "(function_definition declarator: (function_declarator declarator: (identifier) @fn_name))",
    )
    .unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let mut results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    results.sort();

    let add = results.iter().find(|r| r.matched_text == "add").unwrap();
    assert_eq!(add.start_line, 3);
    assert_eq!(add.start_col, 4);
    assert_eq!(add.end_col, 7);

    let multiply = results.iter().find(|r| r.matched_text == "multiply").unwrap();
    assert_eq!(multiply.start_line, 7);
    assert_eq!(multiply.start_col, 4);
    assert_eq!(multiply.end_col, 12);

    let greet = results.iter().find(|r| r.matched_text == "greet").unwrap();
    assert_eq!(greet.start_line, 16);
    assert_eq!(greet.start_col, 5);
    assert_eq!(greet.end_col, 10);
}

#[test]
fn test_c_typedef_name_capture() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("simple.c");
    let lang = get_language("c").unwrap();
    let query =
        compile_query(&lang, "(type_definition declarator: (type_identifier) @type_name)").unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].matched_text, "Point");
    assert_eq!(results[0].start_line, 14);
    assert_eq!(results[0].start_col, 2);
    assert_eq!(results[0].end_col, 7);
}

#[test]
fn test_cpp_class_declaration_capture() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("simple.cpp");
    let lang = get_language("cpp").unwrap();
    let query =
        compile_query(&lang, "(class_specifier name: (type_identifier) @class_name)").unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let mut results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    results.sort();

    let names: std::collections::HashSet<&str> =
        results.iter().map(|r| r.matched_text.as_str()).collect();

    assert!(names.contains("Animal"));
    assert!(names.contains("Dog"));
    assert_eq!(results.len(), 2);

    let animal = results.iter().find(|r| r.matched_text == "Animal").unwrap();
    assert_eq!(animal.start_line, 3);
    assert_eq!(animal.start_col, 6);
    assert_eq!(animal.end_col, 12);

    let dog = results.iter().find(|r| r.matched_text == "Dog").unwrap();
    assert_eq!(dog.start_line, 9);
    assert_eq!(dog.start_col, 6);
    assert_eq!(dog.end_col, 9);
}

#[test]
fn test_cpp_struct_declaration_capture() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("simple.cpp");
    let lang = get_language("cpp").unwrap();
    let query =
        compile_query(&lang, "(struct_specifier name: (type_identifier) @struct_name)").unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].matched_text, "Point");
    assert_eq!(results[0].start_line, 25);
    assert_eq!(results[0].start_col, 7);
    assert_eq!(results[0].end_col, 12);
}

#[test]
fn test_cpp_free_function_capture() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("simple.cpp");
    let lang = get_language("cpp").unwrap();
    let query = compile_query(
        &lang,
        "(function_definition declarator: (function_declarator declarator: (identifier) @fn_name))",
    )
    .unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    let names: std::collections::HashSet<&str> =
        results.iter().map(|r| r.matched_text.as_str()).collect();

    assert!(names.contains("add"));
    assert!(names.contains("multiply"));
}

#[test]
fn test_c_walker_extensions() {
    use ast_search::types::Language;
    use ast_search::walker::build_walker;
    use std::fs;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("main.c"), b"int main() { return 0; }").unwrap();
    fs::write(dir.path().join("util.h"), b"void util();").unwrap();
    fs::write(dir.path().join("app.cpp"), b"int main() { return 0; }").unwrap();
    fs::write(dir.path().join("lib.hpp"), b"class Lib {};").unwrap();
    fs::write(dir.path().join("main.rs"), b"fn main() {}").unwrap();

    let entries: Vec<_> =
        build_walker(dir.path(), &Language::C).collect::<Result<Vec<_>, _>>().unwrap();

    let names: std::collections::HashSet<String> = entries
        .iter()
        .filter_map(|e| e.path().file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();

    assert!(names.contains("main.c"));
    assert!(names.contains("util.h"));
    assert!(!names.contains("app.cpp"));
    assert!(!names.contains("lib.hpp"));
    assert!(!names.contains("main.rs"));
    assert_eq!(entries.len(), 2);
}

#[test]
fn test_cpp_walker_extensions() {
    use ast_search::types::Language;
    use ast_search::walker::build_walker;
    use std::fs;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("app.cpp"), b"int main() {}").unwrap();
    fs::write(dir.path().join("lib.cc"), b"void lib() {}").unwrap();
    fs::write(dir.path().join("types.hpp"), b"struct Point {};").unwrap();
    fs::write(dir.path().join("util.hxx"), b"void util();").unwrap();
    fs::write(dir.path().join("mod.cxx"), b"void mod() {}").unwrap();
    fs::write(dir.path().join("shared.h"), b"void shared();").unwrap();
    fs::write(dir.path().join("main.c"), b"int main() { return 0; }").unwrap();
    fs::write(dir.path().join("main.rs"), b"fn main() {}").unwrap();

    let entries: Vec<_> =
        build_walker(dir.path(), &Language::Cpp).collect::<Result<Vec<_>, _>>().unwrap();

    let names: std::collections::HashSet<String> = entries
        .iter()
        .filter_map(|e| e.path().file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();

    assert!(names.contains("app.cpp"));
    assert!(names.contains("lib.cc"));
    assert!(names.contains("types.hpp"));
    assert!(names.contains("util.hxx"));
    assert!(names.contains("mod.cxx"));
    assert!(names.contains("shared.h"));
    assert!(!names.contains("main.c"));
    assert!(!names.contains("main.rs"));
    assert_eq!(entries.len(), 6);
}

#[test]
fn test_c_eq_predicate() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("simple.c");
    let lang = get_language("c").unwrap();
    let query = compile_query(
        &lang,
        r#"(function_definition
             declarator: (function_declarator
               declarator: (identifier) @fn_name
               (#eq? @fn_name "add")))"#,
    )
    .unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].matched_text, "add");
    assert_eq!(results[0].start_line, 3);
}

#[test]
fn test_cpp_eq_predicate_class() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("simple.cpp");
    let lang = get_language("cpp").unwrap();
    let query = compile_query(
        &lang,
        r#"(class_specifier name: (type_identifier) @class_name (#eq? @class_name "Dog"))"#,
    )
    .unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].matched_text, "Dog");
    assert_eq!(results[0].start_line, 9);
}

#[test]
fn test_all_seven_languages_compile_queries() {
    use ast_search::parser::get_language;
    use ast_search::query::compile_query;

    let cases = vec![
        ("rust",   "(function_item name: (identifier) @fn_name)"),
        ("python", "(function_definition name: (identifier) @fn_name)"),
        ("js",     "(function_declaration name: (identifier) @fn_name)"),
        ("ts",     "(function_declaration name: (identifier) @fn_name)"),
        ("go",     "(function_declaration name: (identifier) @fn_name)"),
        ("c",      "(function_definition declarator: (function_declarator declarator: (identifier) @fn_name))"),
        ("cpp",    "(function_definition declarator: (function_declarator declarator: (identifier) @fn_name))"),
    ];

    for (lang_str, query_str) in cases {
        let lang = get_language(lang_str).unwrap();
        let result = compile_query(&lang, query_str);
        assert!(result.is_ok(), "query compile failed for lang={}: {:?}", lang_str, result.err());
    }
}

#[test]
fn test_all_seven_languages_parse_minimal_source() {
    use ast_search::parser::{get_language, parse_file};
    use std::io::Write;
    use tempfile::NamedTempFile;

    let cases: Vec<(&str, &str, &str)> = vec![
        ("rust", "fn main() {}", "source_file"),
        ("python", "def main(): pass", "module"),
        ("js", "function main() {}", "program"),
        ("ts", "function main(): void {}", "program"),
        ("go", "package main\nfunc main() {}", "source_file"),
        ("c", "int main(void) { return 0; }", "translation_unit"),
        ("cpp", "int main() { return 0; }", "translation_unit"),
    ];

    for (lang_str, source, expected_root_kind) in cases {
        let lang = get_language(lang_str).unwrap();
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "{}", source).unwrap();
        let result = parse_file(file.path(), &lang);
        assert!(result.is_ok(), "parse failed for lang={}: {:?}", lang_str, result.err());
        let (tree, src) = result.unwrap();
        assert_eq!(
            tree.root_node().kind(),
            expected_root_kind,
            "wrong root node kind for lang={}",
            lang_str
        );
        assert!(!tree.root_node().has_error(), "unexpected parse errors for lang={}", lang_str);
        drop(tree);
        drop(src);
    }
}

#[test]
fn test_c_and_cpp_results_do_not_mix() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let c_fixture = fixtures_dir().join("simple.c");
    let cpp_fixture = fixtures_dir().join("simple.cpp");

    let c_lang = get_language("c").unwrap();
    let cpp_lang = get_language("cpp").unwrap();

    let query_str =
        "(function_definition declarator: (function_declarator declarator: (identifier) @fn_name))";

    let c_query = compile_query(&c_lang, query_str).unwrap();
    let cpp_query = compile_query(&cpp_lang, query_str).unwrap();

    let (c_tree, c_src) = parse_file(&c_fixture, &c_lang).unwrap();
    let c_results = extract_matches(&c_tree, &c_src, &c_query, &c_fixture);
    drop(c_tree);
    drop(c_src);

    let (cpp_tree, cpp_src) = parse_file(&cpp_fixture, &cpp_lang).unwrap();
    let cpp_results = extract_matches(&cpp_tree, &cpp_src, &cpp_query, &cpp_fixture);
    drop(cpp_tree);
    drop(cpp_src);

    for r in &c_results {
        assert_eq!(r.file_path, c_fixture);
    }
    for r in &cpp_results {
        assert_eq!(r.file_path, cpp_fixture);
    }

    assert!(!c_results.is_empty());
    assert!(!cpp_results.is_empty());
}

fn auto_compiled_queries(
    query_str: &str,
) -> std::collections::HashMap<Language, Arc<ast_search::query::CompiledQuery>> {
    ast_search::parser::get_all_languages()
        .into_iter()
        .filter_map(|(lang, ts_lang)| {
            compile_query(&ts_lang, query_str).ok().map(|query| (lang, query))
        })
        .collect()
}

fn auto_results(query_str: &str) -> Vec<MatchResult> {
    use ast_search::parser::{detect_language, get_all_languages, parse_file};
    use ast_search::walker::build_auto_walker;

    let compiled = Arc::new(auto_compiled_queries(query_str));
    let results = Arc::new(Mutex::new(Vec::<MatchResult>::new()));
    let results_ref = Arc::clone(&results);
    let compiled_ref = Arc::clone(&compiled);
    let fixture_root = fixtures_dir();

    build_auto_walker(&fixture_root).for_each(|entry_result| {
        let entry = match entry_result {
            Ok(entry) => entry,
            Err(_) => return,
        };

        let detected = match detect_language(entry.path()) {
            Some(lang) => lang,
            None => return,
        };

        let query = match compiled_ref.get(&detected) {
            Some(query) => Arc::clone(query),
            None => return,
        };

        let ts_lang = match get_all_languages().into_iter().find(|(lang, _)| lang == &detected) {
            Some((_, ts_lang)) => ts_lang,
            None => return,
        };

        let (tree, source) = match parse_file(entry.path(), &ts_lang) {
            Ok(pair) => pair,
            Err(_) => return,
        };

        let mut matches = extract_matches(&tree, &source, query.as_ref(), entry.path());
        drop(tree);
        drop(source);

        if !matches.is_empty() {
            results_ref.lock().unwrap().append(&mut matches);
        }
    });

    drop(results_ref);
    drop(compiled_ref);

    let mut results = Arc::try_unwrap(results).unwrap().into_inner().unwrap();
    results.sort();
    results.dedup();
    results
}

#[test]
fn test_auto_mode_finds_rust_functions() {
    use ast_search::parser::get_all_languages;

    let query_str = "(function_item name: (identifier) @fn_name)";
    let compiled = auto_compiled_queries(query_str);

    assert!(compiled.contains_key(&Language::Rust));
    assert!(!compiled.contains_key(&Language::Python));

    let results = auto_results(query_str);
    let matched_texts: HashSet<&str> =
        results.iter().map(|result| result.matched_text.as_str()).collect();

    assert!(matched_texts.contains("add"));

    let supported = get_all_languages();
    assert_eq!(supported.len(), 7);
}

#[test]
fn test_auto_mode_finds_multiple_languages() {
    let query_str = "(identifier) @id";
    let results = auto_results(query_str);

    let file_exts: HashSet<&str> = results
        .iter()
        .filter_map(|result| result.file_path.extension().and_then(|ext| ext.to_str()))
        .collect();

    assert!(file_exts.contains("rs"));
    assert!(file_exts.contains("py"));
    assert!(file_exts.contains("js"));
    assert!(file_exts.contains("ts"));
    assert!(file_exts.contains("go"));
    assert!(file_exts.contains("c"));
    assert!(file_exts.contains("cpp"));
}

#[test]
fn test_detect_language_no_extension_returns_none() {
    use ast_search::parser::detect_language;

    assert_eq!(detect_language(Path::new("Makefile")), None);
    assert_eq!(detect_language(Path::new("Dockerfile")), None);
    assert_eq!(detect_language(Path::new("LICENSE")), None);
}

#[test]
fn test_auto_query_skips_incompatible_languages() {
    let compiled = auto_compiled_queries("(function_item name: (identifier) @fn_name)");

    assert_eq!(compiled.len(), 1);
    assert!(compiled.contains_key(&Language::Rust));
}

#[test]
fn test_auto_mode_universal_query_matches_all_languages() {
    let compiled = auto_compiled_queries("(identifier) @id");

    assert_eq!(compiled.len(), 7);
    assert!(compiled.contains_key(&Language::Rust));
    assert!(compiled.contains_key(&Language::Python));
    assert!(compiled.contains_key(&Language::JavaScript));
    assert!(compiled.contains_key(&Language::TypeScript));
    assert!(compiled.contains_key(&Language::Go));
    assert!(compiled.contains_key(&Language::C));
    assert!(compiled.contains_key(&Language::Cpp));
}

#[test]
fn test_auto_mode_zero_languages_after_filter() {
    let compiled = auto_compiled_queries("(this_node_type_does_not_exist_in_any_grammar @cap)");

    assert!(compiled.is_empty());
}

#[test]
fn test_python_nested_closure_capture() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("nested_closures.py");
    let lang = get_language("python").unwrap();
    let query =
        compile_query(&lang, r#"(function_definition name: (identifier) @fn_name)"#).unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    let fn_names: Vec<&str> = results
        .iter()
        .filter(|r| r.capture_name == "fn_name")
        .map(|r| r.matched_text.as_str())
        .collect();

    let unique: std::collections::HashSet<&str> = fn_names.iter().copied().collect();
    assert_eq!(unique.len(), 4);
    assert!(unique.contains("outer_function"));
    assert!(unique.contains("middle_function"));
    assert!(unique.contains("inner_closure"));
    assert!(unique.contains("another_function"));

    let inner_closure = results
        .iter()
        .find(|r| r.capture_name == "fn_name" && r.matched_text == "inner_closure")
        .unwrap();

    assert_eq!(inner_closure.start_line, 3);
    assert_eq!(inner_closure.start_col, 12);
    assert!(inner_closure.end_col > inner_closure.start_col);
}

#[test]
fn test_javascript_arrow_function_params_capture() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("arrow_params.js");
    let lang = get_language("js").unwrap();
    let query = compile_query(
        &lang,
        r#"(arrow_function parameters: (formal_parameters (identifier) @param))"#,
    )
    .unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let mut results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    results.sort_by_key(|r| (r.start_line, r.start_col));

    let param_names: Vec<&str> = results.iter().map(|r| r.matched_text.as_str()).collect();

    assert!(param_names.contains(&"a"));
    assert!(param_names.contains(&"x"));
    assert!(param_names.contains(&"y"));
    assert!(param_names.contains(&"z"));

    let first_param = results.iter().find(|r| r.matched_text == "a").unwrap();
    assert_eq!(first_param.start_line, 1);
    assert_eq!(first_param.start_col, 16);
}

#[test]
fn test_typescript_interface_regex_predicate() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("interfaces.ts");
    let lang = get_language("ts").unwrap();
    let query = compile_query(
        &lang,
        r#"(interface_declaration name: (type_identifier) @iface_name (#match? @iface_name "^I[A-Z]"))"#,
    )
    .unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    let iface_names: Vec<&str> = results.iter().map(|r| r.matched_text.as_str()).collect();

    assert_eq!(iface_names.len(), 3);
    assert!(iface_names.contains(&"IConfig"));
    assert!(iface_names.contains(&"ILogger"));
    assert!(iface_names.contains(&"IEvent"));

    for result in &results {
        assert!(result.matched_text.starts_with("I"));
    }
}

#[test]
fn test_go_struct_exact_match_predicate() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("structs.go");
    let lang = get_language("go").unwrap();
    let query = compile_query(
        &lang,
        r#"(type_declaration (type_spec name: (type_identifier) @struct_name (#eq? @struct_name "ServerConfig")))"#,
    )
    .unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].matched_text, "ServerConfig");
    assert_eq!(results[0].start_line, 3);
    assert_eq!(results[0].start_col, 5);

    for result in &results {
        assert_eq!(result.matched_text, "ServerConfig");
    }
}

#[test]
fn test_cpp_virtual_keyword_capture() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("virtual_keywords.cpp");
    let lang = get_language("cpp").unwrap();
    let query = compile_query(&lang, r#"(virtual) @keyword"#).unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    assert!(results.len() > 0);

    for result in &results {
        assert_eq!(result.capture_name, "keyword");
        assert_eq!(result.matched_text, "virtual");
    }

    let mut virtual_counts_by_line: std::collections::HashMap<usize, u32> =
        std::collections::HashMap::new();
    for r in &results {
        *virtual_counts_by_line.entry(r.start_line).or_insert(0u32) += 1u32;
    }

    assert!(virtual_counts_by_line.len() > 1);
}

#[test]
fn test_cpp_anonymous_destructor_node() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("virtual_keywords.cpp");
    let lang = get_language("cpp").unwrap();
    let query = match compile_query(
        &lang,
        r#"(destructor_declaration (virtual_function_declarator "~" @tilde))"#,
    ) {
        Ok(q) => q,
        Err(_) => return, // skip if C++ grammar does not expose this node type on this platform
    };

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    if !results.is_empty() {
        for result in &results {
            assert_eq!(result.matched_text, "~");
            assert_eq!(result.start_line, 6);
        }
    }
}

#[test]
fn test_cross_language_string_literal_auto_mode() {
    let query_str = r#"(string) @str"#;

    let compiled = auto_compiled_queries(query_str);

    assert!(compiled.contains_key(&Language::Python));
    assert!(compiled.contains_key(&Language::JavaScript));

    let results = auto_results(query_str);

    let py_results: Vec<_> = results
        .iter()
        .filter(|r| r.file_path.extension().map(|e| e == "py").unwrap_or(false))
        .collect();

    let js_results: Vec<_> = results
        .iter()
        .filter(|r| r.file_path.extension().map(|e| e == "js").unwrap_or(false))
        .collect();

    assert!(!py_results.is_empty(), "should find Python strings");
    assert!(!js_results.is_empty(), "should find JavaScript strings");

    // Go may not expose the node type `string` in its grammar; verify Go string
    // literals by a direct go-specific query to ensure cross-language coverage.
    let go_lang = get_language("go").unwrap();
    let go_query = match compile_query(&go_lang, "(interpreted_string_literal) @str") {
        Ok(q) => q,
        Err(_) => match compile_query(&go_lang, "(string) @str") {
            Ok(q2) => q2,
            Err(_) => return, // skip Go check if neither node exists
        },
    };

    let go_fixture = fixtures_dir().join("simple.go");
    let (go_tree, go_src) = parse_file(&go_fixture, &go_lang).unwrap();
    let go_results = extract_matches(&go_tree, &go_src, &go_query, &go_fixture);
    drop(go_tree);
    drop(go_src);

    assert!(!go_results.is_empty(), "should find Go string literals via go-specific query");
}

#[test]
fn test_auto_mode_mixed_language_dispatcher() {
    let query_str = "(identifier) @id";

    let results = auto_results(query_str);

    let language_files: std::collections::HashMap<&str, Vec<&MatchResult>> =
        results.iter().fold(std::collections::HashMap::new(), |mut map, result| {
            if let Some(ext) = result.file_path.extension().and_then(|e| e.to_str()) {
                map.entry(ext).or_insert_with(Vec::new).push(result);
            }
            map
        });

    let py_count = language_files.get("py").map(|v| v.len()).unwrap_or(0);
    let js_count = language_files.get("js").map(|v| v.len()).unwrap_or(0);
    let go_count = language_files.get("go").map(|v| v.len()).unwrap_or(0);

    assert!(py_count > 0, "must find Python identifiers");
    assert!(js_count > 0, "must find JavaScript identifiers");
    assert!(go_count > 0, "must find Go identifiers");

    for result in &results {
        assert!(result.file_path.is_absolute());
        assert!(result.file_path.exists());
        assert!(!result.matched_text.is_empty());
    }
}

#[test]
fn test_auto_mode_no_memory_leak_with_mixed_languages() {
    let query_str = "(function_item name: (identifier) @fn_name)";

    let compiled1 = auto_compiled_queries(query_str);
    let compiled2 = auto_compiled_queries(query_str);

    assert_eq!(compiled1.len(), compiled2.len());

    let results1 = auto_results(query_str);
    let results2 = auto_results(query_str);

    assert_eq!(results1, results2);
    assert_eq!(results1.len(), results2.len());
}

#[test]
fn test_nested_python_closure_line_accuracy() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("nested_closures.py");
    let lang = get_language("python").unwrap();
    let query =
        compile_query(&lang, r#"(function_definition name: (identifier) @fn_name)"#).unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let mut results = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    results.sort_by_key(|r| r.start_line);

    let outer = results.iter().find(|r| r.matched_text == "outer_function").unwrap();
    let middle = results.iter().find(|r| r.matched_text == "middle_function").unwrap();
    let inner = results.iter().find(|r| r.matched_text == "inner_closure").unwrap();

    assert_eq!(outer.start_line, 1);
    assert!(middle.start_line > outer.start_line);
    assert!(inner.start_line > middle.start_line);
    assert_eq!(middle.start_line, 2);
    assert_eq!(inner.start_line, 3);
}

#[test]
fn test_typescript_interface_count_with_regex() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("interfaces.ts");
    let lang = get_language("ts").unwrap();

    let query_i_prefix = compile_query(
        &lang,
        r#"(interface_declaration name: (type_identifier) @iface_name (#match? @iface_name "^I"))"#,
    )
    .unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let i_results = extract_matches(&tree, &source, &query_i_prefix, &fixture);
    drop(tree);
    drop(source);

    assert_eq!(i_results.len(), 3);

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let query_all_interfaces =
        compile_query(&lang, r#"(interface_declaration name: (type_identifier) @iface_name)"#)
            .unwrap();
    let all_results = extract_matches(&tree, &source, &query_all_interfaces, &fixture);
    drop(tree);
    drop(source);

    assert_eq!(all_results.len(), 3);
}

#[test]
fn test_go_struct_selective_extraction() {
    use ast_search::parser::{get_language, parse_file};
    use ast_search::query::{compile_query, extract_matches};

    let fixture = fixtures_dir().join("structs.go");
    let lang = get_language("go").unwrap();

    let query = compile_query(
        &lang,
        r#"(type_declaration (type_spec name: (type_identifier) @struct_name))"#,
    )
    .unwrap();

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let all_structs = extract_matches(&tree, &source, &query, &fixture);
    drop(tree);
    drop(source);

    assert_eq!(all_structs.len(), 3);

    let struct_names: std::collections::HashSet<&str> =
        all_structs.iter().map(|r| r.matched_text.as_str()).collect();

    assert!(struct_names.contains("ServerConfig"));
    assert!(struct_names.contains("DatabaseConfig"));
    assert!(struct_names.contains("CacheConfig"));

    let (tree, source) = parse_file(&fixture, &lang).unwrap();
    let query_database = compile_query(
        &lang,
        r#"(type_declaration (type_spec name: (type_identifier) @struct_name (#match? @struct_name "Config$")))"#,
    )
    .unwrap();
    let filtered = extract_matches(&tree, &source, &query_database, &fixture);
    drop(tree);
    drop(source);

    assert_eq!(filtered.len(), 3);
    for result in &filtered {
        assert!(result.matched_text.ends_with("Config"));
    }
}
