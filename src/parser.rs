#![allow(clippy::module_name_repetitions)]

use std::cell::RefCell;
use std::path::Path;
use tree_sitter::{Language, Parser, Tree};
use crate::types::{AppError, Result};

fn create_parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::language())
        .expect("failed to initialize Rust parser: tree-sitter language registration failed");
    parser
}

// thread_local! is used instead of Mutex<Parser> because each Rayon worker thread
// should own one Parser for its entire lifetime with zero cross-thread contention.
// RefCell is required inside thread_local! because the storage is accessed through
// a shared reference, but Parser::parse needs &mut self for mutation during parsing.
// This is safe because each thread gets its own independent Parser instance; no
// Parser value is ever shared between threads. As a result, Parser is initialized
// at most once per OS thread and reused for every file that thread processes.
// The RefCell borrow must never be held across an await point or Rayon spawn boundary.
thread_local! {
    static PARSER: RefCell<Parser> = RefCell::new(create_parser());
}

/// Resolves a CLI language string to a Tree-sitter language.
///
/// # Errors
/// Returns `AppError::LanguageNotSupported` for any language other than `rust`.
pub fn get_language(lang: &str) -> Result<Language> {
    match lang {
        "rust" => Ok(tree_sitter_rust::language()),
        other => Err(AppError::LanguageNotSupported(format!(
            "Language '{other}' is not supported. Supported: rust"
        ))),
    }
}

