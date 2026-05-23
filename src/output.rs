use crate::types::MatchResult;
use std::io::Write;
use std::time::Duration;

const RESET: &str = "\x1b[0m";
const CYAN: &str = "\x1b[36m";
const YELLOW: &str = "\x1b[33m";
const GREEN: &str = "\x1b[32m";

/// Controls whether ANSI color escape sequences are emitted in output.
///
/// Determined once at startup via [`resolve_color_mode`] and passed to
/// every print function. Never stored in a global — always passed explicitly
/// so tests can control it without environment variable side effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColorMode {
    /// Emit ANSI color escape sequences.
    On,
    /// Plain text output — no escape sequences. Safe for pipes and CI.
    Off,
}

/// Determines the correct [`ColorMode`] for this invocation.
///
/// Resolution order (first match wins):
///   1. If `no_color_flag` is true (--no-color CLI flag) → [`ColorMode::Off`]
///   2. If the `NO_COLOR` environment variable is set to any non-empty value → [`ColorMode::Off`]
///   3. Otherwise → [`ColorMode::On`]
///
/// The `NO_COLOR` convention is defined at <https://no-color.org/>:
/// any non-empty value of the env var disables color, regardless of its content.
///
/// # Arguments
/// * `no_color_flag` — true if the user passed `--no-color` on the CLI.
#[must_use]
pub fn resolve_color_mode(no_color_flag: bool) -> ColorMode {
    if no_color_flag {
        return ColorMode::Off;
    }

    // any non-empty value disables color — we do not inspect the value itself, only its presence
    if let Ok(v) = std::env::var("NO_COLOR") {
        if !v.is_empty() {
            return ColorMode::Off;
        }
    }

    ColorMode::On
}

/// Apply an ANSI color escape sequence to `text` if color mode is [`ColorMode::On`].
///
/// Returns the original text unchanged when color is [`ColorMode::Off`],
/// making it safe to call unconditionally in format strings.
///
/// # Arguments
/// * `text` — the string to colorize
/// * `code` — one of the module-level ANSI constants: [`CYAN`], [`YELLOW`], [`GREEN`]
/// * `color` — the active [`ColorMode`]
#[must_use]
fn colorize(text: &str, code: &str, color: &ColorMode) -> String {
    match color {
        ColorMode::On => format!("{code}{text}{RESET}"),
        ColorMode::Off => text.to_string(),
    }
}

/// Return "match" when `n == 1`, otherwise "matches".
#[must_use]
fn plural_match(n: usize) -> &'static str {
    if n == 1 {
        "match"
    } else {
        "matches"
    }
}

/// Return "file" when `n == 1`, otherwise "files".
#[must_use]
fn plural_file(n: usize) -> &'static str {
    if n == 1 {
        "file"
    } else {
        "files"
    }
}

/// Build the formatted single-match string without printing it.
///
/// The filepath, capture name, and matched text are colorized according
/// to `color`. Punctuation, numeric fields, and the surrounding quotes are
/// never colorized.
#[must_use]
fn format_match(result: &MatchResult, color: &ColorMode) -> String {
    let filepath = result.file_path.display().to_string();
    let colored_path = colorize(&filepath, CYAN, color);
    let colored_name = colorize(&result.capture_name, YELLOW, color);
    let colored_text = colorize(&result.matched_text, GREEN, color);

    // Single println! call per result — avoids interleaved
    // output lines when multiple Rayon threads print simultaneously. println!
    // holds an internal lock on stdout for the duration of one call.
    format!(
        "{colored_path}:{line}:{col}  [@{colored_name}]  \"{colored_text}\"",
        colored_path = colored_path,
        line = result.start_line,
        col = result.start_col,
        colored_name = colored_name,
        colored_text = colored_text
    )
}

/// Print a single structural match result to the provided writer.
///
/// In production, pass a locked stdout handle. In tests, pass a `Vec<u8>`.
///
/// # Panics
///
/// Panics if writing to the provided writer fails.
pub fn print_match<W: Write>(result: &MatchResult, color: &ColorMode, writer: &mut W) {
    let line = format!("{}\n", format_match(result, color));
    writer.write_all(line.as_bytes()).expect("failed to write match output");
}

