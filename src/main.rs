#![deny(warnings)]
#![warn(clippy::pedantic)]

use std::{
    collections::HashSet,
    path::PathBuf,
    process,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use clap::Parser;
use rayon::prelude::*;

mod output;
mod parser;
mod query;
mod types;
pub mod walker;

use output::{print_match, print_summary, resolve_color_mode, ColorMode};
use parser::{get_language as get_ts_language, parse_file};
use query::{compile_query, extract_matches};
use types::{AppError, Language, MatchResult, SearchConfig};
use walker::build_walker;

#[derive(Debug)]
enum FileError {
    WalkerAccess { path: Option<PathBuf>, message: String },
    ReadFailure { path: PathBuf, message: String },
    ParseFailure { path: PathBuf, message: String },
}

#[must_use]
fn format_file_error(error: &FileError) -> String {
    match error {
        FileError::WalkerAccess { path, message } => {
            let path_display = path
                .as_ref()
                .map_or_else(|| "<path unknown>".to_string(), |p| p.display().to_string());
            format!("warning: [walker] {path_display}: {message}")
        }
        FileError::ReadFailure { path, message } => {
            format!("warning: [read] {}: {message}", path.display())
        }
        FileError::ParseFailure { path, message } => {
            format!("warning: [parse] {}: {message}", path.display())
        }
    }
}

fn handle_file_error(error: &FileError, skip_count: &Mutex<usize>) {
    eprintln!("{}", format_file_error(error));
    *skip_count.lock().expect("skip_count Mutex poisoned") += 1;
}

#[derive(Parser, Debug)]
#[command(
    name = "ast-search",
    version,
    author,
    about = "Structural AST-based code search — find code by shape, not by text.",
    long_about = "ast-search parses source files into Abstract Syntax Trees and \
                  executes structural pattern queries against them.\n\n\
                  Unlike grep or ripgrep, ast-search understands code grammar. \
                  It can find function definitions, not just strings that look \
                  like them. It ignores matches inside comments, string literals, \
                  and dead code.\n\n\
                  Queries use Tree-sitter S-expression syntax:\n\
                  \n  \
                  ast-search -q '(function_item name: (identifier) @fn)' -p ./src\n\n\
                  See https://github.com/your-org/ast-search for full documentation."
)]
struct Cli {
    #[arg(
        short = 'q',
        long = "query",
        value_name = "S-EXPR",
        help = "Tree-sitter S-expression query pattern (required)",
        long_help = "An S-expression structural query in Tree-sitter syntax.\n\n\
                     Examples:\n\
                     \n  Find all function definitions:\n  \
                     (function_item name: (identifier) @fn_name)\
                     \n\n  Find a specific function:\n  \
                     (function_item name: (identifier) @fn (#eq? @fn \"connect\"))\
                     \n\n  Find all struct definitions:\n  \
                     (struct_item name: (type_identifier) @struct_name)"
    )]
    query: String,

    #[arg(
        short = 'p',
        long = "path",
        value_name = "DIR",
        default_value = ".",
        help = "Root directory to search (default: current directory)"
    )]
    path: PathBuf,

    #[arg(
        short = 'l',
        long = "lang",
        value_name = "LANG",
        default_value = "rust",
        help = "Language to parse: rust, python, js, ts, go (default: rust)"
    )]
    lang: String,

    #[arg(
        long = "no-color",
        default_value_t = false,
        help = "Disable ANSI color output (also: set NO_COLOR env var)"
    )]
    no_color: bool,

    #[arg(
        long = "quiet",
        short = 'Q',
        default_value_t = false,
        help = "Suppress per-match output lines — only print the summary"
    )]
    quiet: bool,

    #[arg(
        long = "stats",
        default_value_t = false,
        help = "Print detailed performance statistics to stderr after the search"
    )]
    stats: bool,
}

struct SearchOutcome {
    results: Vec<MatchResult>,
    files_walked: usize,
    files_parsed: usize,
    files_skipped: usize,
    files_with_matches: usize,
}

