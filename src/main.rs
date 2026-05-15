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
use types::{AppError, MatchResult};
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
        eprintln!("error: {}", AppError::LanguageNotSupported(cli.lang));
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
                match parse_file(&path) {
                    Ok((tree, source)) => {
                        let matches = extract_matches(&tree, &source, query.as_ref(), &path);
                        acc.0.extend(matches);
                        acc.1 += 1;
                    }
                    Err(error) => {
                        eprintln!("warning: {}", error);
                    }
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

    results.sort();

    for result in &results {
        print_match(result);
    }

    print_summary(results.len(), processed_files, started_at.elapsed().as_millis());
}
