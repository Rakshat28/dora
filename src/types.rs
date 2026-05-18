use std::path::PathBuf;

use thiserror::Error;

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Go,
    C,
    Cpp,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LangMode {
    Single(Language),
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MatchResult {
    pub file_path: PathBuf,
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub capture_name: String,
    pub matched_text: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchConfig {
    pub queries: Vec<String>,
    pub root_path: PathBuf,
    pub lang_mode: LangMode,
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Directory walk error: {0}")]
    WalkError(#[from] ignore::Error),

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Failed to compile query: {0}")]
    QueryCompileError(String),

    #[error("Language not supported: {0}")]
    LanguageNotSupported(String),
    #[error("index file is corrupt or unreadable: {0}")]
    IndexCorrupt(String),

    #[error("index version mismatch: found {found}, expected {expected}")]
    IndexVersionMismatch { found: u32, expected: u32 },

    #[error("index root mismatch: index was built for {index_root}, searching {search_root}")]
    IndexRootMismatch { index_root: PathBuf, search_root: PathBuf },
}

pub type Result<T> = std::result::Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_result_derives() {
        let a = MatchResult {
            file_path: PathBuf::from("src/main.rs"),
            capture_name: "fn.name".to_string(),
            matched_text: "main".to_string(),
            start_line: 0,
            start_col: 3,
            end_line: 0,
            end_col: 7,
        };
        let b = a.clone();
        assert_eq!(a, b);

        let mut results = vec![b.clone(), a.clone()];
        results.sort();
        assert_eq!(results[0], a);
    }

    #[test]
    fn search_config_derives() {
        let config = SearchConfig {
            queries: vec!["(function_item name: (identifier) @fn.name)".to_string()],
            root_path: PathBuf::from("/home/user/project"),
            lang_mode: LangMode::Single(Language::Rust),
        };

        let cloned = config.clone();
        assert_eq!(config, cloned);

        let _ = format!("{:?}", config);
    }

    #[test]
    fn search_config_inequality() {
        let base = SearchConfig {
            queries: vec!["fn main".to_string()],
            root_path: PathBuf::from("."),
            lang_mode: LangMode::Single(Language::Rust),
        };
        let different_lang =
            SearchConfig { lang_mode: LangMode::Single(Language::Python), ..base.clone() };

        assert_ne!(base, different_lang);
    }

    #[test]
    fn io_error_from_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let app_err: AppError = io_err.into();
        assert!(matches!(app_err, AppError::IoError(_)));
        assert!(app_err.to_string().starts_with("I/O error:"));
    }

    #[test]
    fn parse_error_display() {
        let err = AppError::ParseError("unexpected token '}'".to_string());
        assert_eq!(err.to_string(), "Parse error: unexpected token '}'");
    }

    #[test]
    fn query_compile_error_display() {
        let err = AppError::QueryCompileError("invalid node type 'foobar'".to_string());
        assert_eq!(err.to_string(), "Failed to compile query: invalid node type 'foobar'");
    }

    #[test]
    fn language_not_supported_display() {
        let err = AppError::LanguageNotSupported("COBOL".to_string());
        assert_eq!(err.to_string(), "Language not supported: COBOL");
    }

    #[test]
    fn question_mark_propagation() {
        fn try_open(path: &std::path::Path) -> Result<String> {
            let s = std::fs::read_to_string(path)?;
            Ok(s)
        }

        let result = try_open(std::path::Path::new("/nonexistent/path/xyz"));
        assert!(matches!(result, Err(AppError::IoError(_))));
    }

    #[test]
    fn test_match_result_sort_order_file_then_line_then_col() {
        let e = MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 3,
            capture_name: "e".to_string(),
            matched_text: "e".to_string(),
        };
        let b = MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 2,
            start_col: 0,
            end_line: 2,
            end_col: 3,
            capture_name: "b".to_string(),
            matched_text: "b".to_string(),
        };
        let d = MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 2,
            start_col: 3,
            end_line: 2,
            end_col: 5,
            capture_name: "d".to_string(),
            matched_text: "d".to_string(),
        };
        let a = MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 10,
            start_col: 5,
            end_line: 10,
            end_col: 8,
            capture_name: "a".to_string(),
            matched_text: "a".to_string(),
        };
        let c = MatchResult {
            file_path: PathBuf::from("src/z.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 3,
            capture_name: "c".to_string(),
            matched_text: "c".to_string(),
        };

        let mut results = vec![c.clone(), a.clone(), b.clone(), d.clone(), e.clone()];
        results.sort();

        assert_eq!(results[0].file_path, PathBuf::from("src/a.rs"));
        assert_eq!(results[0].start_line, 1);
        assert_eq!(results[0].start_col, 0);

        assert_eq!(results[1].file_path, PathBuf::from("src/a.rs"));
        assert_eq!(results[1].start_line, 2);
        assert_eq!(results[1].start_col, 0);

        assert_eq!(results[2].file_path, PathBuf::from("src/a.rs"));
        assert_eq!(results[2].start_line, 2);
        assert_eq!(results[2].start_col, 3);

        assert_eq!(results[3].file_path, PathBuf::from("src/a.rs"));
        assert_eq!(results[3].start_line, 10);
        assert_eq!(results[3].start_col, 5);

        assert_eq!(results[4].file_path, PathBuf::from("src/z.rs"));
        assert_eq!(results[4].start_line, 1);
        assert_eq!(results[4].start_col, 0);
    }

    #[test]
    fn test_match_result_sort_stable_on_position_tie() {
        let y = MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 5,
            start_col: 2,
            end_line: 5,
            end_col: 4,
            capture_name: "aaa".to_string(),
            matched_text: "bar".to_string(),
        };
        let x = MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 5,
            start_col: 2,
            end_line: 5,
            end_col: 8,
            capture_name: "zzz".to_string(),
            matched_text: "foo".to_string(),
        };

        let mut results = vec![x.clone(), y.clone()];
        results.sort();

        assert_eq!(results[0], y);
        assert_eq!(results[1], x);
    }

    #[test]
    fn test_match_result_dedup_removes_exact_duplicates() {
        let a = MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 3,
            capture_name: "fn_name".to_string(),
            matched_text: "foo".to_string(),
        };
        let b = a.clone();
        let c = MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 2,
            start_col: 0,
            end_line: 2,
            end_col: 3,
            capture_name: "fn_name".to_string(),
            matched_text: "bar".to_string(),
        };

        let mut results = vec![a.clone(), b, c.clone()];
        results.sort();
        results.dedup();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0], a);
        assert_eq!(results[1], c);
    }

    #[test]
    fn test_match_result_dedup_preserves_different_capture_names() {
        let a = MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 3,
            capture_name: "cap_one".to_string(),
            matched_text: "foo".to_string(),
        };
        let b = MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 3,
            capture_name: "cap_two".to_string(),
            matched_text: "foo".to_string(),
        };

        let mut results = vec![a, b];
        results.sort();
        results.dedup();

        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_match_result_dedup_preserves_different_matched_text() {
        let a = MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 3,
            capture_name: "cap".to_string(),
            matched_text: "foo".to_string(),
        };
        let b = MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 3,
            capture_name: "cap".to_string(),
            matched_text: "bar".to_string(),
        };

        let mut results = vec![a, b];
        results.sort();
        results.dedup();

        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_match_result_dedup_empty_vec() {
        let mut results: Vec<MatchResult> = vec![];
        results.sort();
        results.dedup();

        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_match_result_dedup_single_element() {
        let a = MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 3,
            capture_name: "cap".to_string(),
            matched_text: "foo".to_string(),
        };

        let mut results = vec![a.clone()];
        results.sort();
        results.dedup();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], a);
    }

    #[test]
    fn test_match_result_sort_idempotent() {
        let a = MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 3,
            capture_name: "a".to_string(),
            matched_text: "a".to_string(),
        };
        let b = MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 2,
            start_col: 0,
            end_line: 2,
            end_col: 3,
            capture_name: "b".to_string(),
            matched_text: "b".to_string(),
        };
        let c = MatchResult {
            file_path: PathBuf::from("src/b.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 3,
            capture_name: "c".to_string(),
            matched_text: "c".to_string(),
        };

        let mut results = vec![a.clone(), b.clone(), c.clone()];
        results.sort();
        let after_first_sort = results.clone();

        results.sort();
        let after_second_sort = results.clone();

        assert_eq!(after_first_sort, after_second_sort);
    }

    #[test]
    fn test_match_result_sort_by_line_ascending() {
        let mut results = vec![];
        for line in [99, 1, 50, 2, 100] {
            results.push(MatchResult {
                file_path: PathBuf::from("src/a.rs"),
                start_line: line,
                start_col: 0,
                end_line: line,
                end_col: 3,
                capture_name: "cap".to_string(),
                matched_text: "txt".to_string(),
            });
        }

        results.sort();

        assert_eq!(results[0].start_line, 1);
        assert_eq!(results[1].start_line, 2);
        assert_eq!(results[2].start_line, 50);
        assert_eq!(results[3].start_line, 99);
        assert_eq!(results[4].start_line, 100);
    }

    #[test]
    fn test_match_result_sort_by_col_ascending() {
        let mut results = vec![];
        for col in [10, 0, 7, 3] {
            results.push(MatchResult {
                file_path: PathBuf::from("src/a.rs"),
                start_line: 5,
                start_col: col,
                end_line: 5,
                end_col: col + 3,
                capture_name: "cap".to_string(),
                matched_text: "txt".to_string(),
            });
        }

        results.sort();

        assert_eq!(results[0].start_col, 0);
        assert_eq!(results[1].start_col, 3);
        assert_eq!(results[2].start_col, 7);
        assert_eq!(results[3].start_col, 10);
    }
}
