use std::path::PathBuf;

use thiserror::Error;

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Go,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MatchResult {
    pub file_path: PathBuf,
    pub capture_name: String,
    pub matched_text: String,
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchConfig {
    pub query_str: String,
    pub root_path: PathBuf,
    pub language: Language,
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Failed to compile query: {0}")]
    QueryCompileError(String),

    #[error("Language not supported: {0}")]
    LanguageNotSupported(String),
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
            query_str: "(function_item name: (identifier) @fn.name)".to_string(),
            root_path: PathBuf::from("/home/user/project"),
            language: Language::Rust,
        };

        let cloned = config.clone();
        assert_eq!(config, cloned);

        let _ = format!("{:?}", config);
    }

    #[test]
    fn search_config_inequality() {
        let base = SearchConfig {
            query_str: "fn main".to_string(),
            root_path: PathBuf::from("."),
            language: Language::Rust,
        };
        let different_lang = SearchConfig {
            language: Language::Python,
            ..base.clone()
        };

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
}
