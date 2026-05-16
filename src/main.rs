#![deny(warnings)]
#![warn(clippy::pedantic)]

use std::{
    path::PathBuf,
    process,
    sync::{Arc, Mutex},
    time::Instant,
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
use types::{Language, MatchResult, SearchConfig};
use walker::build_walker;

#[derive(Parser, Debug)]
#[command(name = "ast-search", about = "Structural AST-based code search")]
struct Cli {
    #[arg(short = 'q', long = "query")]
    query: String,

    #[arg(short = 'p', long = "path", default_value = ".")]
    path: PathBuf,

    #[arg(short = 'l', long = "lang", default_value = "rust")]
    lang: String,

    #[arg(long = "no-color", default_value_t = false)]
    no_color: bool,
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

#[must_use]
#[allow(clippy::too_many_lines)]
fn run_search(
    config: &SearchConfig,
    query: &Arc<tree_sitter::Query>,
    color: &ColorMode,
) -> (Vec<MatchResult>, usize) {
    let _ = color;

    let results = Arc::new(Mutex::new(Vec::<MatchResult>::new()));
    let file_count = Arc::new(Mutex::new(0usize));

    let results_ref = Arc::clone(&results);
    let file_count_ref = Arc::clone(&file_count);
    let query_ref = Arc::clone(query);


    build_walker(config.root_path.as_path(), &config.language).par_bridge().for_each(
        move |entry_result| match entry_result {
            Ok(entry) => match parse_file(entry.path()) {
                Ok((tree, source)) => {
                    let matches = extract_matches(&tree, &source, query_ref.as_ref(), entry.path());
                    drop(tree);
                    drop(source);

                    let mut results_guard = results_ref
                        .lock()
                        .expect("results Mutex was poisoned by a panicked thread");
                    results_guard.extend(matches);

                    let mut count_guard = file_count_ref
                        .lock()
                        .expect("file_count Mutex was poisoned by a panicked thread");
                    *count_guard += 1;
                }
                Err(error) => eprintln!("warning: {error}"),
            },
            Err(error) => eprintln!("warning: {error}"),
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
    let processed_files = {
        match Arc::try_unwrap(file_count) {
            Ok(mutex) => {
                mutex.into_inner().expect("file_count Mutex was poisoned by a panicked thread")
            }
            Err(shared) => {
                *shared.lock().expect("file_count Mutex was poisoned by a panicked thread")
            }
        }
    };

    final_results.sort();
    final_results.dedup();

    (final_results, processed_files)
}

fn main() {
    let cli = Cli::parse();

    let color = resolve_color_mode(cli.no_color);

    if let Err(error) = cli.validate() {
        eprintln!("error: {error}");
        process::exit(1);
    }

    let walker_language = match resolve_lang(&cli.lang) {
        Ok(language) => language,
        Err(error) => {
            eprintln!("error: {error}");
            process::exit(1);
        }
    };

    let ts_language = match get_ts_language(&cli.lang) {
        Ok(language) => language,
        Err(error) => {
            eprintln!("error: {error}");
            process::exit(1);
        }
    };

    let config = SearchConfig {
        query_str: cli.query.clone(),
        root_path: cli.path.clone(),
        language: walker_language,
    };

    let query = match compile_query(&ts_language, &config.query_str) {
        Ok(query) => query,
        Err(error) => {
            eprintln!("error: failed to compile query: {error}");
            process::exit(1);
        }
    };

    let started_at = Instant::now();

    let (results, processed_files) = run_search(&config, &query, &color);

    let mut stdout = std::io::stdout().lock();
    for result in &results {
        print_match(result, &color, &mut stdout);
    }

    let mut stderr = std::io::stderr().lock();
    print_summary(results.len(), processed_files, started_at.elapsed(), &color, &mut stderr);
}
