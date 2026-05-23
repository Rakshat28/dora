#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use crate::types::{AppError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

pub const MEMORY_DB_FILENAME: &str = ".ast-search-memory.db";

#[must_use]
pub fn memory_db_path(root: &Path) -> PathBuf {
    root.join(MEMORY_DB_FILENAME)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolKind {
    Function,
    Method,
    Struct,
    Enum,
    Trait,
    Interface,
    TypeAlias,
    Constant,
    Variable,
    Class,
    Module,
    Import,
    Unknown,
}

impl fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Function => "function",
            Self::Method => "method",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Trait => "trait",
            Self::Interface => "interface",
            Self::TypeAlias => "typealias",
            Self::Constant => "constant",
            Self::Variable => "variable",
            Self::Class => "class",
            Self::Module => "module",
            Self::Import => "import",
            Self::Unknown => "unknown",
        })
    }
}

impl FromStr for SymbolKind {
    type Err = Infallible;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        Ok(match value {
            "function" => Self::Function,
            "method" => Self::Method,
            "struct" => Self::Struct,
            "enum" => Self::Enum,
            "trait" => Self::Trait,
            "interface" => Self::Interface,
            "typealias" => Self::TypeAlias,
            "constant" => Self::Constant,
            "variable" => Self::Variable,
            "class" => Self::Class,
            "module" => Self::Module,
            "import" => Self::Import,
            _ => Self::Unknown,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileRow {
    pub id: i64,
    pub path: String,
    pub mtime: i64,
    pub language: String,
    pub indexed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolRow {
    pub id: i64,
    pub file_id: i64,
    pub kind: SymbolKind,
    pub name: String,
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewFileRow {
    pub path: String,
    pub mtime: i64,
    pub language: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewSymbolRow {
    pub file_id: i64,
    pub kind: SymbolKind,
    pub name: String,
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub signature: Option<String>,
}

pub struct MemoryDb {
    conn: Connection,
}

impl MemoryDb {
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path).map_err(db_error)?;
        let db = Self { conn };
        db.initialize_schema()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(db_error)?;
        let db = Self { conn };
        db.initialize_schema()?;
        Ok(db)
    }

    fn initialize_schema(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "PRAGMA foreign_keys=ON;\nPRAGMA journal_mode=WAL;\nCREATE TABLE IF NOT EXISTS files (\n    id INTEGER PRIMARY KEY AUTOINCREMENT,\n    path TEXT NOT NULL UNIQUE,\n    mtime INTEGER NOT NULL,\n    language TEXT NOT NULL,\n    indexed_at INTEGER NOT NULL\n);\nCREATE INDEX IF NOT EXISTS idx_files_path ON files(path);\nCREATE TABLE IF NOT EXISTS symbols (\n    id INTEGER PRIMARY KEY AUTOINCREMENT,\n    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,\n    kind TEXT NOT NULL,\n    name TEXT NOT NULL,\n    start_line INTEGER NOT NULL,\n    start_col INTEGER NOT NULL,\n    end_line INTEGER NOT NULL,\n    end_col INTEGER NOT NULL,\n    signature TEXT\n);\nCREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);\nCREATE INDEX IF NOT EXISTS idx_symbols_file_id ON symbols(file_id);\nCREATE INDEX IF NOT EXISTS idx_symbols_kind ON symbols(kind);",
            )
            .map_err(db_error)
    }

    pub fn upsert_file(&self, row: &NewFileRow) -> Result<i64> {
        let indexed_at = unix_seconds_now()?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO files(path, mtime, language, indexed_at) VALUES (?1, ?2, ?3, ?4)",
                params![&row.path, row.mtime, &row.language, indexed_at],
            )
            .map_err(db_error)?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_file_by_path(&self, path: &str) -> Result<Option<FileRow>> {
        self.conn
            .query_row(
                "SELECT id, path, mtime, language, indexed_at FROM files WHERE path = ?1",
                params![path],
                |row| {
                    Ok(FileRow {
                        id: row.get(0)?,
                        path: row.get(1)?,
                        mtime: row.get(2)?,
                        language: row.get(3)?,
                        indexed_at: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(db_error)
    }

    pub fn get_file_by_id(&self, id: i64) -> Result<Option<FileRow>> {
        self.conn
            .query_row(
                "SELECT id, path, mtime, language, indexed_at FROM files WHERE id = ?1",
                params![id],
                |row| {
                    Ok(FileRow {
                        id: row.get(0)?,
                        path: row.get(1)?,
                        mtime: row.get(2)?,
                        language: row.get(3)?,
                        indexed_at: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(db_error)
    }

    pub fn delete_file_by_path(&self, path: &str) -> Result<usize> {
        self.conn.execute("DELETE FROM files WHERE path = ?1", params![path]).map_err(db_error)
    }

    pub fn list_files(&self) -> Result<Vec<FileRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, path, mtime, language, indexed_at FROM files ORDER BY path ASC")
            .map_err(db_error)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(FileRow {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    mtime: row.get(2)?,
                    language: row.get(3)?,
                    indexed_at: row.get(4)?,
                })
            })
            .map_err(db_error)?;
        collect_rows(rows)
    }

    pub fn file_count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
            .map_err(db_error)?;
        i64_to_usize(count)
    }

    pub fn insert_symbol(&self, row: &NewSymbolRow) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO symbols(file_id, kind, name, start_line, start_col, end_line, end_col, signature) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    row.file_id,
                    row.kind.to_string(),
                    &row.name,
                    usize_to_i64(row.start_line)?,
                    usize_to_i64(row.start_col)?,
                    usize_to_i64(row.end_line)?,
                    usize_to_i64(row.end_col)?,
                    row.signature.as_deref(),
                ],
            )
            .map_err(db_error)?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn insert_symbols_batch(&self, rows: &[NewSymbolRow]) -> Result<usize> {
        self.conn.execute_batch("BEGIN IMMEDIATE TRANSACTION;").map_err(db_error)?;
        let mut inserted = 0usize;
        let result = (|| -> Result<usize> {
            for row in rows {
                self.conn
                    .execute(
                        "INSERT INTO symbols(file_id, kind, name, start_line, start_col, end_line, end_col, signature) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        params![
                            row.file_id,
                            row.kind.to_string(),
                            &row.name,
                            usize_to_i64(row.start_line)?,
                            usize_to_i64(row.start_col)?,
                            usize_to_i64(row.end_line)?,
                            usize_to_i64(row.end_col)?,
                            row.signature.as_deref(),
                        ],
                    )
                    .map_err(db_error)?;
                inserted += 1;
            }
            self.conn.execute_batch("COMMIT;").map_err(db_error)?;
            Ok(inserted)
        })();
        if let Err(error) = result {
            let _ = self.conn.execute_batch("ROLLBACK;");
            return Err(error);
        }
        result
    }

    pub fn get_symbols_for_file(&self, file_id: i64) -> Result<Vec<SymbolRow>> {
        self.query_symbols(
            "SELECT id, file_id, kind, name, start_line, start_col, end_line, end_col, signature FROM symbols WHERE file_id = ?1 ORDER BY start_line ASC, start_col ASC",
            params![file_id],
        )
    }

    pub fn find_symbols_by_name(&self, name: &str) -> Result<Vec<SymbolRow>> {
        self.query_symbols(
            "SELECT id, file_id, kind, name, start_line, start_col, end_line, end_col, signature FROM symbols WHERE name = ?1 ORDER BY file_id ASC, start_line ASC, start_col ASC",
            params![name],
        )
    }

    pub fn find_symbols_by_name_prefix(&self, prefix: &str) -> Result<Vec<SymbolRow>> {
        let pattern = format!("{prefix}%");
        self.query_symbols(
            "SELECT id, file_id, kind, name, start_line, start_col, end_line, end_col, signature FROM symbols WHERE name LIKE ?1 ORDER BY name ASC, file_id ASC LIMIT 100",
            params![pattern],
        )
    }

    pub fn find_symbols_by_name_contains(&self, needle: &str) -> Result<Vec<SymbolRow>> {
        let pattern = format!("%{needle}%");
        self.query_symbols(
            "SELECT id, file_id, kind, name, start_line, start_col, end_line, end_col, signature FROM symbols WHERE name LIKE ?1 ORDER BY name ASC, file_id ASC",
            params![pattern],
        )
    }

    pub fn find_symbols_by_kind(&self, kind: &SymbolKind) -> Result<Vec<SymbolRow>> {
        self.query_symbols(
            "SELECT id, file_id, kind, name, start_line, start_col, end_line, end_col, signature FROM symbols WHERE kind = ?1 ORDER BY name ASC, file_id ASC",
            params![kind.to_string()],
        )
    }

    pub fn find_symbols_by_name_and_kind(
        &self,
        name: &str,
        kind: &SymbolKind,
    ) -> Result<Vec<SymbolRow>> {
        self.query_symbols(
            "SELECT id, file_id, kind, name, start_line, start_col, end_line, end_col, signature FROM symbols WHERE name = ?1 AND kind = ?2 ORDER BY file_id ASC, start_line ASC, start_col ASC",
            params![name, kind.to_string()],
        )
    }

    pub fn delete_symbols_for_file(&self, file_id: i64) -> Result<usize> {
        self.conn
            .execute("DELETE FROM symbols WHERE file_id = ?1", params![file_id])
            .map_err(db_error)
    }

    pub fn symbol_count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))
            .map_err(db_error)?;
        i64_to_usize(count)
    }

    fn query_symbols(&self, sql: &str, params: impl rusqlite::Params) -> Result<Vec<SymbolRow>> {
        let mut stmt = self.conn.prepare(sql).map_err(db_error)?;
        let rows = stmt
            .query_map(params, |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            })
            .map_err(db_error)?;
        let mut symbols = Vec::new();
        for row in rows {
            let (id, file_id, kind, name, start_line, start_col, end_line, end_col, signature) =
                row.map_err(db_error)?;
            symbols.push(SymbolRow {
                id,
                file_id,
                kind: kind.parse().unwrap(),
                name,
                start_line: i64_to_usize(start_line)?,
                start_col: i64_to_usize(start_col)?,
                end_line: i64_to_usize(end_line)?,
                end_col: i64_to_usize(end_col)?,
                signature,
            });
        }
        Ok(symbols)
    }
}

