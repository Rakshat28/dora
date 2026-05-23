#![deny(warnings)]
#![warn(clippy::pedantic)]

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use rayon::prelude::*;

mod bloom;
mod index;
mod mcp;
#[allow(dead_code)]
mod memory;
mod output;
mod parser;
mod query;
mod sieve;
mod trigram;
mod tui;
mod types;
pub mod walker;

use bloom::BloomFilter;
use index::{index_path_for_root, load_index, save_index, IndexEntry, IndexManifest};
use mcp::{run as run_mcp, McpConfig};
use memory::{memory_db_path, FileRow, MemoryDb, SymbolKind, SymbolRow};
use output::{print_lookup_results, print_match, print_summary, resolve_color_mode, ColorMode};
use parser::{detect_language, get_all_languages, parse_file_with_metadata};
use sieve::{
    build_query_trigram_set, get_file_index_status, should_parse_file, FileIndexStatus,
    QueryTrigramSet,
};
use trigram::extract_unique_trigrams_from_bytes;
use types::{AppError, LangMode, Language, MatchResult, SearchConfig};
use walker::{build_auto_walker, build_walker};

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
    name = "dora",
    version,
    author,
    about = "Structural AST-based code search — find code by shape, not by text.",
    long_about = "dora parses source files into Abstract Syntax Trees and \
                  executes structural pattern queries against them.\n\n\
                  Unlike grep or ripgrep, dora understands code grammar. \
                  It can find function definitions, not just strings that look \
                  like them. It ignores matches inside comments, string literals, \
                  and dead code.\n\n\
                  Queries use Tree-sitter S-expression syntax:\n\
                  \n  \
                  dora -q '(function_item name: (identifier) @fn)' -p ./src\n\n\
                  See https://github.com/your-org/dora for full documentation."
)]
struct App {
    #[command(flatten)]
    cli: Cli,

    #[arg(long = "mcp", default_value_t = false, help = "Run the MCP server over stdio")]
    mcp: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Lookup(LookupArgs),
}

#[derive(Args, Debug)]
#[command(next_line_help = true)]
struct Cli {
    #[arg(
        short = 'q',
        long = "query",
        num_args = 1..,
        value_name = "S-EXPR",
        help = "Tree-sitter S-expression query (repeatable: -q QUERY1 -q QUERY2)",
        long_help = "An S-expression structural query in Tree-sitter syntax.\n\n\
                     Examples:\n\
                     \n  Find all function definitions:\n  \
                     (function_item name: (identifier) @fn_name)\
                     \n\n  Find a specific function:\n  \
                     (function_item name: (identifier) @fn (#eq? @fn \"connect\"))\
                     \n\n  Find all struct definitions:\n  \
                     (struct_item name: (type_identifier) @struct_name)"
    )]
    query: Vec<String>,

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
        default_value = "auto",
        help = "Language to parse: rust, python, js, ts, go, c, cpp, auto (default: auto)"
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
        long = "tui",
        default_value_t = false,
        help = "Launch the interactive terminal UI instead of stdout output"
    )]
    tui: bool,

    #[arg(
        long = "stats",
        default_value_t = false,
        help = "Print detailed performance statistics to stderr after the search"
    )]
    stats: bool,

    #[arg(
        long = "no-update-index",
        default_value_t = false,
        help = "Do not refresh the on-disk index during search"
    )]
    no_update_index: bool,

    #[arg(
        long = "generate-completions",
        value_name = "SHELL",
        hide = true,
        help = "Generate shell completion script for the specified shell"
    )]
    generate_completions: Option<Shell>,

    #[arg(
        long = "rewrite",
        value_name = "TEMPLATE",
        help = "Rewrite matched captures using this template. @capture_name is substituted."
    )]
    rewrite: Option<String>,

    #[arg(
        long = "in-place",
        default_value_t = false,
        requires = "rewrite",
        help = "Apply rewrites to files in place. Requires --rewrite."
    )]
    in_place: bool,

    #[arg(
        long = "yes",
        default_value_t = false,
        requires = "in_place",
        help = "Skip confirmation prompt when using --in-place."
    )]
    yes: bool,
}

#[derive(Args, Debug)]
struct LookupArgs {
    #[arg(long = "symbol", value_name = "NAME", help = "Lookup an exact symbol name")]
    symbol: Option<String>,

    #[arg(long = "prefix", value_name = "PREFIX", help = "Lookup symbols by name prefix")]
    prefix: Option<String>,

    #[arg(long = "kind", value_name = "KIND", help = "Restrict matches to a symbol kind")]
    kind: Option<String>,

    #[arg(
        short = 'p',
        long = "path",
        value_name = "DIR",
        default_value = ".",
        help = "Root directory whose persisted index should be queried"
    )]
    path: PathBuf,

    #[arg(
        long = "lang",
        value_name = "LANG",
        default_value = "auto",
        help = "Language to filter after lookup: rust, python, js, ts, go, c, cpp, auto"
    )]
    lang: String,

    #[arg(long = "no-color", default_value_t = false, help = "Disable ANSI color output")]
    no_color: bool,
}

struct SearchOutcome {
    results: Vec<MatchResult>,
    files_walked: usize,
    files_parsed: usize,
    files_skipped: usize,
    sieve_rejected: usize,
    index_entries_updated: usize,
    files_with_matches: usize,
}

impl Cli {
    fn validate(&self) -> std::result::Result<(), String> {
        self.validate_internal(false)
    }

    fn validate_allowing_empty_query(&self) -> std::result::Result<(), String> {
        self.validate_internal(true)
    }