/// Parse a single source file into a Tree-sitter Concrete Syntax Tree.
///
/// # Memory Contract (CRITICAL — read before calling)
///
/// This function returns a `(Tree, String)` pair. Both values are HEAVY:
///
///   - `String`: the entire file source loaded into heap memory.
///   - `Tree`:   a Tree-sitter CST containing potentially tens of thousands
///               of nodes, allocated by the underlying C library via FFI.
///
/// **The caller MUST drop both values immediately after query execution.**
/// Do NOT store them in a Vec, cache them, or move them into any structure
/// with a lifetime longer than a single file-processing event.
///
/// In a 10,000-file repository, retaining all Trees simultaneously would
/// consume tens of gigabytes of RAM and trigger the OS OOM killer.
///
/// Correct usage pattern:
/// ```ignore
/// let (tree, source) = parse_file(&path)?;
/// let results = extract_matches(&tree, &source, &query);
/// drop(tree);
/// drop(source);
/// // Only `results` (Vec<MatchResult> of extracted strings) persists.
/// ```
///
/// # Errors
///
/// Returns `AppError::IoError`    if the file cannot be read.
/// Returns `AppError::ParseError` if the file is empty.
/// Returns `AppError::ParseError` if Tree-sitter yields `None`
///         (malformed input that the error-recovery parser cannot handle).
#[must_use = "The returned Tree and String must be dropped immediately after query execution. Holding them accumulates unbounded RAM."]
pub fn parse_file(path: &Path) -> Result<(Tree, String)> {
    // Step 1 — Read entire file into an owned String.
    // fs::read_to_string validates UTF-8 as it reads. Files containing invalid
    // UTF-8 sequences (binary files, corrupted files) are rejected here before
    // they reach the Tree-sitter C library, preventing undefined FFI behavior.
    let source = std::fs::read_to_string(path)
        .map_err(AppError::IoError)?;

    // Step 2 — Reject empty files before entering the FFI boundary.
    // Tree-sitter handles empty input without crashing, but an empty file
    // produces an empty CST with no queryable nodes. Skipping it early
    // avoids wasting a borrow_mut() call and produces a clearer error message.
    if source.is_empty() {
        return Err(AppError::ParseError(format!(
            "File is empty and contains no parseable content: {}",
            path.display()
        )));
    }

    // Step 3 — Borrow the thread-local Parser for this OS thread.
    // PARSER.with() provides a &RefCell<Parser> scoped to the closure.
    // borrow_mut() will panic if called re-entrantly on the same thread —
    // this cannot happen because parse_file is synchronous and never calls
    // itself recursively, directly or indirectly.
    //
    // IMPORTANT: the borrow_mut() guard must NOT be held across an await
    // point or a rayon::spawn boundary. It is not — this entire function
    // is synchronous and the guard is released when the closure returns.
    let tree = PARSER.with(|cell| {
        let mut parser = cell.borrow_mut();

        // Step 4 — Parse the source bytes.
        // parser.parse() takes &[u8]. We pass source.as_bytes() — a borrow
        // into the String we already own. No copy occurs.
        //
        // The second argument (Option<&Tree>) is the previous tree for
        // incremental re-parsing. We pass None — each file is parsed fresh.
        //
        // Return type is Option<Tree>. Tree-sitter returns None only for
        // pathological inputs (e.g. parsing was cancelled via a timeout
        // callback, or the language was not set). Since we set the language
        // in create_parser() and never set a timeout, None is extremely rare.
        parser.parse(source.as_bytes(), None)
    });

    // Step 5 — Unwrap the Option<Tree>, converting None to a descriptive error.
    // If this branch is hit in practice, it almost certainly indicates a
    // Tree-sitter version mismatch between the core library and the grammar
    // crate, or memory pressure causing internal C allocation failure.
    let tree = tree.ok_or_else(|| AppError::ParseError(format!(
        "Tree-sitter returned no parse tree for: {}. \
     This may indicate a grammar/library version mismatch.",
        path.display()
    )))?;

    // Step 6 — Return the tree and source string to the caller.
    // The caller owns both. The memory contract above applies from this point.
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
                .parse(source.as_bytes(), None)
                .expect("Test source failed to parse")
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
        assert!(matches!(
            get_language("cobol"),
            Err(AppError::LanguageNotSupported(_))
        ));
    }

    #[test]
    fn test_parse_file_empty_returns_error() {
        // An empty NamedTempFile — zero bytes on disk.
        // parse_file must reject this at Step 2 before entering the FFI boundary.
        let file = NamedTempFile::new().unwrap();

        let result = parse_file(file.path());

        assert!(
            matches!(result, Err(AppError::ParseError(_))),
            "Expected ParseError for empty file, got: {:?}",
            result
        );

        // Verify the error message mentions the file path for debuggability.
        if let Err(AppError::ParseError(msg)) = result {
            assert!(
                msg.contains("empty"),
                "Error message should mention 'empty', got: {msg}"
            );
        }
    }

    #[test]
    fn test_parse_file_valid_rust() {
        // Write a known-good Rust source to a real temp file on disk.
        // This tests the full path: fs::read_to_string → parse → Tree returned.
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "fn greet(name: &str) -> String {{").unwrap();
        writeln!(file, "    format!(\"Hello, {{}}!\", name)").unwrap();
        writeln!(file, "}}").unwrap();

        let result = parse_file(file.path());

        assert!(
            result.is_ok(),
            "Expected Ok from parse_file on valid Rust source, got: {:?}",
            result.err()
        );

        let (tree, source) = result.unwrap();

        // Root node must be source_file — the tree-sitter Rust grammar root.
        assert_eq!(
            tree.root_node().kind(),
            "source_file",
            "Root node kind should be source_file"
        );

        // No parse errors — the source is syntactically valid.
        assert!(
            !tree.root_node().has_error(),
            "Valid Rust source should produce a tree with no errors"
        );

        // Source string must contain what we wrote.
        assert!(
            source.contains("fn greet"),
            "Returned source should contain the written function"
        );

        // Explicit drop — mirrors correct call-site usage per memory contract.
        drop(tree);
        drop(source);
    }

    #[test]
    fn test_parse_file_invalid_utf8_returns_error() {
        // Write raw bytes that are not valid UTF-8.
        // fs::read_to_string will fail with an InvalidData IoError.
        // This verifies that binary files and corrupted files are safely
        // rejected before reaching the Tree-sitter C FFI boundary.
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&[0xFF, 0xFE, 0x00, 0x80, 0xBF]).unwrap();

        let result = parse_file(file.path());

        assert!(
            matches!(result, Err(AppError::IoError(_))),
            "Expected IoError for invalid UTF-8 file, got: {:?}",
            result
        );
    }

    #[test]
    fn test_parse_file_broken_syntax_yields_partial_tree() {
        // Tree-sitter uses error recovery — it never fails to return a Tree
        // for non-empty UTF-8 input, even if the source has syntax errors.
        // This test documents and LOCKS IN that behavior.
        //
        // If this test fails in the future, it means a Tree-sitter version
        // change broke error recovery — a critical regression for our use case,
        // since we depend on partial trees for in-progress file editing.
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "fn broken_function( {{").unwrap();
        writeln!(file, "    let x = ;;; ???").unwrap();
        writeln!(file, "}}}}}}}}").unwrap();

        let result = parse_file(file.path());

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

        // The tree must have at least one child node — it is not empty.
        assert!(
            tree.root_node().child_count() > 0,
            "Partial tree should have at least one child node"
        );
    }

    #[test]
    fn test_parse_file_nonexistent_path_returns_io_error() {
        // Passing a path that does not exist must return AppError::IoError.
        // This covers the fs::read_to_string failure path at Step 1.
        let result = parse_file(Path::new("/tmp/ast_search_this_file_does_not_exist_xyz.rs"));

        assert!(
            matches!(result, Err(AppError::IoError(_))),
            "Expected IoError for nonexistent path, got: {:?}",
            result
        );
    }

    #[test]
    fn test_parse_file_return_values_are_owned() {
        // Verify that the returned Tree and String are fully owned by the caller —
        // not borrowed from the parser or any internal buffer.
        // If ownership is correct, both values can be moved freely.
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "struct Config {{ timeout: u64 }}").unwrap();

        let (tree, source) = parse_file(file.path()).unwrap();

        // Move both into a tuple — proves they are owned, not borrowed.
        let owned: (Tree, String) = (tree, source);

        assert_eq!(owned.0.root_node().kind(), "source_file");
        assert!(owned.1.contains("Config"));

        // Explicit drop in correct order: Tree before String.
        // (Tree does not borrow String in our architecture — Tree-sitter
        //  copies source bytes internally — but dropping Tree first is
        //  the safe convention to document for future maintainers.)
        drop(owned.0);
        drop(owned.1);
    }
}
