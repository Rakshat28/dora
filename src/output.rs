use crate::types::MatchResult;

pub fn print_match(result: &MatchResult) {
    println!(
        "{}:{}:{}  [@{}]  {:?}",
        result.file_path.display(),
        result.start_line,
        result.start_col,
        result.capture_name,
        result.matched_text
    );
}

pub fn print_summary(matches: usize, files: usize, duration_ms: u128) {
    eprintln!("Found {matches} matches across {files} files in {duration_ms}ms");
}
