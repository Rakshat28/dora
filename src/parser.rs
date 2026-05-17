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
        other => Err(AppError::LanguageNotSupported(format!(
            "Language '{other}' is not supported. Supported: rust, python"
        ))),
    }
}

#[must_use = "The returned Tree and String must be dropped immediately after query execution. Holding them accumulates unbounded RAM."]
#[allow(clippy::missing_errors_doc)]
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
        let result = parse_file(Path::new("/tmp/ast_search_this_file_does_not_exist_xyz.rs"), &get_language("rust").unwrap());

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
}