    fn validate_internal(&self, allow_empty_query: bool) -> std::result::Result<(), String> {
        if self.generate_completions.is_some() {
            return Ok(());
        }

        if !allow_empty_query && self.query.iter().all(|q| q.trim().is_empty()) {
            return Err("at least one query string must not be empty".to_string());
        }

        if !self.path.exists() {
            return Err(format!(
                "path does not exist: {}\n  hint: check for typos or run from the correct directory",
                self.path.display()
            ));
        }

        if !self.path.is_dir() {
            return Err(format!(
                "path is not a directory: {}\n  hint: --path must point to a directory, not a file",
                self.path.display()
            ));
        }

        if let Some(t) = &self.rewrite {
            if t.trim().is_empty() {
                return Err("rewrite template must not be empty".to_string());
            }
        }

        let supported = ["rust", "python", "js", "ts", "go", "c", "cpp", "auto"];
        if !supported.contains(&self.lang.as_str()) {
            return Err(format!(
                "unsupported language: '{}'
      supported languages: rust, python, js, ts, go, c, cpp, auto
      example: --lang rust",
                self.lang
            ));
        }

        Ok(())
    }
}

impl LookupArgs {
    fn validate(&self) -> std::result::Result<(), String> {
        let symbol_present = self.symbol.as_ref().is_some_and(|value| !value.trim().is_empty());
        let prefix_present = self.prefix.as_ref().is_some_and(|value| !value.trim().is_empty());

        if symbol_present == prefix_present {
            return Err("specify exactly one of --symbol or --prefix".to_string());
        }

        if !self.path.exists() {
            return Err(format!(
                "path does not exist: {}\n  hint: check for typos or run from the correct directory",
                self.path.display()
            ));
        }

        if !self.path.is_dir() {
            return Err(format!(
                "path is not a directory: {}\n  hint: --path must point to a directory, not a file",
                self.path.display()
            ));
        }

        let supported = ["rust", "python", "js", "ts", "go", "c", "cpp", "auto"];
        if !supported.contains(&self.lang.as_str()) {
            return Err(format!(
                "unsupported language: '{}'\n      supported languages: rust, python, js, ts, go, c, cpp, auto\n      example: --lang rust",
                self.lang
            ));
        }

        if let Some(kind) = &self.kind {
            parse_lookup_kind(kind)?;
        }

        Ok(())
    }
}

fn parse_lookup_kind(kind: &str) -> std::result::Result<SymbolKind, String> {
    match kind.trim().to_lowercase().as_str() {
        "function" => Ok(SymbolKind::Function),
        "method" => Ok(SymbolKind::Method),
        "struct" => Ok(SymbolKind::Struct),
        "enum" => Ok(SymbolKind::Enum),
        "trait" => Ok(SymbolKind::Trait),
        "interface" => Ok(SymbolKind::Interface),
        "typealias" => Ok(SymbolKind::TypeAlias),
        "constant" => Ok(SymbolKind::Constant),
        "variable" => Ok(SymbolKind::Variable),
        "class" => Ok(SymbolKind::Class),
        "module" => Ok(SymbolKind::Module),
        "import" => Ok(SymbolKind::Import),
        "unknown" => Ok(SymbolKind::Unknown),
        _ => Err(format!(
            "unsupported kind: '{}'\n      supported kinds: function, method, struct, enum, trait, interface, typealias, constant, variable, class, module, import, unknown",
            kind
        )),
    }
}

fn lookup_db_path(root: &Path) -> PathBuf {
    memory_db_path(root)
}

fn open_lookup_db(root: &Path) -> std::result::Result<MemoryDb, String> {
    let db_path = lookup_db_path(root);
    if !db_path.exists() {
        return Err(format!(
            "no structural index found at {}\n  hint: run dora --persist {} first",
            db_path.display(),
            root.display()
        ));
    }

    MemoryDb::open(&db_path).map_err(|error| format!("error: {error}"))
}

fn filter_lookup_rows_by_language(
    rows: Vec<(SymbolRow, FileRow)>,
    lang: &str,
) -> std::result::Result<Vec<(SymbolRow, FileRow)>, String> {
    if lang == "auto" {
        return Ok(rows);
    }

    let desired = lang.to_string();
    Ok(rows.into_iter().filter(|(_, file)| file.language == desired).collect())
}

fn execute_symbol_lookup(
    db: &MemoryDb,
    symbol: &str,
    kind: Option<&SymbolKind>,
    lang: &str,
) -> std::result::Result<Vec<(SymbolRow, FileRow)>, String> {
    let symbols = match kind {
        Some(kind) => db
            .find_symbols_by_name_and_kind(symbol, kind)
            .map_err(|error| format!("error: {error}"))?,
        None => db.find_symbols_by_name(symbol).map_err(|error| format!("error: {error}"))?,
    };

    collect_lookup_rows(db, symbols, lang)
}

fn execute_prefix_lookup(
    db: &MemoryDb,
    prefix: &str,
    kind: Option<&SymbolKind>,
    lang: &str,
) -> std::result::Result<Vec<(SymbolRow, FileRow)>, String> {
    let symbols =
        db.find_symbols_by_name_prefix(prefix).map_err(|error| format!("error: {error}"))?;
    let symbols = if let Some(kind) = kind {
        symbols.into_iter().filter(|symbol| &symbol.kind == kind).collect()
    } else {
        symbols
    };

    collect_lookup_rows(db, symbols, lang)
}