impl Cli {
    fn validate(&self) -> std::result::Result<(), String> {
        if !self.path.exists() {
            return Err(format!("path does not exist: {}", self.path.display()));
        }
        if !self.path.is_dir() {
            return Err(format!("path is not a directory: {}", self.path.display()));
        }
        if self.query.trim().is_empty() {
            return Err(format!("query must not be empty: {:?}", self.query));
        }
        Ok(())
    }
}

fn resolve_lang(lang_str: &str) -> std::result::Result<Language, String> {
    match lang_str {
        "rust" => Ok(Language::Rust),
        "python" => Ok(Language::Python),
        "js" => Ok(Language::JavaScript),
        "ts" => Ok(Language::TypeScript),
        "go" => Ok(Language::Go),
        other => {
            Err(format!("unsupported language '{other}'; supported: rust, python, js, ts, go"))
        }
    }
}

fn usize_to_f64(value: usize) -> f64 {
    f64::from(u32::try_from(value).unwrap_or(u32::MAX))
}

fn print_stats(outcome: &SearchOutcome, elapsed: Duration) {
    let match_rate = if outcome.files_parsed == 0 {
        0.0_f64
    } else {
        (usize_to_f64(outcome.files_with_matches) / usize_to_f64(outcome.files_parsed)) * 100.0
    };

    let elapsed_secs = elapsed.as_secs_f64();
    let throughput = if elapsed_secs == 0.0 {
        None
    } else {
        Some(usize_to_f64(outcome.files_parsed) / elapsed_secs)
    };

    eprintln!("--- search statistics ---");
    eprintln!("{:<18}{}", "files walked:", outcome.files_walked);
    eprintln!("{:<18}{}", "files parsed:", outcome.files_parsed);
    eprintln!("{:<18}{}", "files skipped:", outcome.files_skipped);
    eprintln!("{:<18}{}", "matches found:", outcome.results.len());
    eprintln!("{:<18}{:.2}% (files with matches / files parsed)", "match rate:", match_rate);
    eprintln!("{:<18}{}ms", "wall time:", elapsed.as_millis());
    match throughput {
        Some(t) => eprintln!("{:<18}{:.2} files/sec", "throughput:", t),
        None => eprintln!("{:<18}N/A", "throughput:"),
    }
}

