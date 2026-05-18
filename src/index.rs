#![allow(dead_code)]

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use bincode;
use serde::{Deserialize, Serialize};

use crate::types::{AppError, Result};

pub const INDEX_FILENAME: &str = ".dora-index";

pub const INDEX_FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct IndexEntry {
    pub path: PathBuf,
    pub mtime_secs: u64,
    pub file_size_bytes: u64,
    pub bloom_bits: Vec<u8>,
    pub language: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct IndexManifest {
    pub version: u32,
    pub indexed_at_secs: u64,
    pub root_path: PathBuf,
    pub entries: Vec<IndexEntry>,
}

impl IndexManifest {
    #[must_use]
    pub fn new(root_path: PathBuf) -> Self {
        IndexManifest {
            version: INDEX_FORMAT_VERSION,
            indexed_at_secs: current_unix_secs(),
            root_path,
            entries: Vec::new(),
        }
    }

    #[must_use]
    pub fn find_entry(&self, path: &Path) -> Option<&IndexEntry> {
        self.entries.iter().find(|e| e.path == path)
    }

    pub fn upsert_entry(&mut self, entry: IndexEntry) {
        if let Some(pos) = self.entries.iter().position(|e| e.path == entry.path) {
            self.entries[pos] = entry;
        } else {
            self.entries.push(entry);
        }
    }

    pub fn remove_stale_entries(&mut self, current_paths: &HashSet<PathBuf>) {
        self.entries.retain(|e| current_paths.contains(&e.path));
    }
}

#[allow(clippy::missing_errors_doc)]
pub fn save_index(manifest: &IndexManifest, index_path: &Path) -> Result<()> {
    let encoded = bincode::serde::encode_to_vec(manifest, bincode::config::standard())
        .map_err(|e| AppError::IndexCorrupt(e.to_string()))?;
    let tmp_path = match index_path.file_name() {
        Some(file_name) => {
            let mut tmp_name = file_name.to_os_string();
            tmp_name.push(".tmp");
            index_path.with_file_name(tmp_name)
        }
        None => index_path.with_extension("tmp"),
    };
    fs::write(&tmp_path, &encoded)?;
    match fs::rename(&tmp_path, index_path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            Err(e.into())
        }
    }
}

#[allow(clippy::missing_errors_doc)]
pub fn load_index(index_path: &Path) -> Result<IndexManifest> {
    let bytes = fs::read(index_path)?;
    let (manifest, _) =
        bincode::serde::decode_from_slice::<IndexManifest, _>(&bytes, bincode::config::standard())
            .map_err(|e| AppError::IndexCorrupt(e.to_string()))?;
    if manifest.version != INDEX_FORMAT_VERSION {
        return Err(AppError::IndexVersionMismatch {
            found: manifest.version,
            expected: INDEX_FORMAT_VERSION,
        });
    }
    Ok(manifest)
}

#[must_use]
pub fn index_path_for_root(root: &Path) -> PathBuf {
    root.join(INDEX_FILENAME)
}

#[must_use]
pub fn index_exists(root: &Path) -> bool {
    index_path_for_root(root).exists()
}