fn collect_lookup_rows(
    db: &MemoryDb,
    symbols: Vec<SymbolRow>,
    lang: &str,
) -> std::result::Result<Vec<(SymbolRow, FileRow)>, String> {
    let mut rows = Vec::new();

    for symbol in symbols {
        let file = db
            .get_file_by_id(symbol.file_id)
            .map_err(|error| format!("error: {error}"))?
            .ok_or_else(|| format!("error: missing file row for file_id {}", symbol.file_id))?;
        rows.push((symbol, file));
    }

    let mut filtered = filter_lookup_rows_by_language(rows, lang)?;
    filtered.sort_by(|left, right| {
        left.1
            .path
            .cmp(&right.1.path)
            .then_with(|| left.0.start_line.cmp(&right.0.start_line))
            .then_with(|| left.0.start_col.cmp(&right.0.start_col))
            .then_with(|| left.0.name.cmp(&right.0.name))
    });
    Ok(filtered)
}

fn run_lookup_mode(args: &LookupArgs) {
    if let Err(message) = args.validate() {
        eprintln!("error: {message}");
        process::exit(1);
    }

    let color = resolve_color_mode(args.no_color);
    let db = match open_lookup_db(&args.path) {
        Ok(db) => db,
        Err(message) => {
            eprintln!("error: {message}");
            process::exit(1);
        }
    };

    let kind = match args.kind.as_deref() {
        Some(kind) => match parse_lookup_kind(kind) {
            Ok(kind) => Some(kind),
            Err(message) => {
                eprintln!("error: {message}");
                process::exit(1);
            }
        },
        None => None,
    };

    let results = match (args.symbol.as_deref(), args.prefix.as_deref()) {
        (Some(symbol), None) => execute_symbol_lookup(&db, symbol, kind.as_ref(), &args.lang),
        (None, Some(prefix)) => execute_prefix_lookup(&db, prefix, kind.as_ref(), &args.lang),
        _ => Err("specify exactly one of --symbol or --prefix".to_string()),
    };

    let results = match results {
        Ok(results) => results,
        Err(message) => {
            eprintln!("error: {message}");
            process::exit(1);
        }
    };

    let mut stdout = std::io::stdout().lock();
    print_lookup_results(&results, &color, &mut stdout);
}

fn resolve_root_path(path: &Path) -> PathBuf {
    match fs::canonicalize(path) {
        Ok(resolved) => resolved,
        Err(_) => path.to_path_buf(),
    }
}

fn resolve_lang(lang_str: &str) -> Language {
    match lang_str {
        "rust" => Language::Rust,
        "python" => Language::Python,
        "js" => Language::JavaScript,
        "ts" => Language::TypeScript,
        "go" => Language::Go,
        "c" => Language::C,
        "cpp" => Language::Cpp,
        _ => unreachable!("validate() should have rejected lang: {}", lang_str),
    }
}

fn resolve_lang_mode(lang_str: &str) -> LangMode {
    match lang_str {
        "auto" => LangMode::Auto,
        other => LangMode::Single(resolve_lang(other)),
    }
}

fn lang_to_ts_language(lang: &Language) -> tree_sitter::Language {
    match lang {
        Language::Rust => tree_sitter_rust::language(),
        Language::Python => tree_sitter_python::language(),
        Language::JavaScript => tree_sitter_javascript::language(),
        Language::TypeScript => tree_sitter_typescript::language_tsx(),
        Language::Go => tree_sitter_go::language(),
        Language::C => tree_sitter_c::language(),
        Language::Cpp => tree_sitter_cpp::language(),
    }
}

fn language_to_index_name(lang: &Language) -> &'static str {
    match lang {
        Language::Rust => "rust",
        Language::Python => "python",
        Language::JavaScript => "js",
        Language::TypeScript => "ts",
        Language::Go => "go",
        Language::C => "c",
        Language::Cpp => "cpp",
    }
}