/// Build the summary string (printed to stderr) without emitting it.
///
/// Always formats duration as milliseconds and chooses singular/plural
/// words correctly. The `color` parameter is accepted for API symmetry but
/// is not used because summaries are never colorized.
#[must_use]
fn format_summary(matches: usize, files: usize, elapsed: Duration, _color: &ColorMode) -> String {
    let ms = elapsed.as_millis();
    if matches == 0 {
        format!(
            "No matches found across {files} {files_word} in {ms}ms",
            files = files,
            files_word = plural_file(files),
            ms = ms
        )
    } else {
        format!(
            "Found {matches} {match_word} across {files} {files_word} in {ms}ms",
            matches = matches,
            match_word = plural_match(matches),
            files = files,
            files_word = plural_file(files),
            ms = ms
        )
    }
}

/// Print a search summary line to the provided writer.
///
/// Always prints to stderr regardless of [`ColorMode`] — summary output
/// is never colorized because it is diagnostic, not structured data.
///
/// # Pluralization
///
/// "match"/"matches" and "file"/"files" are correctly singularized when
/// their count is exactly 1.
///
/// # Zero match message
///
/// When `matches` is 0, prints "No matches found" rather than "Found 0 matches"
/// so the user knows the tool completed successfully with no results — not that
/// it failed silently.
///
/// # Arguments
///
/// * `matches` — total number of [`MatchResult`] items found across all files
/// * `files`   — number of files that were successfully parsed and searched
/// * `elapsed` — wall-clock duration of the search (formatted as milliseconds)
/// * `color`   — unused for summary output; accepted for API consistency
///
/// In production, pass a locked stderr handle. In tests, pass a `Vec<u8>`.
///
/// # Panics
///
/// Panics if writing to the provided writer fails.
pub fn print_summary<W: Write>(
    matches: usize,
    files: usize,
    elapsed: Duration,
    color: &ColorMode,
    writer: &mut W,
) {
    let line = format!("{}\n", format_summary(matches, files, elapsed, color));
    writer.write_all(line.as_bytes()).expect("failed to write summary output");
}

#[cfg(test)]
mod tests {
    use super::{
        colorize, format_match, format_summary, plural_file, plural_match, print_match,
        print_summary, resolve_color_mode, ColorMode, CYAN, GREEN, YELLOW,
    };
    use crate::types::MatchResult;
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::Duration;

    fn buf_to_string(buf: Vec<u8>) -> String {
        String::from_utf8(buf).expect("output contained non-UTF-8 bytes")
    }

    fn canonical_match_result() -> MatchResult {
        MatchResult {
            file_path: PathBuf::from("src/auth/handler.rs"),
            capture_name: "fn_name".to_string(),
            matched_text: "authenticate".to_string(),
            start_line: 42,
            start_col: 4,
            end_line: 42,
            end_col: 16,
            start_byte: 0,
            end_byte: 0,
        }
    }

    struct FailWriter;

    impl Write for FailWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "simulated pipe failure"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn test_colorize_off_returns_plain() {
        assert_eq!(colorize("hello", CYAN, &ColorMode::Off), "hello");
        assert_eq!(colorize("world", YELLOW, &ColorMode::Off), "world");
        assert_eq!(colorize("", GREEN, &ColorMode::Off), "");
    }

    #[test]
    fn test_colorize_on_wraps_with_codes() {
        assert!(colorize("hello", CYAN, &ColorMode::On).starts_with("\x1b["));
        assert!(colorize("hello", CYAN, &ColorMode::On).ends_with("\x1b[0m"));
        assert!(colorize("hello", CYAN, &ColorMode::On).contains("hello"));
        assert_eq!(colorize("x", CYAN, &ColorMode::On), "\x1b[36mx\x1b[0m");
        assert_eq!(colorize("y", YELLOW, &ColorMode::On), "\x1b[33my\x1b[0m");
        assert_eq!(colorize("z", GREEN, &ColorMode::On), "\x1b[32mz\x1b[0m");
    }

    #[test]
    fn test_colorize_on_empty_produces_reset_only() {
        assert_eq!(colorize("", CYAN, &ColorMode::On), "\x1b[36m\x1b[0m");
    }

    #[test]
    fn test_resolve_color_mode_flag_wins() {
        // --no-color flag must win regardless of NO_COLOR env var state
        let r = resolve_color_mode(true);
        assert_eq!(r, ColorMode::Off);
    }

