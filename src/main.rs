#![deny(warnings)]
#![warn(clippy::pedantic)]

use std::{path::PathBuf, process, time::Instant};

use clap::Parser;
use rayon::prelude::*;

mod output;
mod parser;
mod query;
mod types;
pub mod walker;

use output::{print_match, print_summary};
use parser::{get_language, parse_file};
use query::{compile_query, extract_matches};
use types::{Language, MatchResult};
use walker::build_walker;

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

    let language = match get_language(&cli.lang) {
        Ok(language) => language,
        Err(error) => {
            eprintln!("error: {error}");
            process::exit(1);
        }
    };
    let query = match compile_query(&language, &cli.query) {
        Ok(query) => query,
        Err(error) => {
            eprintln!("error: failed to compile query: {error}");
            process::exit(1);
        }
    };

    let started_at = Instant::now();

    let (mut results, processed_files) = build_walker(&cli.path, &Language::Rust)
        .par_bridge()
        .fold(
            || (Vec::<MatchResult>::new(), 0usize),
            |mut acc, entry_result| {
                match entry_result {
                    Ok(entry) => match parse_file(entry.path()) {
                        Ok((tree, source)) => {
                            let matches = extract_matches(&tree, &source, query.as_ref(), entry.path());
                            acc.0.extend(matches);
                            acc.1 += 1;
                        }
                        Err(error) => {
                            eprintln!("warning: {error}");
                        }
                    },
                    Err(error) => {
                        eprintln!("warning: {error}");
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