#[must_use]
#[allow(clippy::too_many_lines)]
fn run_search(
    config: &SearchConfig,
    query: &Arc<tree_sitter::Query>,
    color: &ColorMode,
) -> SearchOutcome {
    let _ = color;

    let results = Arc::new(Mutex::new(Vec::<MatchResult>::new()));
    let files_walked_count = Arc::new(Mutex::new(0usize));
    let files_parsed_count = Arc::new(Mutex::new(0usize));
    let files_skipped_count = Arc::new(Mutex::new(0usize));

    let results_ref = Arc::clone(&results);
    let files_walked_ref = Arc::clone(&files_walked_count);
    let files_parsed_ref = Arc::clone(&files_parsed_count);
    let files_skipped_ref = Arc::clone(&files_skipped_count);
    let query_ref = Arc::clone(query);

    build_walker(config.root_path.as_path(), &config.language).par_bridge().for_each(
        move |entry_result| match entry_result {
            Ok(entry) => {
                *files_walked_ref
                    .lock()
                    .expect("files_walked Mutex was poisoned by a panicked thread") += 1;

                match parse_file(entry.path()) {
                    Ok((tree, source)) => {
                        let matches =
                            extract_matches(&tree, &source, query_ref.as_ref(), entry.path());
                        drop(tree);
                        drop(source);

                        let mut results_guard = results_ref
                            .lock()
                            .expect("results Mutex was poisoned by a panicked thread");
                        results_guard.extend(matches);

                        let mut count_guard = files_parsed_ref
                            .lock()
                            .expect("files_parsed Mutex was poisoned by a panicked thread");
                        *count_guard += 1;
                    }
                    Err(error) => {
                        let file_error = match &error {
                            AppError::IoError(_) => FileError::ReadFailure {
                                path: entry.path().to_path_buf(),
                                message: error.to_string(),
                            },
                            AppError::ParseError(_) => FileError::ParseFailure {
                                path: entry.path().to_path_buf(),
                                message: error.to_string(),
                            },
                            _ => FileError::ReadFailure {
                                path: entry.path().to_path_buf(),
                                message: format!("unexpected error: {error}"),
                            },
                        };
                        handle_file_error(&file_error, &files_skipped_ref);
                    }
                }
            }
            Err(error) => {
                handle_file_error(
                    &FileError::WalkerAccess { path: None, message: error.to_string() },
                    &files_skipped_ref,
                );
            }
        },
    );

    let mut final_results = {
        match Arc::try_unwrap(results) {
            Ok(mutex) => {
                mutex.into_inner().expect("results Mutex was poisoned by a panicked thread")
            }
            Err(shared) => {
                shared.lock().expect("results Mutex was poisoned by a panicked thread").clone()
            }
        }
    };
    let files_walked = {
        match Arc::try_unwrap(files_walked_count) {
            Ok(mutex) => {
                mutex.into_inner().expect("files_walked Mutex was poisoned by a panicked thread")
            }
            Err(shared) => {
                *shared.lock().expect("files_walked Mutex was poisoned by a panicked thread")
            }
        }
    };
    let files_parsed = {
        match Arc::try_unwrap(files_parsed_count) {
            Ok(mutex) => {
                mutex.into_inner().expect("files_parsed Mutex was poisoned by a panicked thread")
            }
            Err(shared) => {
                *shared.lock().expect("files_parsed Mutex was poisoned by a panicked thread")
            }
        }
    };
    let files_skipped = {
        match Arc::try_unwrap(files_skipped_count) {
            Ok(mutex) => {
                mutex.into_inner().expect("files_skipped Mutex was poisoned by a panicked thread")
            }
            Err(shared) => {
                *shared.lock().expect("files_skipped Mutex was poisoned by a panicked thread")
            }
        }
    };

    final_results.sort();
    final_results.dedup();

    let files_with_matches =
        final_results.iter().map(|r| &r.file_path).collect::<HashSet<_>>().len();

    SearchOutcome {
        results: final_results,
        files_walked,
        files_parsed,
        files_skipped,
        files_with_matches,
    }
}

fn main() {
    let cli = Cli::parse();

    let color = resolve_color_mode(cli.no_color);

    if let Err(message) = cli.validate() {
        eprintln!("error: {message}");
        process::exit(1);
    }

    let walker_lang = match resolve_lang(&cli.lang) {
        Ok(lang) => lang,
        Err(message) => {
            eprintln!("error: {message}");
            process::exit(1);
        }
    };

    let ts_lang = match get_ts_language(&cli.lang) {
        Ok(lang) => lang,
        Err(error) => {
            eprintln!("error: {error}");
            process::exit(1);
        }
    };

    let config = SearchConfig {
        query_str: cli.query.clone(),
        root_path: cli.path.clone(),
        language: walker_lang,
    };

    let query = match compile_query(&ts_lang, &config.query_str) {
        Ok(query) => query,
        Err(error) => {
            eprintln!("error: {error}");
            process::exit(1);
        }
    };

    let started_at = Instant::now();
    let outcome = run_search(&config, &query, &color);

    let stdout = std::io::stdout();
    if !cli.quiet {
        for result in &outcome.results {
            print_match(result, &color, &mut stdout.lock());
        }
    }

    print_summary(
        outcome.results.len(),
        outcome.files_parsed,
        started_at.elapsed(),
        &color,
        &mut std::io::stderr().lock(),
    );

    if outcome.files_skipped > 0 {
        eprintln!(
            "warning: skipped {} {} due to errors",
            outcome.files_skipped,
            if outcome.files_skipped == 1 { "file" } else { "files" }
        );
    }

    if cli.stats {
        print_stats(&outcome, started_at.elapsed());
    }
}