    #[test]
    fn test_resolve_color_mode_env_var_behavior() {
        // NOTE: std::env::set_var is not thread-safe when other threads read env vars
        // concurrently. In a real codebase we would use serial_test or env isolation.
        // Here we set and immediately unset within the test and accept the theoretical
        // race as a documentation artifact.
        std::env::set_var("NO_COLOR", "1");
        assert_eq!(resolve_color_mode(false), ColorMode::Off);
        std::env::remove_var("NO_COLOR");

        std::env::set_var("NO_COLOR", "true");
        assert_eq!(resolve_color_mode(false), ColorMode::Off);
        std::env::remove_var("NO_COLOR");

        std::env::set_var("NO_COLOR", "");
        assert_eq!(resolve_color_mode(false), ColorMode::On);
        std::env::remove_var("NO_COLOR");
    }

    #[test]
    fn test_resolve_color_mode_default_on() {
        std::env::remove_var("NO_COLOR");
        assert_eq!(resolve_color_mode(false), ColorMode::On);
    }

    #[test]
    fn test_plural_match() {
        assert_eq!(plural_match(0), "matches");
        assert_eq!(plural_match(1), "match");
        assert_eq!(plural_match(2), "matches");
        assert_eq!(plural_match(100), "matches");
    }

    #[test]
    fn test_plural_file() {
        assert_eq!(plural_file(0), "files");
        assert_eq!(plural_file(1), "file");
        assert_eq!(plural_file(2), "files");
        assert_eq!(plural_file(100), "files");
    }

    #[test]
    fn test_format_match_off_exact() {
        let r = MatchResult {
            file_path: PathBuf::from("src/auth.rs"),
            capture_name: "fn_name".to_string(),
            matched_text: "login".to_string(),
            start_line: 10,
            start_col: 4,
            end_line: 10,
            end_col: 9,
            start_byte: 0,
            end_byte: 0,
        };

        let out = format_match(&r, &ColorMode::Off);
        assert_eq!(out, "src/auth.rs:10:4  [@fn_name]  \"login\"");
    }

    #[test]
    fn test_format_match_on_contains_codes() {
        let r = MatchResult {
            file_path: PathBuf::from("src/auth.rs"),
            capture_name: "fn_name".to_string(),
            matched_text: "login".to_string(),
            start_line: 10,
            start_col: 4,
            end_line: 10,
            end_col: 9,
            start_byte: 0,
            end_byte: 0,
        };

        let out = format_match(&r, &ColorMode::On);
        assert!(out.contains("\x1b[36m"));
        assert!(out.contains("\x1b[33m"));
        assert!(out.contains("\x1b[32m"));
        assert!(out.contains("\x1b[0m"));
        assert!(out.contains("src/auth.rs"));
        assert!(out.contains("fn_name"));
        assert!(out.contains("login"));
        assert!(out.contains(":10:4"));
        assert!(out.contains('"'));
    }

    #[test]
    fn test_format_match_path_with_spaces() {
        let r = MatchResult {
            file_path: PathBuf::from("src/my project/auth handler.rs"),
            capture_name: "fn_name".to_string(),
            matched_text: "login".to_string(),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 5,
            start_byte: 0,
            end_byte: 0,
        };

        let out = format_match(&r, &ColorMode::Off);
        assert!(out.starts_with("src/my project/auth handler.rs:"));
    }

    #[test]
    fn test_format_summary_zero_matches() {
        let out = format_summary(0, 15, Duration::from_millis(42), &ColorMode::Off);
        assert_eq!(out, "No matches found across 15 files in 42ms");
    }

    #[test]
    fn test_format_summary_singular() {
        let out = format_summary(1, 1, Duration::from_millis(7), &ColorMode::Off);
        assert_eq!(out, "Found 1 match across 1 file in 7ms");
    }

    #[test]
    fn test_format_summary_plural() {
        let out = format_summary(47, 312, Duration::from_millis(156), &ColorMode::Off);
        assert_eq!(out, "Found 47 matches across 312 files in 156ms");
    }

    #[test]
    fn test_format_summary_duration_rounding() {
        let out = format_summary(1, 1, Duration::from_micros(1500), &ColorMode::Off);
        assert!(out.contains("in 1ms"));
    }