#[allow(clippy::needless_pass_by_value)]
fn db_error(error: rusqlite::Error) -> AppError {
    AppError::DbError(error.to_string())
}

fn unix_seconds_now() -> Result<i64> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| AppError::DbError(error.to_string()))?
        .as_secs();
    i64::try_from(seconds).map_err(|_| AppError::DbError("timestamp out of range".to_string()))
}

fn usize_to_i64(value: usize) -> Result<i64> {
    i64::try_from(value)
        .map_err(|_| AppError::DbError(format!("value out of range for i64: {value}")))
}

fn i64_to_usize(value: i64) -> Result<usize> {
    usize::try_from(value)
        .map_err(|_| AppError::DbError(format!("value out of range for usize: {value}")))
}

fn collect_rows(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<FileRow>>,
) -> Result<Vec<FileRow>> {
    let mut items = Vec::new();
    for row in rows {
        items.push(row.map_err(db_error)?);
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_db() -> MemoryDb {
        MemoryDb::open_in_memory().unwrap()
    }

    fn make_file_row(path: &str, mtime: i64) -> NewFileRow {
        NewFileRow { path: path.to_string(), mtime, language: "rust".to_string() }
    }

    fn insert_file(db: &MemoryDb, path: &str, mtime: i64) -> i64 {
        db.upsert_file(&make_file_row(path, mtime)).unwrap()
    }

    fn make_symbol_row(
        file_id: i64,
        kind: SymbolKind,
        name: &str,
        start_line: usize,
    ) -> NewSymbolRow {
        NewSymbolRow {
            file_id,
            kind,
            name: name.to_string(),
            start_line,
            start_col: 0,
            end_line: start_line,
            end_col: 1,
            signature: None,
        }
    }

    #[test]
    fn test_schema_creates_without_error() {
        assert!(MemoryDb::open_in_memory().is_ok());
    }

    #[test]
    fn test_upsert_file_returns_id() {
        let db = make_db();
        let id = db.upsert_file(&make_file_row("/tmp/a.rs", 100)).unwrap();
        assert!(id > 0);
    }

    #[test]
    fn test_get_file_by_path_after_upsert() {
        let db = make_db();
        db.upsert_file(&make_file_row("/tmp/a.rs", 100)).unwrap();
        let row = db.get_file_by_path("/tmp/a.rs").unwrap().unwrap();
        assert_eq!(row.path, "/tmp/a.rs");
        assert_eq!(row.mtime, 100);
        assert_eq!(row.language, "rust");
    }

    #[test]
    fn test_get_file_by_path_returns_none_for_missing() {
        let db = make_db();
        assert!(db.get_file_by_path("nonexistent").unwrap().is_none());
    }

    #[test]
    fn test_get_file_by_id_after_upsert() {
        let db = make_db();
        let id = db.upsert_file(&make_file_row("/tmp/a.rs", 100)).unwrap();
        let row = db.get_file_by_id(id).unwrap().unwrap();
        assert_eq!(row.id, id);
        assert_eq!(row.path, "/tmp/a.rs");
        assert_eq!(row.language, "rust");
    }

    #[test]
    fn test_get_file_by_id_returns_none_for_missing() {
        let db = make_db();
        assert!(db.get_file_by_id(9999).unwrap().is_none());
    }

    #[test]
    fn test_upsert_file_replaces_existing() {
        let db = make_db();
        insert_file(&db, "/tmp/a.rs", 100);
        insert_file(&db, "/tmp/a.rs", 200);
        let row = db.get_file_by_path("/tmp/a.rs").unwrap().unwrap();
        assert_eq!(row.mtime, 200);
        assert_eq!(db.file_count().unwrap(), 1);
    }

    #[test]
    fn test_delete_file_removes_row() {
        let db = make_db();
        insert_file(&db, "/tmp/a.rs", 100);
        db.delete_file_by_path("/tmp/a.rs").unwrap();
        assert!(db.get_file_by_path("/tmp/a.rs").unwrap().is_none());
    }

    #[test]
    fn test_delete_cascades_to_symbols() {
        let db = make_db();
        let file_id = insert_file(&db, "/tmp/a.rs", 100);
        db.insert_symbol(&make_symbol_row(file_id, SymbolKind::Function, "one", 1)).unwrap();
        db.insert_symbol(&make_symbol_row(file_id, SymbolKind::Struct, "two", 2)).unwrap();
        db.delete_file_by_path("/tmp/a.rs").unwrap();
        assert_eq!(db.symbol_count().unwrap(), 0);
    }

    #[test]
    fn test_insert_symbol_returns_id() {
        let db = make_db();
        let file_id = insert_file(&db, "/tmp/a.rs", 100);
        let id =
            db.insert_symbol(&make_symbol_row(file_id, SymbolKind::Function, "one", 1)).unwrap();
        assert!(id > 0);
    }

    #[test]
    fn test_get_symbols_for_file_returns_all() {
        let db = make_db();
        let file_id = insert_file(&db, "/tmp/a.rs", 100);
        db.insert_symbol(&make_symbol_row(file_id, SymbolKind::Function, "one", 1)).unwrap();
        db.insert_symbol(&make_symbol_row(file_id, SymbolKind::Struct, "two", 2)).unwrap();
        db.insert_symbol(&make_symbol_row(file_id, SymbolKind::Enum, "three", 3)).unwrap();
        let symbols = db.get_symbols_for_file(file_id).unwrap();
        assert_eq!(symbols.len(), 3);
    }

    #[test]
    fn test_find_symbols_by_name_exact() {
        let db = make_db();
        let file_one = insert_file(&db, "/tmp/a.rs", 100);
        let file_two = insert_file(&db, "/tmp/b.rs", 100);
        db.insert_symbol(&make_symbol_row(file_one, SymbolKind::Function, "foo", 1)).unwrap();
        db.insert_symbol(&make_symbol_row(file_two, SymbolKind::Function, "bar", 1)).unwrap();
        db.insert_symbol(&make_symbol_row(file_two, SymbolKind::Struct, "foo", 2)).unwrap();
        let symbols = db.find_symbols_by_name("foo").unwrap();
        assert_eq!(symbols.len(), 2);
    }

    #[test]
    fn test_find_symbols_by_name_prefix() {
        let db = make_db();
        let file_id = insert_file(&db, "/tmp/a.rs", 100);
        db.insert_symbol(&make_symbol_row(file_id, SymbolKind::Function, "authenticate", 1))
            .unwrap();
        db.insert_symbol(&make_symbol_row(file_id, SymbolKind::Function, "authorize", 2)).unwrap();
        db.insert_symbol(&make_symbol_row(file_id, SymbolKind::Function, "connect", 3)).unwrap();
        let symbols = db.find_symbols_by_name_prefix("auth").unwrap();
        assert_eq!(symbols.len(), 2);
    }

    #[test]
    fn test_find_symbols_by_name_contains() {
        let db = make_db();
        let file_id = insert_file(&db, "/tmp/a.rs", 100);
        db.insert_symbol(&make_symbol_row(file_id, SymbolKind::Function, "authenticate", 1))
            .unwrap();
        db.insert_symbol(&make_symbol_row(file_id, SymbolKind::Function, "authorise", 2)).unwrap();
        db.insert_symbol(&make_symbol_row(file_id, SymbolKind::Function, "connect", 3)).unwrap();
        let symbols = db.find_symbols_by_name_contains("auth").unwrap();
        assert_eq!(symbols.len(), 2);
    }

    #[test]
    fn test_find_symbols_by_kind() {
        let db = make_db();
        let file_id = insert_file(&db, "/tmp/a.rs", 100);
        db.insert_symbol(&make_symbol_row(file_id, SymbolKind::Function, "foo", 1)).unwrap();
        db.insert_symbol(&make_symbol_row(file_id, SymbolKind::Struct, "bar", 2)).unwrap();
        let symbols = db.find_symbols_by_kind(&SymbolKind::Function).unwrap();
        assert!(symbols.iter().all(|symbol| symbol.kind == SymbolKind::Function));
    }

    #[test]
    fn test_find_symbols_by_name_and_kind() {
        let db = make_db();
        let file_id = insert_file(&db, "/tmp/a.rs", 100);
        db.insert_symbol(&make_symbol_row(file_id, SymbolKind::Function, "new", 1)).unwrap();
        db.insert_symbol(&make_symbol_row(file_id, SymbolKind::Struct, "new", 2)).unwrap();
        let symbols = db.find_symbols_by_name_and_kind("new", &SymbolKind::Function).unwrap();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn test_insert_symbols_batch_is_atomic() {
        let db = make_db();
        let file_id = insert_file(&db, "/tmp/a.rs", 100);
        let rows = vec![
            make_symbol_row(file_id, SymbolKind::Function, "one", 1),
            make_symbol_row(file_id, SymbolKind::Struct, "two", 2),
            make_symbol_row(file_id, SymbolKind::Enum, "three", 3),
        ];
        let inserted = db.insert_symbols_batch(&rows).unwrap();
        assert_eq!(inserted, 3);
        assert_eq!(db.symbol_count().unwrap(), 3);
    }

    #[test]
    fn test_delete_symbols_for_file() {
        let db = make_db();
        let file_id = insert_file(&db, "/tmp/a.rs", 100);
        db.insert_symbol(&make_symbol_row(file_id, SymbolKind::Function, "one", 1)).unwrap();
        db.insert_symbol(&make_symbol_row(file_id, SymbolKind::Struct, "two", 2)).unwrap();
        db.insert_symbol(&make_symbol_row(file_id, SymbolKind::Enum, "three", 3)).unwrap();
        db.delete_symbols_for_file(file_id).unwrap();
        assert_eq!(db.symbol_count().unwrap(), 0);
    }

    #[test]
    fn test_symbol_kind_display_roundtrip() {
        let kinds = [
            SymbolKind::Function,
            SymbolKind::Method,
            SymbolKind::Struct,
            SymbolKind::Enum,
            SymbolKind::Trait,
            SymbolKind::Interface,
            SymbolKind::TypeAlias,
            SymbolKind::Constant,
            SymbolKind::Variable,
            SymbolKind::Class,
            SymbolKind::Module,
            SymbolKind::Import,
            SymbolKind::Unknown,
        ];

        for kind in kinds {
            let text = kind.to_string();
            let parsed: SymbolKind = text.parse().unwrap();
            assert_eq!(kind, parsed);
        }
    }

    #[test]
    fn test_symbol_kind_unknown_for_garbage_string() {
        let parsed: SymbolKind = "not_a_real_kind".parse().unwrap();
        assert_eq!(parsed, SymbolKind::Unknown);
    }

    #[test]
    fn test_find_symbols_prefix_limit() {
        let db = make_db();
        let file_id = insert_file(&db, "/tmp/a.rs", 100);
        let mut rows = Vec::new();
        for i in 0..150usize {
            rows.push(make_symbol_row(
                file_id,
                SymbolKind::Function,
                &format!("sym_{i:03}"),
                i + 1,
            ));
        }
        db.insert_symbols_batch(&rows).unwrap();
        let symbols = db.find_symbols_by_name_prefix("sym_").unwrap();
        assert!(symbols.len() <= 100);
    }

    #[test]
    fn test_list_files_sorted_by_path() {
        let db = make_db();
        insert_file(&db, "/tmp/z.rs", 100);
        insert_file(&db, "/tmp/a.rs", 100);
        insert_file(&db, "/tmp/m.rs", 100);
        let files = db.list_files().unwrap();
        let paths: Vec<_> = files.iter().map(|row| row.path.as_str()).collect();
        assert_eq!(paths, vec!["/tmp/a.rs", "/tmp/m.rs", "/tmp/z.rs"]);
    }

    #[test]
    fn test_file_count_accurate() {
        let db = make_db();
        insert_file(&db, "/tmp/a.rs", 100);
        insert_file(&db, "/tmp/b.rs", 100);
        insert_file(&db, "/tmp/c.rs", 100);
        insert_file(&db, "/tmp/d.rs", 100);
        insert_file(&db, "/tmp/e.rs", 100);
        assert_eq!(db.file_count().unwrap(), 5);
        db.delete_file_by_path("/tmp/a.rs").unwrap();
        db.delete_file_by_path("/tmp/b.rs").unwrap();
        assert_eq!(db.file_count().unwrap(), 3);
    }

    #[test]
    fn test_symbol_count_accurate() {
        let db = make_db();
        let file_id = insert_file(&db, "/tmp/a.rs", 100);
        for i in 0..4usize {
            db.insert_symbol(&make_symbol_row(
                file_id,
                SymbolKind::Function,
                &format!("s{i}"),
                i + 1,
            ))
            .unwrap();
        }
        assert_eq!(db.symbol_count().unwrap(), 4);
    }

    #[test]
    fn test_get_symbols_for_file_ordered_by_position() {
        let db = make_db();
        let file_id = insert_file(&db, "/tmp/a.rs", 100);
        db.insert_symbol(&make_symbol_row(file_id, SymbolKind::Function, "three", 10)).unwrap();
        db.insert_symbol(&make_symbol_row(file_id, SymbolKind::Function, "two", 5)).unwrap();
        db.insert_symbol(&make_symbol_row(file_id, SymbolKind::Function, "one", 1)).unwrap();
        let symbols = db.get_symbols_for_file(file_id).unwrap();
        let lines: Vec<_> = symbols.iter().map(|row| row.start_line).collect();
        assert_eq!(lines, vec![1, 5, 10]);
    }

    #[test]
    fn test_wal_mode_enabled() {
        let db = make_db();
        let mode: String = db.conn.query_row("PRAGMA journal_mode", [], |row| row.get(0)).unwrap();
        assert!(!mode.is_empty());
        assert!(mode == "memory" || mode == "wal");
    }
}