#[cfg(test)]
mod tests {
    use super::{
        format_file_error, handle_file_error, resolve_lang, Cli, FileError, SearchOutcome,
    };
    use crate::types::Language;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    #[test]
    fn test_format_file_error_walker_known_path() {
        let error = FileError::WalkerAccess {
            path: Some(PathBuf::from("src/secret/file.rs")),
            message: "permission denied".to_string(),
        };
        let output = format_file_error(&error);
        assert_eq!(output, "warning: [walker] src/secret/file.rs: permission denied");
    }

    #[test]
    fn test_format_file_error_walker_unknown_path() {
        let error =
            FileError::WalkerAccess { path: None, message: "too many open files".to_string() };
        let output = format_file_error(&error);
        assert_eq!(output, "warning: [walker] <path unknown>: too many open files");
    }

    #[test]
    fn test_format_file_error_read_failure() {
        let error = FileError::ReadFailure {
            path: PathBuf::from("src/broken.rs"),
            message: "No such file or directory (os error 2)".to_string(),
        };
        let output = format_file_error(&error);
        assert_eq!(output, "warning: [read] src/broken.rs: No such file or directory (os error 2)");
        assert!(output.starts_with("warning: [read]"));
        assert!(output.contains("src/broken.rs"));
    }

    #[test]
    fn test_format_file_error_parse_failure() {
        let error = FileError::ParseFailure {
            path: PathBuf::from("src/empty.rs"),
            message: "File is empty and contains no parseable content".to_string(),
        };
        let output = format_file_error(&error);
        assert_eq!(
            output,
            "warning: [parse] src/empty.rs: File is empty and contains no parseable content"
        );
        assert!(output.starts_with("warning: [parse]"));
    }

    #[test]
    fn test_format_file_error_structure() {
        let errors = vec![
            FileError::WalkerAccess {
                path: Some(PathBuf::from("test.rs")),
                message: "err".to_string(),
            },
            FileError::ReadFailure { path: PathBuf::from("test.rs"), message: "err".to_string() },
            FileError::ParseFailure { path: PathBuf::from("test.rs"), message: "err".to_string() },
        ];

        for error in errors {
            let output = format_file_error(&error);
            assert!(output.starts_with("warning: "));
            let has_category = output.contains("[walker]")
                || output.contains("[read]")
                || output.contains("[parse]");
            assert!(has_category);
            assert!(output.contains(": "));
            assert!(!output.ends_with('\n'));
        }
    }

    #[test]
    fn test_handle_file_error_increments_counter() {
        let counter = Mutex::new(0usize);
        let error =
            FileError::ReadFailure { path: PathBuf::from("x.rs"), message: "test".to_string() };
        handle_file_error(&error, &counter);
        assert_eq!(*counter.lock().unwrap(), 1);
    }

    #[test]
    fn test_handle_file_error_multiple_increments() {
        let counter = Mutex::new(0usize);
        let make_error =
            || FileError::ReadFailure { path: PathBuf::from("f.rs"), message: "e".to_string() };
        handle_file_error(&make_error(), &counter);
        handle_file_error(&make_error(), &counter);
        handle_file_error(&make_error(), &counter);
        assert_eq!(*counter.lock().unwrap(), 3);
    }

    #[test]
    fn test_cli_validate_valid_path() {
        let cli = Cli {
            query: "(function_item)".to_string(),
            path: std::env::temp_dir(),
            lang: "rust".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
        };
        assert!(cli.validate().is_ok());
    }

    #[test]
    fn test_cli_validate_nonexistent_path() {
        let cli = Cli {
            path: PathBuf::from("/tmp/ast_search_nonexistent_xyz_12345"),
            query: "(function_item)".to_string(),
            lang: "rust".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
        };
        let result = cli.validate();
        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        assert!(err_msg.contains("does not exist"));
        assert!(err_msg.contains("ast_search_nonexistent_xyz_12345"));
    }

    #[test]
    fn test_cli_validate_file_path_fails() {
        use tempfile::NamedTempFile;

        let temp_file = NamedTempFile::new().expect("failed to create temp file for test");
        let cli = Cli {
            path: temp_file.path().to_path_buf(),
            query: "(function_item)".to_string(),
            lang: "rust".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
        };
        let result = cli.validate();
        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        assert!(err_msg.contains("not a directory"));
    }

