use crate::index::{index_path_for_root, load_index, IndexManifest};
use crate::memory::{FileRow, MemoryDb, SymbolRow};
use crate::output::ColorMode;
use crate::parser::get_all_languages;
use crate::query;
use crate::sieve::build_query_trigram_set;
use crate::types::{AppError, LangMode, Language, MatchResult, Result, SearchConfig};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct McpConfig {
    pub root_path: PathBuf,
    pub db_path: PathBuf,
    pub lang_mode: LangMode,
}

#[derive(Debug, Deserialize)]
struct RpcRequest {
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct RpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct SearchAstParams {
    query: String,
}

#[derive(Debug, Deserialize)]
struct LookupSymbolParams {
    name: String,
}

#[derive(Debug, Serialize)]
struct SearchAstItem {
    file_path: String,
    capture_name: String,
    matched_text: String,
    start_line: usize,
    start_col: usize,
    end_line: usize,
    end_col: usize,
}

#[derive(Debug, Serialize)]
struct LookupSymbolItem {
    file_path: String,
    kind: String,
    name: String,
    start_line: usize,
    start_col: usize,
    end_line: usize,
    end_col: usize,
    signature: Option<String>,
}

pub fn run(config: McpConfig) -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut input = stdin.lock();
    let mut output = stdout.lock();
    let mut line = String::new();

    loop {
        line.clear();
        let read = input.read_line(&mut line).map_err(AppError::IoError)?;
        if read == 0 {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }

        let response = handle_line(&config, line.trim_end());
        output.write_all(response.as_bytes()).map_err(AppError::IoError)?;
        output.write_all(b"\n").map_err(AppError::IoError)?;
        output.flush().map_err(AppError::IoError)?;
    }

    Ok(())
}

fn handle_line(config: &McpConfig, line: &str) -> String {
    match serde_json::from_str::<RpcRequest>(line) {
        Ok(request) => response_to_string(handle_request(config, request)),
        Err(error) => response_to_string(RpcResponse {
            jsonrpc: "2.0",
            id: Value::Null,
            result: None,
            error: Some(RpcError {
                code: -32700,
                message: "parse error".to_string(),
                data: Some(Value::String(error.to_string())),
            }),
        }),
    }
}

fn handle_request(config: &McpConfig, request: RpcRequest) -> RpcResponse {
    let id = request.id.unwrap_or(Value::Null);

    if request.jsonrpc != "2.0" {
        return rpc_error(
            id,
            -32600,
            "invalid request",
            Some(Value::String("jsonrpc must be \"2.0\"".to_string())),
        );
    }

    match request.method.as_str() {
        "search_ast" => match parse_params::<SearchAstParams>(request.params) {
            Ok(params) if params.query.trim().is_empty() => rpc_error(
                id,
                -32602,
                "invalid params",
                Some(Value::String("query must not be empty".to_string())),
            ),
            Ok(params) => match search_ast(config, &params.query) {
                Ok(results) => rpc_ok(id, json!(results)),
                Err(error) => rpc_app_error(id, error),
            },
            Err(message) => rpc_error(id, -32602, "invalid params", Some(Value::String(message))),
        },
        "lookup_symbol" => match parse_params::<LookupSymbolParams>(request.params) {
            Ok(params) if params.name.trim().is_empty() => rpc_error(
                id,
                -32602,
                "invalid params",
                Some(Value::String("name must not be empty".to_string())),
            ),
            Ok(params) => match lookup_symbol(config, &params.name) {
                Ok(results) => rpc_ok(id, json!(results)),
                Err(error) => rpc_app_error(id, error),
            },
            Err(message) => rpc_error(id, -32602, "invalid params", Some(Value::String(message))),
        },
        _ => rpc_error(id, -32601, "method not found", Some(Value::String(request.method))),
    }
}