    #[test]
    fn test_format_summary_color_invariant() {
        let a = format_summary(5, 10, Duration::from_millis(20), &ColorMode::On);
        let b = format_summary(5, 10, Duration::from_millis(20), &ColorMode::Off);
        assert_eq!(a, b);
    }

    #[test]
    fn test_print_match_writes_to_provided_writer() {
        let result = canonical_match_result();
        let mut buf: Vec<u8> = Vec::new();
        print_match(&result, &ColorMode::Off, &mut buf);

        let output = buf_to_string(buf);
        assert!(!output.is_empty());
        assert!(output.contains("src/auth/handler.rs"));
        assert!(output.contains("fn_name"));
        assert!(output.contains("authenticate"));
    }

    #[test]
    fn test_print_match_output_equals_format_match_plus_newline() {
        let result = canonical_match_result();
        let mut buf: Vec<u8> = Vec::new();
        print_match(&result, &ColorMode::Off, &mut buf);
        let expected = format!("{}\n", format_match(&result, &ColorMode::Off));

        assert_eq!(buf_to_string(buf), expected);
    }

    #[test]
    fn test_print_match_color_off_no_ansi_in_writer() {
        let result = canonical_match_result();
        let mut buf: Vec<u8> = Vec::new();
        print_match(&result, &ColorMode::Off, &mut buf);
        let output = buf_to_string(buf);

        assert!(!output.contains("\x1b["));
        assert!(output.contains("src/auth/handler.rs"));
        assert!(output.contains("fn_name"));
        assert!(output.contains("authenticate"));
    }

    #[test]
    fn test_print_match_color_on_ansi_present_in_writer() {
        let result = canonical_match_result();
        let mut buf: Vec<u8> = Vec::new();
        print_match(&result, &ColorMode::On, &mut buf);
        let output = buf_to_string(buf);

        assert!(output.contains("\x1b[36m"));
        assert!(output.contains("\x1b[33m"));
        assert!(output.contains("\x1b[32m"));
        assert!(output.contains("\x1b[0m"));
        assert!(output.contains("src/auth/handler.rs"));
        assert!(output.contains("authenticate"));
        assert!(output.contains(":42:4"));
        assert!(!output.contains(":42:4\x1b"));
    }

    #[test]
    fn test_print_match_writes_exactly_one_newline() {
        let result = canonical_match_result();
        let mut buf: Vec<u8> = Vec::new();
        print_match(&result, &ColorMode::Off, &mut buf);
        let output = buf_to_string(buf);

        assert!(output.ends_with('\n'));
        assert_eq!(output.chars().filter(|&c| c == '\n').count(), 1);
    }

    #[test]
    fn test_print_match_multiple_results_accumulate_in_writer() {
        let result1 = MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            capture_name: "fn_name".to_string(),
            matched_text: "alpha".to_string(),
            start_line: 1,
            start_col: 3,
            end_line: 1,
            end_col: 8,
            start_byte: 0,
            end_byte: 0,
        };
        let result2 = MatchResult {
            file_path: PathBuf::from("src/b.rs"),
            capture_name: "fn_name".to_string(),
            matched_text: "beta".to_string(),
            start_line: 5,
            start_col: 3,
            end_line: 5,
            end_col: 7,
            start_byte: 0,
            end_byte: 0,
        };
        let mut buf: Vec<u8> = Vec::new();
        print_match(&result1, &ColorMode::Off, &mut buf);
        print_match(&result2, &ColorMode::Off, &mut buf);
        let output = buf_to_string(buf);

