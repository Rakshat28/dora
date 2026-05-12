#![deny(warnings)]
#![warn(clippy::pedantic)]

use std::{path::PathBuf, process, time::Instant};

use clap::Parser;
use rayon::prelude::*;

mod output;
mod parser;
mod query;
mod types;
mod walker;

use output::{print_match, print_summary};
use parser::parse_file;
use query::{compile_query, extract_matches};
use types::MatchResult;
use walker::walk_rust_files;

#[derive(Parser, Debug)]
#[command(name = "ast-search", about = "Structural AST-based code search")]
struct Cli {
    /// An S-expression query string.
    #[arg(short = 'q', long = "query")]
    query: String,

    /// Root directory to search.
    #[arg(short = 'p', long = "path", default_value = ".")]
    path: PathBuf,

    /// Language to parse.
    #[arg(short = 'l', long = "lang", default_value = "rust")]
    lang: String,
}

fn main() {
    let cli = Cli::parse();

    if cli.lang != "rust" {
        eprintln!("error: only rust is supported in the MVP");
        process::exit(1);
    }

    let language = tree_sitter_rust::language();
    let query = match compile_query(&language, &cli.query) {
        Ok(query) => query,
        Err(error) => {
            eprintln!("error: failed to compile query: {}", error);
            process::exit(1);
        }
    };

    let started_at = Instant::now();

    let (mut results, processed_files) = walk_rust_files(&cli.path)
        .par_bridge()
        .fold(
            || (Vec::<MatchResult>::new(), 0usize),
            |mut acc, path| {
                if let Some((tree, source)) = parse_file(&path) {
                    let file_path = path.to_string_lossy().into_owned();
                    let matches = extract_matches(&tree, &source, query.as_ref(), &file_path);
                    acc.0.extend(matches);
                    acc.1 += 1;
                }

                acc
            },
        )
        .reduce(
            || (Vec::<MatchResult>::new(), 0usize),
            |mut left, right| {
                left.0.extend(right.0);
                left.1 += right.1;
                left
            },
        );

    results.sort_by(|left, right| {
        left.file_path
            .cmp(&right.file_path)
            .then(left.start_line.cmp(&right.start_line))
            .then(left.start_col.cmp(&right.start_col))
            .then(left.capture_name.cmp(&right.capture_name))
            .then(left.end_line.cmp(&right.end_line))
            .then(left.end_col.cmp(&right.end_col))
    });

    for result in &results {
        print_match(result);
    }

    print_summary(results.len(), processed_files, started_at.elapsed().as_millis());
}