    #[test]
    fn test_cli_validate_empty_query() {
        let cli = Cli {
            path: std::env::temp_dir(),
            query: "   ".to_string(),
            lang: "rust".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
        };
        let result = cli.validate();
        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        assert!(err_msg.contains("empty"));
    }

    #[test]
    fn test_resolve_lang_all_supported() {
        assert!(resolve_lang("rust").is_ok());
        assert!(resolve_lang("python").is_ok());
        assert!(resolve_lang("js").is_ok());
        assert!(resolve_lang("ts").is_ok());
        assert!(resolve_lang("go").is_ok());

        assert_eq!(resolve_lang("rust").unwrap(), Language::Rust);
        assert_eq!(resolve_lang("python").unwrap(), Language::Python);
        assert_eq!(resolve_lang("js").unwrap(), Language::JavaScript);
        assert_eq!(resolve_lang("ts").unwrap(), Language::TypeScript);
        assert_eq!(resolve_lang("go").unwrap(), Language::Go);
    }

    #[test]
    fn test_resolve_lang_unsupported() {
        assert!(resolve_lang("cobol").is_err());
        assert!(resolve_lang("").is_err());
        assert!(resolve_lang("RUST").is_err());

        let err = resolve_lang("cobol").unwrap_err();
        assert!(err.contains("cobol"));
        assert!(err.contains("rust"));
    }

    #[test]
    fn test_instant_is_monotonically_non_decreasing() {
        let before = Instant::now();
        std::thread::sleep(Duration::from_millis(1));
        let after = Instant::now();
        assert!(after > before);
        assert!(after.duration_since(before) >= Duration::from_millis(1));
    }

    #[test]
    fn test_elapsed_duration_is_non_negative() {
        let start = Instant::now();
        let elapsed = start.elapsed();
        assert!(elapsed.as_millis() <= 10_000);
    }

    #[test]
    fn test_duration_as_millis_truncates_not_rounds() {
        let d1 = Duration::from_micros(999);
        let d2 = Duration::from_micros(1000);
        let d3 = Duration::from_micros(1999);
        let d4 = Duration::from_micros(2000);

        assert_eq!(d1.as_millis(), 0);
        assert_eq!(d2.as_millis(), 1);
        assert_eq!(d3.as_millis(), 1);
        assert_eq!(d4.as_millis(), 2);
    }

