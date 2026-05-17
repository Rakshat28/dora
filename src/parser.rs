#![allow(clippy::module_name_repetitions)]

use crate::types::{AppError, Result};
use std::cell::RefCell;
use std::path::Path;
use tree_sitter::{Language, Parser, Tree};

fn create_parser() -> Parser {
    Parser::new()
}

thread_local! {
    static PARSER: RefCell<Parser> = RefCell::new(create_parser());
}

#[allow(clippy::missing_errors_doc)]
pub fn get_language(lang: &str) -> Result<Language> {
    match lang {
        "rust" => Ok(tree_sitter_rust::language()),
        "python" => Ok(tree_sitter_python::language()),
        "js" => Ok(tree_sitter_javascript::language()),
        "ts" => Ok(tree_sitter_typescript::language_tsx()),
        "go" => Ok(tree_sitter_go::language()),
        "c" => Ok(tree_sitter_c::language()),
        "cpp" => Ok(tree_sitter_cpp::language()),
        other => Err(AppError::LanguageNotSupported(format!(
            "Language '{other}' is not supported. Supported: rust, python, js, ts, go, c, cpp"
        ))),
    }
}

#[must_use = "The returned Tree and String must be dropped immediately after query execution. Holding them accumulates unbounded RAM."]
#[allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]
pub fn parse_file(path: &Path, language: &tree_sitter::Language) -> Result<(Tree, String)> {
    let source = std::fs::read_to_string(path).map_err(AppError::IoError)?;

    if source.is_empty() {
        return Err(AppError::ParseError(format!(
            "File is empty and contains no parseable content: {}",
            path.display()
        )));
    }

    let tree = PARSER.with(|cell| {
        let mut parser = cell.borrow_mut();
        parser
            .set_language(language)
            .expect("failed to set language on parser: grammar/library version mismatch");
        parser.parse(source.as_bytes(), None)
    });

    let tree = tree.ok_or_else(|| {
        AppError::ParseError(format!(
            "Tree-sitter returned no parse tree for: {}. This may indicate a grammar/library version mismatch.",
            path.display()
        ))
    })?;

    Ok((tree, source))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn parse_str(source: &str) -> Tree {
        PARSER.with(|cell| {
            let mut parser = cell.borrow_mut();
            parser
                .set_language(&tree_sitter_rust::language())
                .expect("failed to set rust language in parse_str");
            parser.parse(source.as_bytes(), None).expect("Test source failed to parse")
        })
    }

    #[test]
    fn test_parse_valid_rust() {
        let tree = parse_str("fn hello(x: i32) -> i32 { x + 1 }");
        assert_eq!(tree.root_node().kind(), "source_file");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn test_parse_returns_tree_on_syntax_error() {
        let tree = parse_str("fn broken( {");
        assert_eq!(tree.root_node().kind(), "source_file");
        assert!(tree.root_node().has_error());
    }

    #[test]
    fn test_thread_local_parser_is_reused() {
        let first = parse_str("fn first() {}");
        let second = parse_str("fn second() {}");

        assert_eq!(first.root_node().kind(), "source_file");
        assert_eq!(second.root_node().kind(), "source_file");
        assert!(!first.root_node().has_error());
        assert!(!second.root_node().has_error());
    }

    #[test]
    fn test_get_language_rust() {
        assert!(get_language("rust").is_ok());
    }

    #[test]
    fn test_get_language_unsupported() {
        assert!(matches!(get_language("cobol"), Err(AppError::LanguageNotSupported(_))));
    }

    #[test]
    fn test_parse_file_empty_returns_error() {
        let file = NamedTempFile::new().unwrap();

        let result = parse_file(file.path(), &get_language("rust").unwrap());

        assert!(
            matches!(result, Err(AppError::ParseError(_))),
            "Expected ParseError for empty file, got: {:?}",
            result
        );

        if let Err(AppError::ParseError(msg)) = result {
            assert!(msg.contains("empty"), "Error message should mention 'empty', got: {msg}");
        }
    }

    #[test]
    fn test_parse_file_valid_rust() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "fn greet(name: &str) -> String {{").unwrap();
        writeln!(file, "    format!(\"Hello, {{}}!\", name)").unwrap();
        writeln!(file, "}}").unwrap();

        let result = parse_file(file.path(), &get_language("rust").unwrap());

        assert!(
            result.is_ok(),
            "Expected Ok from parse_file on valid Rust source, got: {:?}",
            result.err()
        );

        let (tree, source) = result.unwrap();

        assert_eq!(tree.root_node().kind(), "source_file", "Root node kind should be source_file");

        assert!(
            !tree.root_node().has_error(),
            "Valid Rust source should produce a tree with no errors"
        );

        assert!(source.contains("fn greet"), "Returned source should contain the written function");

        drop(tree);
        drop(source);
    }

    #[test]
    fn test_parse_file_invalid_utf8_returns_error() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&[0xFF, 0xFE, 0x00, 0x80, 0xBF]).unwrap();

        let result = parse_file(file.path(), &get_language("rust").unwrap());

        assert!(
            matches!(result, Err(AppError::IoError(_))),
            "Expected IoError for invalid UTF-8 file, got: {:?}",
            result
        );
    }

    #[test]
    fn test_parse_file_broken_syntax_yields_partial_tree() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "fn broken_function( {{").unwrap();
        writeln!(file, "    let x = ;;; ???").unwrap();
        writeln!(file, "}}}}}}}}").unwrap();

        let result = parse_file(file.path(), &get_language("rust").unwrap());

        assert!(
            result.is_ok(),
            "Tree-sitter should return a partial tree for broken syntax, got: {:?}",
            result.err()
        );

        let (tree, _source) = result.unwrap();

        assert_eq!(
            tree.root_node().kind(),
            "source_file",
            "Root node must always be source_file even for broken input"
        );

        assert!(
            tree.root_node().has_error(),
            "Broken syntax should set has_error() = true on the root node"
        );

        assert!(
            tree.root_node().child_count() > 0,
            "Partial tree should have at least one child node"
        );
    }

    #[test]
    fn test_parse_file_nonexistent_path_returns_io_error() {
        let result = parse_file(
            Path::new("/tmp/ast_search_this_file_does_not_exist_xyz.rs"),
            &get_language("rust").unwrap(),
        );

        assert!(
            matches!(result, Err(AppError::IoError(_))),
            "Expected IoError for nonexistent path, got: {:?}",
            result
        );
    }

    #[test]
    fn test_parse_file_return_values_are_owned() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "struct Config {{ timeout: u64 }}").unwrap();

        let (tree, source) = parse_file(file.path(), &get_language("rust").unwrap()).unwrap();

        let owned: (Tree, String) = (tree, source);

        assert_eq!(owned.0.root_node().kind(), "source_file");
        assert!(owned.1.contains("Config"));

        drop(owned.0);
        drop(owned.1);
    }

    #[test]
    fn test_get_language_js() {
        assert!(get_language("js").is_ok());
    }

    #[test]
    fn test_get_language_c() {
        assert!(get_language("c").is_ok());
    }

    #[test]
    fn test_get_language_cpp() {
        assert!(get_language("cpp").is_ok());
    }

    #[test]
    fn test_get_language_ts() {
        assert!(get_language("ts").is_ok());
    }

    #[test]
    fn test_get_language_error_lists_js_and_ts() {
        let err = get_language("ruby").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("js"));
        assert!(msg.contains("ts"));
        assert!(msg.contains("ruby"));
    }

    #[test]
    fn test_parse_file_javascript_valid() {
        let lang = get_language("js").unwrap();
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "function greet(name) {{").unwrap();
        writeln!(file, "    return name;").unwrap();
        writeln!(file, "}}").unwrap();
        let result = parse_file(file.path(), &lang);
        assert!(result.is_ok());
        let (tree, source) = result.unwrap();
        assert_eq!(tree.root_node().kind(), "program");
        assert!(!tree.root_node().has_error());
        assert!(source.contains("function greet"));
        drop(tree);
        drop(source);
    }

    #[test]
    fn test_parse_file_typescript_valid() {
        let lang = get_language("ts").unwrap();
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "interface Shape {{").unwrap();
        writeln!(file, "    area(): number;").unwrap();
        writeln!(file, "}}").unwrap();
        let result = parse_file(file.path(), &lang);
        assert!(result.is_ok());
        let (tree, source) = result.unwrap();
        assert_eq!(tree.root_node().kind(), "program");
        assert!(!tree.root_node().has_error());
        assert!(source.contains("interface Shape"));
        drop(tree);
        drop(source);
    }

    #[test]
    fn test_parse_file_tsx_handles_jsx() {
        let lang = get_language("ts").unwrap();
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "function App() {{").unwrap();
        writeln!(file, "    return <div>Hello</div>;").unwrap();
        writeln!(file, "}}").unwrap();
        let result = parse_file(file.path(), &lang);
        assert!(result.is_ok());
        let (tree, _source) = result.unwrap();
        assert_eq!(tree.root_node().kind(), "program");
        drop(tree);
    }

    #[test]
    fn test_js_grammar_on_typescript_syntax_has_errors() {
        let js_lang = get_language("js").unwrap();
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "interface Shape {{").unwrap();
        writeln!(file, "    area(): number;").unwrap();
        writeln!(file, "}}").unwrap();
        let result = parse_file(file.path(), &js_lang);
        assert!(result.is_ok());
        let (tree, _source) = result.unwrap();
        assert!(
            tree.root_node().has_error(),
            "TypeScript interface syntax parsed with JS grammar must produce errors"
        );
        drop(tree);
    }

    #[test]
    fn test_sequential_parse_js_ts_rust_same_thread() {
        let js_lang = get_language("js").unwrap();
        let ts_lang = get_language("ts").unwrap();
        let rust_lang = get_language("rust").unwrap();

        let mut js_file = NamedTempFile::new().unwrap();
        writeln!(js_file, "function foo() {{}}").unwrap();

        let mut ts_file = NamedTempFile::new().unwrap();
        writeln!(ts_file, "function bar(): void {{}}").unwrap();

        let mut rs_file = NamedTempFile::new().unwrap();
        writeln!(rs_file, "fn baz() {{}}").unwrap();

        let (js_tree, js_src) = parse_file(js_file.path(), &js_lang).unwrap();
        assert_eq!(js_tree.root_node().kind(), "program");
        assert!(!js_tree.root_node().has_error());
        drop(js_tree);
        drop(js_src);

        let (ts_tree, ts_src) = parse_file(ts_file.path(), &ts_lang).unwrap();
        assert_eq!(ts_tree.root_node().kind(), "program");
        assert!(!ts_tree.root_node().has_error());
        drop(ts_tree);
        drop(ts_src);

        let (rs_tree, rs_src) = parse_file(rs_file.path(), &rust_lang).unwrap();
        assert_eq!(rs_tree.root_node().kind(), "source_file");
        assert!(!rs_tree.root_node().has_error());
        drop(rs_tree);
        drop(rs_src);
    }

    #[test]
    fn test_get_language_go() {
        assert!(get_language("go").is_ok());
    }

    #[test]
    fn test_get_language_error_lists_all_supported() {
        let err = get_language("java").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("rust"));
        assert!(msg.contains("python"));
        assert!(msg.contains("js"));
        assert!(msg.contains("ts"));
        assert!(msg.contains("go"));
        assert!(msg.contains("c"));
        assert!(msg.contains("cpp"));
        assert!(msg.contains("java"));
    }

    #[test]
    fn test_parse_file_c_valid() {
        let lang = get_language("c").unwrap();
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "int add(int a, int b) {{").unwrap();
        writeln!(file, "    return a + b;").unwrap();
        writeln!(file, "}}").unwrap();
        let result = parse_file(file.path(), &lang);
        assert!(result.is_ok());
        let (tree, source) = result.unwrap();
        assert_eq!(tree.root_node().kind(), "translation_unit");
        assert!(!tree.root_node().has_error());
        assert!(source.contains("int add"));
        drop(tree);
        drop(source);
    }

    #[test]
    fn test_parse_file_cpp_valid() {
        let lang = get_language("cpp").unwrap();
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "class Calculator {{").unwrap();
        writeln!(file, "public:").unwrap();
        writeln!(file, "    int add(int a, int b) {{ return a + b; }}").unwrap();
        writeln!(file, "}};").unwrap();
        let result = parse_file(file.path(), &lang);
        assert!(result.is_ok());
        let (tree, source) = result.unwrap();
        assert_eq!(tree.root_node().kind(), "translation_unit");
        assert!(!tree.root_node().has_error());
        assert!(source.contains("class Calculator"));
        drop(tree);
        drop(source);
    }

    #[test]
    fn test_c_grammar_on_cpp_class_has_errors() {
        let c_lang = get_language("c").unwrap();
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "class Calculator {{").unwrap();
        writeln!(file, "public:").unwrap();
        writeln!(file, "    int add(int a, int b) {{ return a + b; }}").unwrap();
        writeln!(file, "}};").unwrap();
        let result = parse_file(file.path(), &c_lang);
        assert!(result.is_ok());
        let (tree, _source) = result.unwrap();
        assert!(tree.root_node().has_error());
        drop(tree);
    }

    #[test]
    fn test_sequential_parse_c_then_cpp_same_thread() {
        let c_lang = get_language("c").unwrap();
        let cpp_lang = get_language("cpp").unwrap();

        let mut c_file = NamedTempFile::new().unwrap();
        writeln!(c_file, "int foo(void) {{ return 0; }}").unwrap();

        let mut cpp_file = NamedTempFile::new().unwrap();
        writeln!(cpp_file, "class Foo {{ public: int bar() {{ return 0; }} }};").unwrap();

        let (c_tree, c_src) = parse_file(c_file.path(), &c_lang).unwrap();
        assert_eq!(c_tree.root_node().kind(), "translation_unit");
        assert!(!c_tree.root_node().has_error());
        drop(c_tree);
        drop(c_src);

        let (cpp_tree, cpp_src) = parse_file(cpp_file.path(), &cpp_lang).unwrap();
        assert_eq!(cpp_tree.root_node().kind(), "translation_unit");
        assert!(!cpp_tree.root_node().has_error());
        drop(cpp_tree);
        drop(cpp_src);
    }

    #[test]
    fn test_parse_file_go_valid() {
        let lang = get_language("go").unwrap();
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "package main").unwrap();
        writeln!(file, "").unwrap();
        writeln!(file, "func greet(name string) string {{").unwrap();
        writeln!(file, "    return \"Hello, \" + name").unwrap();
        writeln!(file, "}}").unwrap();
        let result = parse_file(file.path(), &lang);
        assert!(result.is_ok());
        let (tree, source) = result.unwrap();
        assert_eq!(tree.root_node().kind(), "source_file");
        assert!(!tree.root_node().has_error());
        assert!(source.contains("func greet"));
        drop(tree);
        drop(source);
    }

    #[test]
    fn test_parse_file_go_broken_syntax_partial_tree() {
        let lang = get_language("go").unwrap();
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "func broken(").unwrap();
        writeln!(file, "    ???").unwrap();
        let result = parse_file(file.path(), &lang);
        assert!(result.is_ok());
        let (tree, _source) = result.unwrap();
        assert_eq!(tree.root_node().kind(), "source_file");
        assert!(tree.root_node().has_error());
        drop(tree);
    }
}
