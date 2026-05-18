#![allow(clippy::module_name_repetitions, dead_code)]

use crate::types::{AppError, Language, Result};
use std::cell::RefCell;
use std::path::Path;
use tree_sitter::{Language as TsLanguage, Parser, Tree};

fn create_parser() -> Parser {
    Parser::new()
}

thread_local! {
    static PARSER: RefCell<Parser> = RefCell::new(create_parser());
}

#[derive(Debug)]
pub enum FileSource {
    Heap(String),
    Mapped(memmap2::Mmap),
}

impl FileSource {
    #[allow(dead_code)]
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            FileSource::Heap(s) => s.as_bytes(),
            FileSource::Mapped(m) => m.as_ref(),
        }
    }

    #[allow(dead_code)]
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            FileSource::Heap(s) => Some(s.as_str()),
            FileSource::Mapped(m) => std::str::from_utf8(m.as_ref()).ok(),
        }
    }
}

impl AsRef<[u8]> for FileSource {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

#[allow(clippy::missing_errors_doc, dead_code)]
pub fn get_language(lang: &str) -> Result<TsLanguage> {
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

pub const MMAP_THRESHOLD_BYTES: u64 = 1_024 * 1_024;

#[allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]
pub fn parse_file_with_threshold(
    path: &Path,
    language: &tree_sitter::Language,
    threshold: u64,
) -> Result<(Tree, FileSource)> {
    let metadata = std::fs::metadata(path).map_err(AppError::IoError)?;
    parse_file_with_metadata_and_threshold(path, language, &metadata, threshold)
}

#[allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]
pub fn parse_file_with_metadata(
    path: &Path,
    language: &tree_sitter::Language,
    metadata: &std::fs::Metadata,
) -> Result<(Tree, FileSource)> {
    parse_file_with_metadata_and_threshold(path, language, metadata, MMAP_THRESHOLD_BYTES)
}

#[allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]
fn parse_file_with_metadata_and_threshold(
    path: &Path,
    language: &tree_sitter::Language,
    metadata: &std::fs::Metadata,
    threshold: u64,
) -> Result<(Tree, FileSource)> {
    let file_size = metadata.len();

    let source = if file_size >= threshold {
        let file = std::fs::File::open(path).map_err(AppError::IoError)?;
        let mmap = unsafe { memmap2::Mmap::map(&file).map_err(AppError::IoError)? };
        if std::str::from_utf8(mmap.as_ref()).is_err() {
            return Err(AppError::ParseError(format!(
                "file contains invalid UTF-8: {}",
                path.display()
            )));
        }
        FileSource::Mapped(mmap)
    } else {
        let s = std::fs::read_to_string(path).map_err(AppError::IoError)?;
        FileSource::Heap(s)
    };

    if source.as_bytes().is_empty() {
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

#[must_use = "The returned Tree and FileSource must be dropped immediately after query execution. Holding them accumulates unbounded RAM."]
#[allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]
pub fn parse_file(path: &Path, language: &tree_sitter::Language) -> Result<(Tree, FileSource)> {
    parse_file_with_threshold(path, language, MMAP_THRESHOLD_BYTES)
}

#[must_use]
pub fn detect_language(path: &Path) -> Option<Language> {
    let ext = path.extension().and_then(|e| e.to_str())?.to_lowercase();

    match ext.as_str() {
        "rs" => Some(Language::Rust),
        "py" | "pyi" => Some(Language::Python),
        "js" | "mjs" | "cjs" => Some(Language::JavaScript),
        "ts" | "mts" | "cts" | "tsx" => Some(Language::TypeScript),
        "go" => Some(Language::Go),
        "c" | "h" => Some(Language::C),
        "cpp" | "cc" | "hpp" | "hxx" | "cxx" => Some(Language::Cpp),
        _ => None,
    }
}

#[must_use]
pub fn get_all_languages() -> Vec<(Language, TsLanguage)> {
    vec![
        (Language::Rust, tree_sitter_rust::language()),
        (Language::Python, tree_sitter_python::language()),
        (Language::JavaScript, tree_sitter_javascript::language()),
        (Language::TypeScript, tree_sitter_typescript::language_tsx()),
        (Language::Go, tree_sitter_go::language()),
        (Language::C, tree_sitter_c::language()),
        (Language::Cpp, tree_sitter_cpp::language()),
    ]
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

        assert!(
            source.as_str().unwrap().contains("fn greet"),
            "Returned source should contain the written function"
        );

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
            Path::new("/tmp/dora_this_file_does_not_exist_xyz.rs"),
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

        let owned: (Tree, FileSource) = (tree, source);

        assert_eq!(owned.0.root_node().kind(), "source_file");
        assert!(owned.1.as_str().unwrap().contains("Config"));

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
        assert!(source.as_str().unwrap().contains("function greet"));
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
        assert!(source.as_str().unwrap().contains("interface Shape"));
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
        assert!(source.as_str().unwrap().contains("int add"));
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
        assert!(source.as_str().unwrap().contains("class Calculator"));
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
        assert!(source.as_str().unwrap().contains("func greet"));
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

    #[test]
    fn test_detect_language_rust_extensions() {
        assert_eq!(detect_language(Path::new("main.rs")), Some(Language::Rust));
        assert_eq!(detect_language(Path::new("src/lib.rs")), Some(Language::Rust));
    }

    #[test]
    fn test_detect_language_python_extensions() {
        assert_eq!(detect_language(Path::new("script.py")), Some(Language::Python));
        assert_eq!(detect_language(Path::new("stub.pyi")), Some(Language::Python));
    }

    #[test]
    fn test_detect_language_javascript_extensions() {
        assert_eq!(detect_language(Path::new("app.js")), Some(Language::JavaScript));
        assert_eq!(detect_language(Path::new("mod.mjs")), Some(Language::JavaScript));
        assert_eq!(detect_language(Path::new("cjs.cjs")), Some(Language::JavaScript));
    }

    #[test]
    fn test_detect_language_typescript_extensions() {
        assert_eq!(detect_language(Path::new("index.ts")), Some(Language::TypeScript));
        assert_eq!(detect_language(Path::new("app.tsx")), Some(Language::TypeScript));
        assert_eq!(detect_language(Path::new("mod.mts")), Some(Language::TypeScript));
        assert_eq!(detect_language(Path::new("cts.cts")), Some(Language::TypeScript));
    }

    #[test]
    fn test_detect_language_go_extensions() {
        assert_eq!(detect_language(Path::new("main.go")), Some(Language::Go));
    }

    #[test]
    fn test_detect_language_c_extensions() {
        assert_eq!(detect_language(Path::new("main.c")), Some(Language::C));
        assert_eq!(detect_language(Path::new("util.h")), Some(Language::C));
    }

    #[test]
    fn test_detect_language_h_defaults_to_c() {
        let result = detect_language(Path::new("types.h"));
        assert_eq!(result, Some(Language::C));
        assert_ne!(result, Some(Language::Cpp));
    }

    #[test]
    fn test_detect_language_cpp_extensions() {
        assert_eq!(detect_language(Path::new("app.cpp")), Some(Language::Cpp));
        assert_eq!(detect_language(Path::new("lib.cc")), Some(Language::Cpp));
        assert_eq!(detect_language(Path::new("types.hpp")), Some(Language::Cpp));
        assert_eq!(detect_language(Path::new("util.hxx")), Some(Language::Cpp));
        assert_eq!(detect_language(Path::new("mod.cxx")), Some(Language::Cpp));
    }

    #[test]
    fn test_detect_language_unknown_extension() {
        assert_eq!(detect_language(Path::new("README.md")), None);
        assert_eq!(detect_language(Path::new("config.toml")), None);
        assert_eq!(detect_language(Path::new("Makefile")), None);
        assert_eq!(detect_language(Path::new("no_extension")), None);
    }

    #[test]
    fn test_detect_language_case_insensitive() {
        assert_eq!(detect_language(Path::new("Main.RS")), Some(Language::Rust));
        assert_eq!(detect_language(Path::new("App.JS")), Some(Language::JavaScript));
        assert_eq!(detect_language(Path::new("Lib.CPP")), Some(Language::Cpp));
    }

    #[test]
    fn test_get_all_languages_returns_all_seven() {
        let all = get_all_languages();
        assert_eq!(all.len(), 7);

        let lang_variants: Vec<&Language> = all.iter().map(|(lang, _)| lang).collect();
        assert!(lang_variants.contains(&&Language::Rust));
        assert!(lang_variants.contains(&&Language::Python));
        assert!(lang_variants.contains(&&Language::JavaScript));
        assert!(lang_variants.contains(&&Language::TypeScript));
        assert!(lang_variants.contains(&&Language::Go));
        assert!(lang_variants.contains(&&Language::C));
        assert!(lang_variants.contains(&&Language::Cpp));
    }

    #[test]
    fn test_get_all_languages_all_valid() {
        for (_, ts_lang) in get_all_languages() {
            let mut parser = tree_sitter::Parser::new();
            assert!(parser.set_language(&ts_lang).is_ok());
        }
    }

    #[test]
    fn test_small_file_uses_heap_source() {
        let lang = get_language("rust").unwrap();
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "fn main() {{}}").unwrap();
        let (_tree, source) = parse_file_with_threshold(file.path(), &lang, 1_024 * 1_024).unwrap();
        assert!(matches!(source, FileSource::Heap(_)));
    }

    #[test]
    fn test_large_file_uses_mapped_source() {
        let lang = get_language("rust").unwrap();
        let mut file = NamedTempFile::new().unwrap();
        let content = "fn foo() {}\n".repeat(100);
        write!(file, "{}", content).unwrap();
        let threshold = content.len() as u64;
        let (_tree, source) = parse_file_with_threshold(file.path(), &lang, threshold).unwrap();
        assert!(matches!(source, FileSource::Mapped(_)));
    }

    #[test]
    fn test_file_source_as_bytes_consistent() {
        let lang = get_language("rust").unwrap();
        let content = "fn consistent() {}";
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "{}", content).unwrap();

        let (_, heap_src) = parse_file_with_threshold(file.path(), &lang, u64::MAX).unwrap();
        let (_, mmap_src) = parse_file_with_threshold(file.path(), &lang, 0).unwrap();

        assert_eq!(heap_src.as_bytes(), mmap_src.as_bytes());
        assert_eq!(heap_src.as_bytes(), content.as_bytes());
    }

    #[test]
    fn test_mmap_file_parses_correctly() {
        let lang = get_language("rust").unwrap();
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "fn mmapped() {{ let x = 42; }}").unwrap();
        let (tree, source) = parse_file_with_threshold(file.path(), &lang, 0).unwrap();
        assert!(matches!(source, FileSource::Mapped(_)));
        assert_eq!(tree.root_node().kind(), "source_file");
        assert!(!tree.root_node().has_error());
        drop(tree);
        drop(source);
    }
}