fn build_compiled_queries(
    config: &SearchConfig,
) -> HashMap<Language, Arc<query::MultiCompiledQuery>> {
    match &config.lang_mode {
        LangMode::Single(lang) => {
            let ts_lang = lang_to_ts_language(lang);
            match query::compile_multi_query(&ts_lang, &config.queries) {
                Ok(compiled) => HashMap::from([(lang.clone(), compiled)]),
                Err(error) => {
                    eprintln!("error: {error}");
                    process::exit(1);
                }
            }
        }
        LangMode::Auto => {
            let mut map = HashMap::new();
            for (lang, ts_lang) in get_all_languages() {
                if let Ok(compiled) = query::compile_multi_query(&ts_lang, &config.queries) {
                    map.insert(lang, compiled);
                }
            }
            if map.is_empty() {
                eprintln!(
                        "error: query did not compile against any supported language\n  query: {}\n  hint: check the S-expression syntax and node type names",
                        config.queries.join("\n  query: ")
                    );
                process::exit(1);
            }
            map
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
    eprintln!("{:<18}{}", "sieve rejected:", outcome.sieve_rejected);
    eprintln!("{:<18}{}", "matches found:", outcome.results.len());
    eprintln!("{:<18}{}", "index updates:", outcome.index_entries_updated);
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
    compiled_queries: &Arc<HashMap<Language, Arc<query::MultiCompiledQuery>>>,
    query_trigram_set: &Arc<QueryTrigramSet>,
    index_manifest: &Arc<Mutex<IndexManifest>>,
    color: &ColorMode,
    quiet: bool,
    no_update_index: bool,
) -> SearchOutcome {
    let _ = color;
    let _ = quiet;

    let results = Arc::new(Mutex::new(Vec::<MatchResult>::new()));
    let files_walked_count = Arc::new(Mutex::new(0usize));
    let files_parsed_count = Arc::new(Mutex::new(0usize));
    let files_skipped_count = Arc::new(Mutex::new(0usize));
    let sieve_rejected_count = Arc::new(Mutex::new(0usize));
    let index_entries_updated_count = Arc::new(Mutex::new(0usize));

    let results_ref = Arc::clone(&results);
    let files_walked_ref = Arc::clone(&files_walked_count);
    let files_parsed_ref = Arc::clone(&files_parsed_count);
    let files_skipped_ref = Arc::clone(&files_skipped_count);
    let sieve_rejected_ref = Arc::clone(&sieve_rejected_count);
    let index_entries_updated_ref = Arc::clone(&index_entries_updated_count);
    let compiled_queries_ref = Arc::clone(compiled_queries);
    let query_trigram_set_ref = Arc::clone(query_trigram_set);
    let index_manifest_ref = Arc::clone(index_manifest);

    let walker: Box<dyn Iterator<Item = crate::types::Result<ignore::DirEntry>> + Send> =
        match &config.lang_mode {
            LangMode::Single(lang) => Box::new(build_walker(config.root_path.as_path(), lang)),
            LangMode::Auto => Box::new(build_auto_walker(config.root_path.as_path())),
        };

    walker.par_bridge().for_each(move |entry_result| match entry_result {
        Ok(entry) => {
            *files_walked_ref
                .lock()
                .expect("files_walked Mutex was poisoned by a panicked thread") += 1;

            let detected_lang = match &config.lang_mode {
                LangMode::Single(lang) => lang.clone(),
                LangMode::Auto => match detect_language(entry.path()) {
                    Some(lang) => lang,
                    None => return,
                },
            };

            let ts_query = match compiled_queries_ref.get(&detected_lang) {
                Some(query) => Arc::clone(query),
                None => return,
            };

            let ts_lang = lang_to_ts_language(&detected_lang);
            let metadata = match fs::metadata(entry.path()) {
                Ok(metadata) => metadata,
                Err(error) => {
                    handle_file_error(
                        &FileError::ReadFailure {
                            path: entry.path().to_path_buf(),
                            message: error.to_string(),
                        },
                        &files_skipped_ref,
                    );
                    return;
                }
            };

            let file_index_status = {
                let manifest_guard = index_manifest_ref
                    .lock()
                    .expect("index_manifest Mutex was poisoned by a panicked thread");
                get_file_index_status(&manifest_guard, entry.path(), &metadata)
            };

            if let FileIndexStatus::Fresh(filter) = &file_index_status {
                if !should_parse_file(filter, &query_trigram_set_ref) {
                    *sieve_rejected_ref
                        .lock()
                        .expect("sieve_rejected Mutex was poisoned by a panicked thread") += 1;
                    return;
                }
            }

            match parse_file_with_metadata(entry.path(), &ts_lang, &metadata) {
                Ok((tree, source)) => {
                    let source_bytes = source.as_bytes();
                    let matches = query::extract_multi_matches(
                        &tree,
                        source_bytes,
                        ts_query.as_ref(),
                        entry.path(),
                    );

                    let mut results_guard = results_ref
                        .lock()
                        .expect("results Mutex was poisoned by a panicked thread");
                    results_guard.extend(matches);

                    let mut count_guard = files_parsed_ref
                        .lock()
                        .expect("files_parsed Mutex was poisoned by a panicked thread");
                    *count_guard += 1;

                    if !no_update_index
                        && matches!(
                            file_index_status,
                            FileIndexStatus::Stale | FileIndexStatus::NotIndexed
                        )
                    {
                        let mut bloom_filter = BloomFilter::new();
                        bloom_filter
                            .insert_trigrams(&extract_unique_trigrams_from_bytes(source_bytes));
                        let index_entry = IndexEntry {
                            path: entry.path().to_path_buf(),
                            mtime_secs: metadata
                                .modified()
                                .ok()
                                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                                .map(|duration| duration.as_secs())
                                .unwrap_or_default(),
                            file_size_bytes: metadata.len(),
                            bloom_bits: bloom_filter.to_bytes().to_vec(),
                            language: language_to_index_name(&detected_lang).to_string(),
                        };
                        let mut manifest_guard = index_manifest_ref
                            .lock()
                            .expect("index_manifest Mutex was poisoned by a panicked thread");
                        manifest_guard.upsert_entry(index_entry);
                        *index_entries_updated_ref.lock().expect(
                            "index_entries_updated Mutex was poisoned by a panicked thread",
                        ) += 1;
                    }

                    drop(tree);
                    drop(source);
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
    });

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
    let sieve_rejected = {
        match Arc::try_unwrap(sieve_rejected_count) {
            Ok(mutex) => {
                mutex.into_inner().expect("sieve_rejected Mutex was poisoned by a panicked thread")
            }
            Err(shared) => {
                *shared.lock().expect("sieve_rejected Mutex was poisoned by a panicked thread")
            }
        }
    };
    let index_entries_updated = {
        match Arc::try_unwrap(index_entries_updated_count) {
            Ok(mutex) => mutex
                .into_inner()
                .expect("index_entries_updated Mutex was poisoned by a panicked thread"),
            Err(shared) => *shared
                .lock()
                .expect("index_entries_updated Mutex was poisoned by a panicked thread"),
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
        sieve_rejected,
        index_entries_updated,
        files_with_matches,
    }
}

fn main() {
    let app = App::parse();
    let cli = &app.cli;

    if let Some(Commands::Lookup(args)) = &app.command {
        run_lookup_mode(args);
        return;
    }

    if app.mcp {
        if let Err(message) = cli.validate_allowing_empty_query() {
            eprintln!("error: {message}");
            process::exit(1);
        }

        let root_path = resolve_root_path(&cli.path);
        let mcp_config = McpConfig {
            root_path: root_path.clone(),
            db_path: memory_db_path(&root_path),
            lang_mode: resolve_lang_mode(&cli.lang),
        };

        if let Err(error) = run_mcp(mcp_config) {
            eprintln!("error: {error}");
            process::exit(1);
        }
        return;
    }

    if let Some(shell) = cli.generate_completions {
        let mut cmd = App::command();
        generate(shell, &mut cmd, "dora", &mut std::io::stdout());
        process::exit(0);
    }

    let color = resolve_color_mode(cli.no_color);

    if let Err(message) = cli.validate() {
        eprintln!("error: {message}");
        process::exit(1);
    }

    let lang_mode = resolve_lang_mode(&cli.lang);

    let config =
        SearchConfig { queries: cli.query.clone(), root_path: cli.path.clone(), lang_mode };

    let compiled_queries = Arc::new(build_compiled_queries(&config));

    if cli.tui {
        match crate::tui::run_tui(&config, &compiled_queries) {
            Ok(_) => return,
            Err(err) => {
                eprintln!("error: {err}");
                process::exit(1)
            }
        }
    }

    let query_trigram_set = Arc::new(build_query_trigram_set(&config.queries));
    let index_path = index_path_for_root(config.root_path.as_path());
    let index_manifest = Arc::new(Mutex::new(match load_index(&index_path) {
        Ok(manifest) => manifest,
        Err(error) => {
            if index_path.exists() {
                eprintln!("warning: [index] {}: {}", index_path.display(), error);
            }
            IndexManifest::new(config.root_path.clone())
        }
    }));

    let started_at = Instant::now();
    let outcome = run_search(
        &config,
        &compiled_queries,
        &query_trigram_set,
        &index_manifest,
        &color,
        cli.quiet,
        cli.no_update_index,
    );

    if !cli.no_update_index && outcome.index_entries_updated > 0 {
        let manifest_guard =
            index_manifest.lock().expect("index_manifest Mutex was poisoned by a panicked thread");
        let _ = save_index(&manifest_guard, &index_path);
    }

    let stdout = std::io::stdout();
    if let Some(tmpl) = &cli.rewrite {
        run_rewrite_mode(outcome.results, tmpl, cli.in_place, cli.yes, &color);
        return;
    }

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

fn run_rewrite_mode(
    results: Vec<MatchResult>,
    template: &str,
    in_place: bool,
    yes: bool,
    color: &ColorMode,
) {
    use std::io::{self, BufRead};

    let rewrite_results: Vec<dora::types::MatchResult> = results
        .into_iter()
        .map(|result| dora::types::MatchResult {
            file_path: result.file_path,
            start_line: result.start_line,
            start_col: result.start_col,
            end_line: result.end_line,
            end_col: result.end_col,
            start_byte: result.start_byte,
            end_byte: result.end_byte,
            capture_name: result.capture_name,
            matched_text: result.matched_text,
        })
        .collect();

    let tmpl = dora::rewrite::RewriteTemplate { raw: template.to_string() };
    let edits = dora::rewrite::compute_edits(&rewrite_results, &tmpl);
    if edits.is_empty() {
        eprintln!("No changes would be made.");
        return;
    }

    let all: HashMap<_, _> = dora::rewrite::apply_edits_to_files(&edits);

    for (path, result) in &all {
        match result {
            Ok(rewritten) => match std::fs::read_to_string(path) {
                Ok(original) => {
                    let diff = dora::rewrite::generate_diff(&original, rewritten, path);
                    if diff.is_empty() {
                        continue;
                    }

                    if let ColorMode::On = color {
                        for line in diff.lines() {
                            if line.starts_with('+') {
                                eprintln!("\x1b[32m{}\x1b[0m", line);
                            } else if line.starts_with('-') {
                                eprintln!("\x1b[31m{}\x1b[0m", line);
                            } else if line.starts_with('@') {
                                eprintln!("\x1b[36m{}\x1b[0m", line);
                            } else {
                                eprintln!("{}", line);
                            }
                        }
                    } else {
                        println!("{}", diff);
                    }
                }
                Err(error) => eprintln!("error: {}: {}", path.display(), error),
            },
            Err(message) => eprintln!("error: {}: {}", path.display(), message),
        }
    }

    let files_changed = all
        .values()
        .filter(|result| result.as_ref().map(|text| !text.is_empty()).unwrap_or(false))
        .count();
    let captures_rewritten = edits.len();
    eprintln!(
        "{} files would be modified, {} captures would be rewritten",
        files_changed, captures_rewritten
    );

    if in_place {
        eprintln!("Files with changes:");
        for (path, result) in &all {
            if result.is_ok() {
                eprintln!("  {}", path.display());
            }
        }

        let mut apply = yes;
        if !yes {
            eprint!("Apply these changes? [y/N] ");
            let stdin = io::stdin();
            let mut line = String::new();
            if stdin.lock().read_line(&mut line).is_ok() {
                let answer = line.trim();
                if answer == "y" || answer == "Y" {
                    apply = true;
                }
            }
        }

        if !apply {
            eprintln!("Aborted.");
            return;
        }

        let mut rewritten_count = 0usize;
        let mut error_count = 0usize;
        for (path, result) in &all {
            match result {
                Ok(content) => match dora::rewrite::write_atomically(path, content) {
                    Ok(()) => {
                        eprintln!("  rewritten: {}", path.display());
                        rewritten_count += 1;
                    }
                    Err(error) => {
                        eprintln!("  error: {}: {}", path.display(), error);
                        error_count += 1;
                    }
                },
                Err(message) => {
                    eprintln!("  error: {}: {}", path.display(), message);
                    error_count += 1;
                }
            }
        }

        eprintln!("{} files rewritten, {} errors", rewritten_count, error_count);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        format_file_error, handle_file_error, resolve_lang, resolve_lang_mode, Cli, FileError,
        LookupArgs, SearchOutcome,
    };
    use crate::types::{LangMode, Language};
    use clap_complete::Shell;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    fn lookup_args_with_symbol(symbol: &str) -> LookupArgs {
        LookupArgs {
            symbol: Some(symbol.to_string()),
            prefix: None,
            kind: None,
            path: std::env::temp_dir(),
            lang: "auto".to_string(),
            no_color: false,
        }
    }

    fn lookup_args_with_prefix(prefix: &str) -> LookupArgs {
        LookupArgs {
            symbol: None,
            prefix: Some(prefix.to_string()),
            kind: None,
            path: std::env::temp_dir(),
            lang: "auto".to_string(),
            no_color: false,
        }
    }

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
            query: vec!["(function_item)".to_string()],
            path: std::env::temp_dir(),
            lang: "rust".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
            no_update_index: false,
            generate_completions: None,
            tui: false,
            rewrite: None,
            in_place: false,
            yes: false,
        };
        assert!(cli.validate().is_ok());
    }

    #[test]
    fn test_cli_validate_nonexistent_path() {
        let cli = Cli {
            path: PathBuf::from("/tmp/dora_nonexistent_xyz_12345"),
            query: vec!["(function_item)".to_string()],
            lang: "rust".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
            no_update_index: false,
            generate_completions: None,
            tui: false,
            rewrite: None,
            in_place: false,
            yes: false,
        };
        let result = cli.validate();
        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        assert!(err_msg.contains("does not exist"));
        assert!(err_msg.contains("dora_nonexistent_xyz_12345"));
    }

    #[test]
    fn test_cli_validate_file_path_fails() {
        use tempfile::NamedTempFile;

        let temp_file = NamedTempFile::new().expect("failed to create temp file for test");
        let cli = Cli {
            path: temp_file.path().to_path_buf(),
            query: vec!["(function_item)".to_string()],
            lang: "rust".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
            no_update_index: false,
            generate_completions: None,
            tui: false,
            rewrite: None,
            in_place: false,
            yes: false,
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
            query: vec!["   ".to_string()],
            lang: "rust".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
            no_update_index: false,
            generate_completions: None,
            tui: false,
            rewrite: None,
            in_place: false,
            yes: false,
        };
        let result = cli.validate();
        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        assert_eq!(err_msg, "at least one query string must not be empty");
    }

    #[test]
    fn test_lookup_validate_exact_symbol_only() {
        let args = lookup_args_with_symbol("authenticate");
        assert!(args.validate().is_ok());
    }

    #[test]
    fn test_lookup_validate_prefix_only() {
        let args = lookup_args_with_prefix("auth");
        assert!(args.validate().is_ok());
    }

    #[test]
    fn test_lookup_validate_requires_exactly_one_selector() {
        let args = LookupArgs {
            symbol: None,
            prefix: None,
            kind: None,
            path: std::env::temp_dir(),
            lang: "auto".to_string(),
            no_color: false,
        };
        let err = args.validate().unwrap_err();
        assert!(err.contains("exactly one"));

        let args = LookupArgs {
            symbol: Some("one".to_string()),
            prefix: Some("two".to_string()),
            kind: None,
            path: std::env::temp_dir(),
            lang: "auto".to_string(),
            no_color: false,
        };
        let err = args.validate().unwrap_err();
        assert!(err.contains("exactly one"));
    }

    #[test]
    fn test_lookup_validate_rejects_bad_kind() {
        let args = LookupArgs {
            symbol: Some("foo".to_string()),
            prefix: None,
            kind: Some("not_a_kind".to_string()),
            path: std::env::temp_dir(),
            lang: "auto".to_string(),
            no_color: false,
        };
        let err = args.validate().unwrap_err();
        assert!(err.contains("unsupported kind"));
    }

    #[test]
    fn test_lookup_validate_rejects_bad_path() {
        let args = LookupArgs {
            symbol: Some("foo".to_string()),
            prefix: None,
            kind: None,
            path: PathBuf::from("/tmp/dora_lookup_missing_dir_12345"),
            lang: "auto".to_string(),
            no_color: false,
        };
        let err = args.validate().unwrap_err();
        assert!(err.contains("does not exist"));
    }

    #[test]
    fn test_resolve_lang_all_supported() {
        assert_eq!(resolve_lang("rust"), Language::Rust);
        assert_eq!(resolve_lang("python"), Language::Python);
        assert_eq!(resolve_lang("js"), Language::JavaScript);
        assert_eq!(resolve_lang("ts"), Language::TypeScript);
        assert_eq!(resolve_lang("go"), Language::Go);
        assert_eq!(resolve_lang("c"), Language::C);
        assert_eq!(resolve_lang("cpp"), Language::Cpp);
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
            start_byte: 0,
            end_byte: 0,
            capture_name: "cap".to_string(),
            matched_text: "txt".to_string(),
        });
        results.push(crate::types::MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 3,
            start_byte: 0,
            end_byte: 0,
            capture_name: "cap".to_string(),
            matched_text: "txt".to_string(),
        });
        results.push(crate::types::MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 3,
            start_byte: 0,
            end_byte: 0,
            capture_name: "cap".to_string(),
            matched_text: "txt".to_string(),
        });
        results.push(crate::types::MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 3,
            start_col: 0,
            end_line: 3,
            end_col: 3,
            start_byte: 0,
            end_byte: 0,
            capture_name: "cap".to_string(),
            matched_text: "txt".to_string(),
        });
        results.push(crate::types::MatchResult {
            file_path: PathBuf::from("src/b.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 3,
            start_byte: 0,
            end_byte: 0,
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
            start_byte: 0,
            end_byte: 0,
            capture_name: "x".to_string(),
            matched_text: "x".to_string(),
        });
        results.push(crate::types::MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 3,
            start_byte: 0,
            end_byte: 0,
            capture_name: "a".to_string(),
            matched_text: "a".to_string(),
        });
        results.push(crate::types::MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 3,
            start_byte: 0,
            end_byte: 0,
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
            query: vec!["(function_item)".to_string()],
            path: std::env::temp_dir(),
            lang: "rust".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
            no_update_index: false,
            generate_completions: None,
            tui: false,
            rewrite: None,
            in_place: false,
            yes: false,
        };
        assert!(!cli.stats);
    }

    #[test]
    fn test_quiet_flag_defaults_false() {
        let cli = Cli {
            query: vec!["(function_item)".to_string()],
            path: std::env::temp_dir(),
            lang: "rust".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
            no_update_index: false,
            generate_completions: None,
            tui: false,
            rewrite: None,
            in_place: false,
            yes: false,
        };
        assert!(!cli.quiet);
    }

    #[test]
    fn test_tui_flag_defaults_false() {
        let cli = Cli {
            query: vec!["(function_item)".to_string()],
            path: std::env::temp_dir(),
            lang: "rust".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
            no_update_index: false,
            generate_completions: None,
            tui: false,
            rewrite: None,
            in_place: false,
            yes: false,
        };
        assert!(!cli.tui);
    }

    #[test]
    fn test_tui_flag_is_independent_of_quiet() {
        let cli = Cli {
            query: vec!["(function_item)".to_string()],
            path: std::env::temp_dir(),
            lang: "rust".to_string(),
            no_color: false,
            quiet: true,
            stats: false,
            no_update_index: false,
            generate_completions: None,
            tui: true,
            rewrite: None,
            in_place: false,
            yes: false,
        };
        assert!(cli.quiet);
        assert!(cli.tui);
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
            sieve_rejected: 2,
            index_entries_updated: 3,
            files_with_matches: 3,
        };
        assert_eq!(outcome.files_walked, 10);
        assert_eq!(outcome.files_parsed, 9);
        assert_eq!(outcome.files_skipped, 1);
        assert_eq!(outcome.sieve_rejected, 2);
        assert_eq!(outcome.index_entries_updated, 3);
        assert_eq!(outcome.files_with_matches, 3);
        assert!(outcome.results.is_empty());
    }

    #[test]
    fn test_search_outcome_has_sieve_rejected_field() {
        let outcome = SearchOutcome {
            results: Vec::new(),
            files_walked: 0,
            files_parsed: 0,
            files_skipped: 0,
            sieve_rejected: 7,
            index_entries_updated: 0,
            files_with_matches: 0,
        };
        assert_eq!(outcome.sieve_rejected, 7);
    }

    #[test]
    fn test_search_outcome_has_index_entries_updated_field() {
        let outcome = SearchOutcome {
            results: Vec::new(),
            files_walked: 0,
            files_parsed: 0,
            files_skipped: 0,
            sieve_rejected: 0,
            index_entries_updated: 11,
            files_with_matches: 0,
        };
        assert_eq!(outcome.index_entries_updated, 11);
    }

    #[test]
    fn test_no_update_index_flag_defaults_false() {
        let cli = Cli {
            query: vec!["(function_item)".to_string()],
            path: std::env::temp_dir(),
            lang: "rust".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
            no_update_index: false,
            generate_completions: None,
            tui: false,
            rewrite: None,
            in_place: false,
            yes: false,
        };
        assert!(!cli.no_update_index);
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
                start_byte: 0,
                end_byte: 0,
                capture_name: "fn".to_string(),
                matched_text: "foo".to_string(),
            },
            crate::types::MatchResult {
                file_path: PathBuf::from("src/a.rs"),
                start_line: 5,
                start_col: 0,
                end_line: 5,
                end_col: 3,
                start_byte: 0,
                end_byte: 0,
                capture_name: "fn".to_string(),
                matched_text: "bar".to_string(),
            },
            crate::types::MatchResult {
                file_path: PathBuf::from("src/b.rs"),
                start_line: 1,
                start_col: 0,
                end_line: 1,
                end_col: 3,
                start_byte: 0,
                end_byte: 0,
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
            query: vec!["(function_item)".to_string()],
            path: std::env::temp_dir(),
            lang: "rust".to_string(),
            no_color: false,
            quiet: false,
            stats: true,
            no_update_index: false,
            generate_completions: None,
            tui: false,
            rewrite: None,
            in_place: false,
            yes: false,
        };
        assert!(cli.validate().is_ok());
    }

    #[test]
    fn test_validate_skips_checks_for_generate_completions() {
        let cli = Cli {
            query: vec!["".to_string()],
            path: PathBuf::from("/nonexistent/path/xyz"),
            lang: "cobol".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
            no_update_index: false,
            generate_completions: Some(Shell::Bash),
            tui: false,
            rewrite: None,
            in_place: false,
            yes: false,
        };
        assert!(cli.validate().is_ok());
    }

    #[test]
    fn test_validate_rejects_empty_query() {
        let cli = Cli {
            query: vec!["   ".to_string()],
            path: std::env::temp_dir(),
            lang: "rust".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
            no_update_index: false,
            generate_completions: None,
            tui: false,
            rewrite: None,
            in_place: false,
            yes: false,
        };
        let result = cli.validate();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "at least one query string must not be empty");
    }

    #[test]
    fn test_validate_rejects_nonexistent_path_with_hint() {
        let cli = Cli {
            query: vec!["(fn)".to_string()],
            path: PathBuf::from("/tmp/dora_nonexistent_xyz_99999"),
            lang: "rust".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
            no_update_index: false,
            generate_completions: None,
            tui: false,
            rewrite: None,
            in_place: false,
            yes: false,
        };
        let result = cli.validate();
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("does not exist"));
        assert!(msg.contains("dora_nonexistent_xyz_99999"));
        assert!(msg.contains("hint:"));
    }

    #[test]
    fn test_validate_rejects_file_path_with_hint() {
        use tempfile::NamedTempFile;
        let f = NamedTempFile::new().unwrap();
        let cli = Cli {
            query: vec!["(fn)".to_string()],
            path: f.path().to_path_buf(),
            lang: "rust".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
            no_update_index: false,
            generate_completions: None,
            tui: false,
            rewrite: None,
            in_place: false,
            yes: false,
        };
        let result = cli.validate();
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("not a directory"));
        assert!(msg.contains("hint:"));
    }

    #[test]
    fn test_validate_rejects_unsupported_lang_with_hint() {
        let cli = Cli {
            query: vec!["(fn)".to_string()],
            path: std::env::temp_dir(),
            lang: "cobol".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
            no_update_index: false,
            generate_completions: None,
            tui: false,
            rewrite: None,
            in_place: false,
            yes: false,
        };
        let result = cli.validate();
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("unsupported language"));
        assert!(msg.contains("cobol"));
        assert!(msg.contains("rust"));
        assert!(msg.contains("python"));
        assert!(msg.contains("auto"));
        assert!(msg.contains("example:"));
    }

    #[test]
    fn test_validate_lang_is_case_sensitive() {
        let cli = Cli {
            query: vec!["(fn)".to_string()],
            path: std::env::temp_dir(),
            lang: "Rust".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
            no_update_index: false,
            generate_completions: None,
            tui: false,
            rewrite: None,
            in_place: false,
            yes: false,
        };
        assert!(cli.validate().is_err());
    }

    #[test]
    fn test_validate_accepts_all_supported_languages() {
        for lang in &["rust", "python", "js", "ts", "go", "c", "cpp", "auto"] {
            let cli = Cli {
                query: vec!["(fn)".to_string()],
                path: std::env::temp_dir(),
                lang: lang.to_string(),
                no_color: false,
                quiet: false,
                stats: false,
                no_update_index: false,
                generate_completions: None,
                tui: false,
                rewrite: None,
                in_place: false,
                yes: false,
            };
            assert!(cli.validate().is_ok(), "validate() rejected valid lang: {}", lang);
        }
    }

    #[test]
    fn test_validate_checks_query_before_path() {
        let cli = Cli {
            query: vec!["".to_string()],
            path: PathBuf::from("/nonexistent/xyz"),
            lang: "rust".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
            no_update_index: false,
            generate_completions: None,
            tui: false,
            rewrite: None,
            in_place: false,
            yes: false,
        };
        let result = cli.validate();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "at least one query string must not be empty");
    }

    #[test]
    fn test_validate_checks_path_before_lang() {
        let cli = Cli {
            query: vec!["(fn)".to_string()],
            path: PathBuf::from("/nonexistent/xyz_abc"),
            lang: "cobol".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
            no_update_index: false,
            generate_completions: None,
            tui: false,
            rewrite: None,
            in_place: false,
            yes: false,
        };
        let result = cli.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[test]
    fn test_resolve_lang_all_variants() {
        assert_eq!(resolve_lang("rust"), Language::Rust);
        assert_eq!(resolve_lang("python"), Language::Python);
        assert_eq!(resolve_lang("js"), Language::JavaScript);
        assert_eq!(resolve_lang("ts"), Language::TypeScript);
        assert_eq!(resolve_lang("go"), Language::Go);
        assert_eq!(resolve_lang("c"), Language::C);
        assert_eq!(resolve_lang("cpp"), Language::Cpp);
    }

    #[test]
    fn test_resolve_lang_mode_auto() {
        assert_eq!(resolve_lang_mode("auto"), LangMode::Auto);
        assert_eq!(resolve_lang_mode("rust"), LangMode::Single(Language::Rust));
    }

    #[test]
    fn test_error_message_contains_newline_hint() {
        let cli = Cli {
            query: vec!["(fn)".to_string()],
            path: PathBuf::from("/nonexistent/xyz_hint_test"),
            lang: "rust".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
            no_update_index: false,
            generate_completions: None,
            tui: false,
            rewrite: None,
            in_place: false,
            yes: false,
        };
        let msg = cli.validate().unwrap_err();
        assert!(msg.contains('\n'));
        assert!(msg.contains("  hint:"));
    }

    #[test]
    fn test_unsupported_lang_error_names_passed_value() {
        let cli = Cli {
            query: vec!["(fn)".to_string()],
            path: std::env::temp_dir(),
            lang: "fortran77".to_string(),
            no_color: false,
            quiet: false,
            stats: false,
            no_update_index: false,
            generate_completions: None,
            tui: false,
            rewrite: None,
            in_place: false,
            yes: false,
        };
        let msg = cli.validate().unwrap_err();
        assert!(msg.contains("fortran77"));
    }
}