fn parse_params<T>(params: Option<Value>) -> std::result::Result<T, String>
where
    T: for<'de> Deserialize<'de>,
{
    let value = params.ok_or_else(|| "missing params".to_string())?;
    serde_json::from_value(value).map_err(|error| error.to_string())
}

fn rpc_ok(id: Value, result: Value) -> RpcResponse {
    RpcResponse { jsonrpc: "2.0", id, result: Some(result), error: None }
}

fn rpc_error(id: Value, code: i32, message: &str, data: Option<Value>) -> RpcResponse {
    RpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(RpcError { code, message: message.to_string(), data }),
    }
}

fn rpc_app_error(id: Value, error: AppError) -> RpcResponse {
    rpc_error(id, -32603, &error.to_string(), Some(Value::String(error.to_string())))
}

fn response_to_string(response: RpcResponse) -> String {
    serde_json::to_string(&response).unwrap_or_else(|error| {
        json!({
            "jsonrpc": "2.0",
            "id": Value::Null,
            "error": {
                "code": -32603,
                "message": "internal error",
                "data": error.to_string(),
            }
        })
        .to_string()
    })
}

fn search_ast(config: &McpConfig, query: &str) -> Result<Vec<SearchAstItem>> {
    let search_config = SearchConfig {
        queries: vec![query.to_string()],
        root_path: config.root_path.clone(),
        lang_mode: config.lang_mode.clone(),
    };
    let compiled_queries = compile_queries(&search_config)?;
    let query_trigram_set = Arc::new(build_query_trigram_set(&search_config.queries));
    let index_path = index_path_for_root(search_config.root_path.as_path());
    let index_manifest = Arc::new(Mutex::new(match load_index(&index_path) {
        Ok(manifest) => manifest,
        Err(_) => IndexManifest::new(search_config.root_path.clone()),
    }));
    let outcome = super::run_search(
        &search_config,
        &compiled_queries,
        &query_trigram_set,
        &index_manifest,
        &ColorMode::Off,
        true,
        true,
    );

    Ok(outcome.results.into_iter().map(Into::into).collect())
}

fn lookup_symbol(config: &McpConfig, name: &str) -> Result<Vec<LookupSymbolItem>> {
    if !config.db_path.exists() {
        return Err(AppError::DbError(format!(
            "no structural index found at {}\n  hint: run doora --persist {} first",
            config.db_path.display(),
            config.root_path.display()
        )));
    }

    let db = MemoryDb::open(&config.db_path)?;
    let symbols = {
        let exact = db.find_symbols_by_name(name)?;
        if exact.is_empty() {
            db.find_symbols_by_name_contains(name)?
        } else {
            exact
        }
    };

    let mut rows = Vec::new();
    for symbol in symbols {
        let file = db.get_file_by_id(symbol.file_id)?.ok_or_else(|| {
            AppError::DbError(format!("missing file row for file_id {}", symbol.file_id))
        })?;
        rows.push(LookupSymbolItem::from((symbol, file)));
    }

    rows.sort_by(|left, right| {
        left.file_path
            .cmp(&right.file_path)
            .then_with(|| left.start_line.cmp(&right.start_line))
            .then_with(|| left.start_col.cmp(&right.start_col))
            .then_with(|| left.name.cmp(&right.name))
    });

    Ok(rows)
}

fn compile_queries(
    config: &SearchConfig,
) -> Result<Arc<HashMap<Language, Arc<query::MultiCompiledQuery>>>> {
    let query = config
        .queries
        .first()
        .ok_or_else(|| AppError::QueryCompileError("query must not be empty".to_string()))?;

    match &config.lang_mode {
        LangMode::Single(lang) => {
            let ts_lang = super::lang_to_ts_language(lang);
            let compiled = query::compile_query(&ts_lang, query)?;
            Ok(Arc::new(HashMap::from([(
                lang.clone(),
                Arc::new(query::MultiCompiledQuery { queries: vec![compiled], language: ts_lang }),
            )])))
        }
        LangMode::Auto => {
            let mut map = HashMap::new();
            for (lang, ts_lang) in get_all_languages() {
                if let Ok(compiled) = query::compile_query(&ts_lang, query) {
                    map.insert(
                        lang,
                        Arc::new(query::MultiCompiledQuery {
                            queries: vec![compiled],
                            language: ts_lang,
                        }),
                    );
                }
            }
            if map.is_empty() {
                return Err(AppError::QueryCompileError(format!(
                    "query did not compile against any supported language\n  query: {}\n  hint: check the S-expression syntax and node type names",
                    query
                )));
            }
            Ok(Arc::new(map))
        }
    }
}