        assert!(output.contains("src/a.rs"));
        assert!(output.contains("alpha"));
        assert!(output.contains("src/b.rs"));
        assert!(output.contains("beta"));
        assert_eq!(output.lines().count(), 2);
        assert!(output.lines().next().unwrap().contains("alpha"));
        assert!(output.lines().nth(1).unwrap().contains("beta"));
    }

    #[test]
    fn test_print_match_empty_matched_text() {
        let result = MatchResult {
            file_path: PathBuf::from("src/main.rs"),
            capture_name: "optional_cap".to_string(),
            matched_text: String::new(),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 0,
            start_byte: 0,
            end_byte: 0,
        };
        let mut buf: Vec<u8> = Vec::new();
        print_match(&result, &ColorMode::Off, &mut buf);
        let output = buf_to_string(buf);

        assert!(output.contains("optional_cap"));
        assert!(output.contains("\"\""));
    }

    #[test]
    fn test_print_summary_writes_to_provided_writer() {
        let mut buf: Vec<u8> = Vec::new();
        print_summary(5, 20, Duration::from_millis(38), &ColorMode::Off, &mut buf);
        let output = buf_to_string(buf);

        assert!(!output.is_empty());
        assert!(output.contains("5"));
        assert!(output.contains("20"));
        assert!(output.contains("38ms"));
    }

    #[test]
    fn test_print_summary_output_equals_format_summary_plus_newline() {
        let elapsed = Duration::from_millis(123);
        let mut buf: Vec<u8> = Vec::new();
        print_summary(10, 50, elapsed, &ColorMode::Off, &mut buf);
        let expected = format!("{}\n", format_summary(10, 50, elapsed, &ColorMode::Off));

        assert_eq!(buf_to_string(buf), expected);
    }

    #[test]
    fn test_print_summary_zero_matches_in_writer() {
        let mut buf: Vec<u8> = Vec::new();
        print_summary(0, 8, Duration::from_millis(12), &ColorMode::Off, &mut buf);
        let output = buf_to_string(buf);

        assert!(output.contains("No matches"));
        assert!(!output.contains("Found 0"));
        assert!(output.contains("8"));
        assert!(output.contains("12ms"));
    }

    #[test]
    fn test_print_summary_singular_forms_in_writer() {
        let mut buf: Vec<u8> = Vec::new();
        print_summary(1, 1, Duration::from_millis(3), &ColorMode::Off, &mut buf);
        let output = buf_to_string(buf);

        assert!(output.contains("1 match"));
        assert!(!output.contains("matches"));
        assert!(output.contains("1 file"));
        assert!(!output.contains("files"));
        assert!(output.contains("3ms"));
    }

    #[test]
    fn test_print_summary_writes_exactly_one_newline() {
        let mut buf: Vec<u8> = Vec::new();
        print_summary(3, 10, Duration::from_millis(5), &ColorMode::Off, &mut buf);
        let output = buf_to_string(buf);

        assert!(output.ends_with('\n'));
        assert_eq!(output.chars().filter(|&c| c == '\n').count(), 1);
    }

    #[test]
    fn test_print_match_and_summary_write_to_independent_writers() {
        let result = canonical_match_result();
        let mut match_buf: Vec<u8> = Vec::new();
        let mut summary_buf: Vec<u8> = Vec::new();
        print_match(&result, &ColorMode::Off, &mut match_buf);
        print_summary(1, 1, Duration::from_millis(5), &ColorMode::Off, &mut summary_buf);

        assert!(!match_buf.is_empty());
        assert!(!summary_buf.is_empty());
        assert!(!buf_to_string(match_buf.clone()).contains("Found"));
        assert!(!buf_to_string(match_buf.clone()).contains("No matches"));
        assert!(!buf_to_string(summary_buf.clone()).contains("[@fn_name]"));
        assert!(buf_to_string(match_buf).contains("authenticate"));
        assert!(buf_to_string(summary_buf).contains("1 match"));
    }

    #[test]
    #[should_panic(expected = "failed to write match output")]
    fn test_print_match_writer_error_panics_with_message() {
        let result = canonical_match_result();
        let mut writer = FailWriter;
        print_match(&result, &ColorMode::Off, &mut writer);
    }

    #[test]
    fn test_print_match_multiline_matched_text() {
        let result = MatchResult {
            file_path: PathBuf::from("src/main.rs"),
            capture_name: "block".to_string(),
            matched_text: "fn foo() {\n    42\n}".to_string(),
            start_line: 1,
            start_col: 0,
            end_line: 3,
            end_col: 1,
            start_byte: 0,
            end_byte: 0,
        };
        let mut buf: Vec<u8> = Vec::new();
        print_match(&result, &ColorMode::Off, &mut buf);
        let output = buf_to_string(buf);

        assert!(output.contains("block"));
        assert!(output.contains("fn foo()"));
        assert!(output.lines().any(|line| line.contains("block")));
    }
}