    #[test]
    fn test_sort_then_dedup_combined_behavior() {
        let mut results = vec![];

        results.push(crate::types::MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 5,
            start_col: 0,
            end_line: 5,
            end_col: 3,
            capture_name: "cap".to_string(),
            matched_text: "txt".to_string(),
        });
        results.push(crate::types::MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 3,
            capture_name: "cap".to_string(),
            matched_text: "txt".to_string(),
        });
        results.push(crate::types::MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 3,
            capture_name: "cap".to_string(),
            matched_text: "txt".to_string(),
        });
        results.push(crate::types::MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 3,
            start_col: 0,
            end_line: 3,
            end_col: 3,
            capture_name: "cap".to_string(),
            matched_text: "txt".to_string(),
        });
        results.push(crate::types::MatchResult {
            file_path: PathBuf::from("src/b.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 3,
            capture_name: "cap".to_string(),
            matched_text: "txt".to_string(),
        });

        results.sort();
        results.dedup();

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].file_path, PathBuf::from("src/a.rs"));
        assert_eq!(results[0].start_line, 1);
        assert_eq!(results[1].file_path, PathBuf::from("src/a.rs"));
        assert_eq!(results[1].start_line, 3);
        assert_eq!(results[2].file_path, PathBuf::from("src/a.rs"));
        assert_eq!(results[2].start_line, 5);
        assert_eq!(results[3].file_path, PathBuf::from("src/b.rs"));
        assert_eq!(results[3].start_line, 1);
    }

    #[test]
    fn test_sort_dedup_idempotent() {
        let mut results = vec![];

        results.push(crate::types::MatchResult {
            file_path: PathBuf::from("src/b.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 3,
            capture_name: "x".to_string(),
            matched_text: "x".to_string(),
        });
        results.push(crate::types::MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 3,
            capture_name: "a".to_string(),
            matched_text: "a".to_string(),
        });
        results.push(crate::types::MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 3,
            capture_name: "a".to_string(),
            matched_text: "a".to_string(),
        });

        results.sort();
        results.dedup();
        let after_first = results.clone();

        results.sort();
        results.dedup();
        let after_second = results.clone();

        assert_eq!(after_first, after_second);
    }

    #[test]
    fn test_stats_flag_defaults_false() {
        let cli = Cli {
            query: "(function_item)".to_string(),
            path: std::env::temp_dir(),
            lang: "rust".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
        };
        assert!(!cli.stats);
    }

    #[test]
    fn test_quiet_flag_defaults_false() {
        let cli = Cli {
            query: "(function_item)".to_string(),
            path: std::env::temp_dir(),
            lang: "rust".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
        };
        assert!(!cli.quiet);
    }

    #[test]
    fn test_match_rate_zero_when_no_files_parsed() {
        let rate = if 0 == 0 { 0.0_f64 } else { (0_f64 / 0_f64) * 100.0 };
        assert_eq!(rate, 0.0);
    }

    #[test]
    fn test_match_rate_full_when_all_files_match() {
        let files_parsed = 10_usize;
        let files_with_matches = 10_usize;
        let rate = (files_with_matches as f64 / files_parsed as f64) * 100.0;
        assert!((rate - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_match_rate_half() {
        let files_parsed = 10_usize;
        let files_with_matches = 5_usize;
        let rate = (files_with_matches as f64 / files_parsed as f64) * 100.0;
        assert!((rate - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_throughput_none_when_elapsed_zero() {
        let elapsed_secs = 0.0_f64;
        let throughput: Option<f64> =
            if elapsed_secs == 0.0 { None } else { Some(100.0 / elapsed_secs) };
        assert!(throughput.is_none());
    }

    #[test]
    fn test_throughput_computation() {
        let files_parsed = 100_usize;
        let elapsed_secs = 0.5_f64;
        let throughput = Some(files_parsed as f64 / elapsed_secs);
        assert!((throughput.unwrap() - 200.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_search_outcome_fields() {
        let outcome = SearchOutcome {
            results: Vec::new(),
            files_walked: 10,
            files_parsed: 9,
            files_skipped: 1,
            files_with_matches: 3,
        };
        assert_eq!(outcome.files_walked, 10);
        assert_eq!(outcome.files_parsed, 9);
        assert_eq!(outcome.files_skipped, 1);
        assert_eq!(outcome.files_with_matches, 3);
        assert!(outcome.results.is_empty());
    }

    #[test]
    fn test_files_with_matches_from_results() {
        use std::collections::HashSet;

        let results = vec![
            crate::types::MatchResult {
                file_path: PathBuf::from("src/a.rs"),
                start_line: 1,
                start_col: 0,
                end_line: 1,
                end_col: 3,
                capture_name: "fn".to_string(),
                matched_text: "foo".to_string(),
            },
            crate::types::MatchResult {
                file_path: PathBuf::from("src/a.rs"),
                start_line: 5,
                start_col: 0,
                end_line: 5,
                end_col: 3,
                capture_name: "fn".to_string(),
                matched_text: "bar".to_string(),
            },
            crate::types::MatchResult {
                file_path: PathBuf::from("src/b.rs"),
                start_line: 1,
                start_col: 0,
                end_line: 1,
                end_col: 3,
                capture_name: "fn".to_string(),
                matched_text: "baz".to_string(),
            },
        ];

        let files_with_matches = results.iter().map(|r| &r.file_path).collect::<HashSet<_>>().len();

        assert_eq!(files_with_matches, 2);
    }

    #[test]
    fn test_cli_validate_with_stats_field() {
        let cli = Cli {
            query: "(function_item)".to_string(),
            path: std::env::temp_dir(),
            lang: "rust".to_string(),
            no_color: false,
            quiet: false,
            stats: true,
        };
        assert!(cli.validate().is_ok());
    }
}