impl From<MatchResult> for SearchAstItem {
    fn from(result: MatchResult) -> Self {
        Self {
            file_path: result.file_path.display().to_string(),
            capture_name: result.capture_name,
            matched_text: result.matched_text,
            start_line: result.start_line,
            start_col: result.start_col,
            end_line: result.end_line,
            end_col: result.end_col,
        }
    }
}

impl From<(SymbolRow, FileRow)> for LookupSymbolItem {
    fn from(value: (SymbolRow, FileRow)) -> Self {
        let (symbol, file) = value;
        Self {
            file_path: file.path,
            kind: symbol.kind.to_string(),
            name: symbol.name,
            start_line: symbol.start_line,
            start_col: symbol.start_col,
            end_line: symbol.end_line,
            end_col: symbol.end_col,
            signature: symbol.signature,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{NewFileRow, NewSymbolRow, SymbolKind};
    use crate::types::Language;
    use std::fs;
    use tempfile::TempDir;

    fn make_config(root_path: PathBuf, db_path: PathBuf, lang_mode: LangMode) -> McpConfig {
        McpConfig { root_path, db_path, lang_mode }
    }

    #[test]
    fn test_unknown_method_returns_error() {
        let config =
            make_config(PathBuf::from("."), PathBuf::from("/tmp/missing.db"), LangMode::Auto);
        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "nope".to_string(),
            params: None,
        };
        let response = handle_request(&config, request);
        assert_eq!(response.error.unwrap().code, -32601);
    }

    #[test]
    fn test_missing_params_returns_invalid_params() {
        let config =
            make_config(PathBuf::from("."), PathBuf::from("/tmp/missing.db"), LangMode::Auto);
        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "search_ast".to_string(),
            params: None,
        };
        let response = handle_request(&config, request);
        assert_eq!(response.error.unwrap().code, -32602);
    }

    #[test]
    fn test_lookup_symbol_exact_then_contains() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let db = MemoryDb::open(&db_path).unwrap();
        let file_id = db
            .upsert_file(&NewFileRow {
                path: dir.path().join("a.rs").display().to_string(),
                mtime: 1,
                language: "rust".to_string(),
            })
            .unwrap();
        db.insert_symbol(&NewSymbolRow {
            file_id,
            kind: SymbolKind::Function,
            name: "authenticate".to_string(),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 12,
            signature: Some("fn authenticate()".to_string()),
        })
        .unwrap();
        let config = make_config(dir.path().to_path_buf(), db_path, LangMode::Auto);
        let exact = lookup_symbol(&config, "authenticate").unwrap();
        assert_eq!(exact.len(), 1);
        let fallback = lookup_symbol(&config, "auth").unwrap();
        assert_eq!(fallback.len(), 1);
        assert_eq!(fallback[0].name, "authenticate");
    }

    #[test]
    fn test_search_ast_returns_match_array() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("simple.rs");
        fs::write(&file, "fn hello() {}\n").unwrap();
        let config = make_config(
            dir.path().to_path_buf(),
            dir.path().join("index.db"),
            LangMode::Single(Language::Rust),
        );
        let results = search_ast(&config, "(function_item name: (identifier) @fn_name)").unwrap();
        assert!(!results.is_empty());
        assert!(results.iter().any(|row| row.matched_text == "hello"));
    }
}