fn current_unix_secs() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_index_path_for_root() {
        let p = index_path_for_root(Path::new("/tmp/myrepo"));
        assert_eq!(p, PathBuf::from("/tmp/myrepo/.dora-index"));
    }

    #[test]
    fn test_index_exists_false_when_no_file() {
        let tmp = TempDir::new().unwrap();
        assert!(!index_exists(tmp.path()));
    }

    #[test]
    fn test_index_exists_true_when_file_present() {
        let tmp = TempDir::new().unwrap();
        let idx = index_path_for_root(tmp.path());
        fs::write(&idx, b"").unwrap();
        assert!(index_exists(tmp.path()));
    }

    #[test]
    fn test_index_manifest_new() {
        let m = IndexManifest::new(PathBuf::from("/tmp/test"));
        assert_eq!(m.version, INDEX_FORMAT_VERSION);
        assert_eq!(m.root_path, PathBuf::from("/tmp/test"));
        assert!(m.entries.is_empty());
        assert!(m.indexed_at_secs > 0);
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let idx_path = index_path_for_root(tmp.path());
        let mut m = IndexManifest::new(tmp.path().to_path_buf());
        let e1 = IndexEntry {
            path: PathBuf::from("/tmp/a.rs"),
            mtime_secs: 1,
            file_size_bytes: 10,
            bloom_bits: vec![1, 2, 3],
            language: "rust".to_string(),
        };
        let e2 = IndexEntry {
            path: PathBuf::from("/tmp/b.rs"),
            mtime_secs: 2,
            file_size_bytes: 20,
            bloom_bits: vec![4, 5, 6],
            language: "python".to_string(),
        };
        m.entries.push(e1.clone());
        m.entries.push(e2.clone());
        save_index(&m, &idx_path).unwrap();
        let loaded = load_index(&idx_path).unwrap();
        assert_eq!(loaded, m);
        assert_eq!(loaded.entries.len(), 2);
    }

    #[test]
    fn test_load_nonexistent_returns_io_error() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("nope");
        let res = load_index(&p);
        assert!(matches!(res, Err(AppError::IoError(_))));
    }

    #[test]
    fn test_load_corrupt_data_returns_index_corrupt() {
        let tmp = TempDir::new().unwrap();
        let p = index_path_for_root(tmp.path());
        let mut f = fs::File::create(&p).unwrap();
        let _ = f.write_all(b"not a valid index");
        let res = load_index(&p);
        assert!(matches!(res, Err(AppError::IndexCorrupt(_))));
    }

    #[test]
    fn test_version_mismatch_returns_error() {
        let tmp = TempDir::new().unwrap();
        let p = index_path_for_root(tmp.path());
        let mut m = IndexManifest::new(tmp.path().to_path_buf());
        m.version = 999;
        let encoded = bincode::serde::encode_to_vec(&m, bincode::config::standard()).unwrap();
        fs::write(&p, &encoded).unwrap();
        let res = load_index(&p);
        assert!(matches!(res, Err(AppError::IndexVersionMismatch { .. })));
    }

    #[test]
    fn test_upsert_entry_inserts_new() {
        let mut m = IndexManifest::new(PathBuf::from("/tmp/test"));
        let e = IndexEntry {
            path: PathBuf::from("/tmp/a.rs"),
            mtime_secs: 1,
            file_size_bytes: 10,
            bloom_bits: vec![],
            language: "rust".to_string(),
        };
        m.upsert_entry(e);
        assert_eq!(m.entries.len(), 1);
    }

    #[test]
    fn test_upsert_entry_replaces_existing() {
        let mut m = IndexManifest::new(PathBuf::from("/tmp/test"));
        let e1 = IndexEntry {
            path: PathBuf::from("/tmp/a.rs"),
            mtime_secs: 1,
            file_size_bytes: 10,
            bloom_bits: vec![],
            language: "rust".to_string(),
        };
        m.upsert_entry(e1);
        let e2 = IndexEntry {
            path: PathBuf::from("/tmp/a.rs"),
            mtime_secs: 42,
            file_size_bytes: 10,
            bloom_bits: vec![],
            language: "rust".to_string(),
        };
        m.upsert_entry(e2);
        assert_eq!(m.entries.len(), 1);
        assert_eq!(m.entries[0].mtime_secs, 42);
    }

    #[test]
    fn test_find_entry_returns_correct() {
        let mut m = IndexManifest::new(PathBuf::from("/tmp/test"));
        let a = IndexEntry {
            path: PathBuf::from("/tmp/a.rs"),
            mtime_secs: 1,
            file_size_bytes: 10,
            bloom_bits: vec![],
            language: "rust".to_string(),
        };
        let b = IndexEntry {
            path: PathBuf::from("/tmp/b.rs"),
            mtime_secs: 2,
            file_size_bytes: 20,
            bloom_bits: vec![],
            language: "python".to_string(),
        };
        let c = IndexEntry {
            path: PathBuf::from("/tmp/c.rs"),
            mtime_secs: 3,
            file_size_bytes: 30,
            bloom_bits: vec![],
            language: "js".to_string(),
        };
        m.entries.push(a.clone());
        m.entries.push(b.clone());
        m.entries.push(c.clone());
        let found = m.find_entry(Path::new("/tmp/b.rs")).unwrap();
        assert_eq!(found.path, PathBuf::from("/tmp/b.rs"));
    }

    #[test]
    fn test_find_entry_returns_none_for_missing() {
        let m = IndexManifest::new(PathBuf::from("/tmp/test"));
        let found = m.find_entry(Path::new("/tmp/missing.rs"));
        assert!(found.is_none());
    }

    #[test]
    fn test_remove_stale_entries() {
        let mut m = IndexManifest::new(PathBuf::from("/tmp/test"));
        let a = IndexEntry {
            path: PathBuf::from("/tmp/a.rs"),
            mtime_secs: 1,
            file_size_bytes: 10,
            bloom_bits: vec![],
            language: "rust".to_string(),
        };
        let b = IndexEntry {
            path: PathBuf::from("/tmp/b.rs"),
            mtime_secs: 2,
            file_size_bytes: 20,
            bloom_bits: vec![],
            language: "python".to_string(),
        };
        let c = IndexEntry {
            path: PathBuf::from("/tmp/c.rs"),
            mtime_secs: 3,
            file_size_bytes: 30,
            bloom_bits: vec![],
            language: "js".to_string(),
        };
        m.entries.push(a.clone());
        m.entries.push(b.clone());
        m.entries.push(c.clone());
        let mut set = HashSet::new();
        set.insert(PathBuf::from("/tmp/a.rs"));
        set.insert(PathBuf::from("/tmp/c.rs"));
        m.remove_stale_entries(&set);
        assert_eq!(m.entries.len(), 2);
        assert!(m.find_entry(Path::new("/tmp/b.rs")).is_none());
    }

    #[test]
    fn test_save_is_atomic_temp_file_cleaned_up() {
        let tmp = TempDir::new().unwrap();
        let p = index_path_for_root(tmp.path());
        let m = IndexManifest::new(tmp.path().to_path_buf());
        save_index(&m, &p).unwrap();
        let tmp_path =
            p.with_file_name(format!("{}.tmp", p.file_name().unwrap().to_string_lossy()));
        assert!(!tmp_path.exists());
        assert!(p.exists());
    }

    #[test]
    fn test_index_entry_derives() {
        let e1 = IndexEntry {
            path: PathBuf::from("/tmp/a.rs"),
            mtime_secs: 1,
            file_size_bytes: 10,
            bloom_bits: vec![1, 2, 3],
            language: "rust".to_string(),
        };
        let e2 = e1.clone();
        assert_eq!(e1, e2);
        let e3 = e1.clone();
        assert_eq!(e3, e1);
    }

    #[test]
    fn test_manifest_with_large_bloom_bits() {
        let tmp = TempDir::new().unwrap();
        let p = index_path_for_root(tmp.path());
        let mut m = IndexManifest::new(tmp.path().to_path_buf());
        let e = IndexEntry {
            path: PathBuf::from("/tmp/large.rs"),
            mtime_secs: 1,
            file_size_bytes: 4096,
            bloom_bits: vec![0u8; 4096],
            language: "rust".to_string(),
        };
        m.entries.push(e.clone());
        save_index(&m, &p).unwrap();
        let loaded = load_index(&p).unwrap();
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].bloom_bits.len(), 4096);
    }
}
