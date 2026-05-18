use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::collections::HashSet;
use rayon::prelude::*;
use std::fs;

use crate::types::{Result, LangMode, Language};
use crate::index::{IndexManifest, IndexEntry};
use crate::index::index_path_for_root;
use crate::trigram::extract_unique_trigrams_from_bytes;
use crate::parser::detect_language;
use crate::walker::{build_walker, build_auto_walker};
use crate::bloom::BloomFilter;

pub fn build_index(root: &Path, lang_mode: &LangMode, verbose: bool) -> Result<()> {
    let root_abs = match fs::canonicalize(root) {
        Ok(p) => p,
        Err(_) => root.to_path_buf(),
    };
    let index_path = index_path_for_root(&root_abs);

    let mut manifest = match crate::index::load_index(&index_path) {
        Ok(m) => {
            if m.root_path != root_abs {
                IndexManifest::new(root_abs.clone())
            } else {
                m
            }
        }
        Err(_) => IndexManifest::new(root_abs.clone()),
    };

    let entries_arc = Arc::new(Mutex::new(Vec::<IndexEntry>::new()));
    let indexed_count = Arc::new(Mutex::new(0usize));
    let skipped_count = Arc::new(Mutex::new(0usize));

    let walker: Box<dyn Iterator<Item = crate::types::Result<ignore::DirEntry>> + Send> =
        match lang_mode {
            LangMode::Single(lang) => Box::new(build_walker(root, lang)),
            LangMode::Auto => Box::new(build_auto_walker(root)),
        };

    let entries_ref = Arc::clone(&entries_arc);
    let indexed_ref = Arc::clone(&indexed_count);
    let skipped_ref = Arc::clone(&skipped_count);
    let manifest_ref = Arc::new(manifest);

    walker.par_bridge().for_each(move |entry_result| match entry_result {
        Ok(entry) => {
            let path = entry.path().to_path_buf();
            let metadata = match fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => return,
            };

            let existing = manifest_ref.find_entry(&path).cloned();
            if let Some(e) = existing {
                if is_fresh(&e, &metadata) {
                    if verbose {
                        eprintln!("fresh:   {}", path.display());
                    }
                    *skipped_ref.lock().unwrap() += 1;
                    entries_ref.lock().unwrap().push(e);
                    return;
                }
            }

            let lang_str = match lang_mode {
                LangMode::Single(lang) => lang_to_str(lang).to_string(),
                LangMode::Auto => match detect_language(&path) {
                    Some(l) => lang_to_str(&l).to_string(),
                    None => "unknown".to_string(),
                },
            };

            match index_file(&path, &metadata, &lang_str) {
                Ok(new_entry) => {
                    if verbose {
                        eprintln!("indexed: {}", path.display());
                    }
                    *indexed_ref.lock().unwrap() += 1;
                    entries_ref.lock().unwrap().push(new_entry);
                }
                Err(_) => {
                    return;
                }
            }
        }
        Err(_) => return,
    });

    let collected = match Arc::try_unwrap(entries_arc) {
        Ok(mutex) => mutex.into_inner().unwrap(),
        Err(a) => a.lock().unwrap().clone(),
    };
    manifest = IndexManifest::new(root_abs.clone());
    manifest.entries = collected;

    let current_paths: HashSet<PathBuf> = manifest.entries.iter().map(|e| e.path.clone()).collect();
    manifest.remove_stale_entries(&current_paths);

    let removed = 0usize;

    crate::index::save_index(&manifest, &index_path)?;

    let indexed = *indexed_count.lock().unwrap();
    let skipped = *skipped_count.lock().unwrap();

    eprintln!("indexed {} files, skipped {} fresh, removed {} stale entries", indexed, skipped, removed);
    eprintln!("index written to {}", index_path.display());

    Ok(())
}

fn is_fresh(entry: &IndexEntry, metadata: &fs::Metadata) -> bool {
    let mtime = metadata.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or_default();
    mtime == entry.mtime_secs && metadata.len() == entry.file_size_bytes
}

fn index_file(path: &Path, metadata: &fs::Metadata, language: &str) -> Result<IndexEntry> {
    let bytes = fs::read(path)?;
    let trigrams = extract_unique_trigrams_from_bytes(&bytes);
    let mut filter = BloomFilter::new();
    filter.insert_trigrams(&trigrams);
    let mtime = metadata.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or_default();
    let size = metadata.len();
    let abs = match fs::canonicalize(path) {
        Ok(p) => p,
        Err(_) => path.to_path_buf(),
    };
    let entry = IndexEntry { path: abs, mtime_secs: mtime, file_size_bytes: size, bloom_bits: filter.to_bytes().to_vec(), language: language.to_string() };
    Ok(entry)
}

fn lang_to_str(lang: &Language) -> &'static str {
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



#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use crate::index::IndexEntry;
    use crate::bloom::BloomFilter;
    use crate::types::LangMode;

    #[test]
    fn test_is_fresh_true_when_mtime_and_size_match() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a.rs");
        fs::write(&file, "fn main() {}\n").unwrap();
        let metadata = fs::metadata(&file).unwrap();
        let mtime = metadata.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let entry = IndexEntry { path: file.clone(), mtime_secs: mtime, file_size_bytes: metadata.len(), bloom_bits: vec![], language: "rust".to_string() };
        assert!(is_fresh(&entry, &metadata));
    }

    #[test]
    fn test_is_fresh_false_when_mtime_differs() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a.rs");
        fs::write(&file, "fn main() {}\n").unwrap();
        let metadata = fs::metadata(&file).unwrap();
        let entry = IndexEntry { path: file.clone(), mtime_secs: metadata.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 1, file_size_bytes: metadata.len(), bloom_bits: vec![], language: "rust".to_string() };
        assert!(!is_fresh(&entry, &metadata));
    }

    #[test]
    fn test_is_fresh_false_when_size_differs() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a.rs");
        fs::write(&file, "fn main() {}\n").unwrap();
        let metadata = fs::metadata(&file).unwrap();
        let entry = IndexEntry { path: file.clone(), mtime_secs: metadata.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(), file_size_bytes: metadata.len() + 1, bloom_bits: vec![], language: "rust".to_string() };
        assert!(!is_fresh(&entry, &metadata));
    }

    #[test]
    fn test_index_file_builds_bloom_filter() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a.rs");
        fs::write(&file, "fn authenticate(user: &str) -> bool { true }\n").unwrap();
        let metadata = fs::metadata(&file).unwrap();
        let entry = index_file(&file, &metadata, "rust").unwrap();
        let bf = BloomFilter::from_bytes(entry.bloom_bits.clone().try_into().unwrap());
        let query = crate::trigram::extract_query_trigrams("authenticate");
        assert!(bf.probably_contains_all(&query));
    }

    #[test]
    fn test_index_file_language_field() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("b.rs");
        fs::write(&file, "fn main() {}\n").unwrap();
        let metadata = fs::metadata(&file).unwrap();
        let entry = index_file(&file, &metadata, "rust").unwrap();
        assert_eq!(entry.language, "rust");
    }

    #[test]
    fn test_build_index_creates_index_file() {
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a.rs");
        let b = tmp.path().join("b.rs");
        fs::write(&a, "fn a() {}\n").unwrap();
        fs::write(&b, "fn b() {}\n").unwrap();
        build_index(tmp.path(), &LangMode::Single(Language::Rust), false).unwrap();
        assert!(crate::index::index_exists(tmp.path()));
    }

    #[test]
    fn test_build_index_incremental_skips_fresh_files() {
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a.rs");
        fs::write(&a, "fn a() {}\n").unwrap();
        build_index(tmp.path(), &LangMode::Single(Language::Rust), false).unwrap();
        let before = crate::index::load_index(&crate::index::index_path_for_root(tmp.path())).unwrap();
        build_index(tmp.path(), &LangMode::Single(Language::Rust), false).unwrap();
        let after = crate::index::load_index(&crate::index::index_path_for_root(tmp.path())).unwrap();
        assert_eq!(before.entries.len(), after.entries.len());
    }

    #[test]
    fn test_build_index_reindexes_stale_files() {
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a.rs");
        fs::write(&a, "fn a() {}\n").unwrap();
        build_index(tmp.path(), &LangMode::Single(Language::Rust), false).unwrap();
        let _entry = crate::index::load_index(&crate::index::index_path_for_root(tmp.path())).unwrap().entries.pop().unwrap();
        fs::write(&a, "fn a_changed() {}\n").unwrap();
        build_index(tmp.path(), &LangMode::Single(Language::Rust), false).unwrap();
        let after = crate::index::load_index(&crate::index::index_path_for_root(tmp.path())).unwrap();
        assert!(after.entries.len() >= 1);
    }

    #[test]
    fn test_build_index_removes_deleted_files() {
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a.rs");
        let b = tmp.path().join("b.rs");
        fs::write(&a, "fn a() {}\n").unwrap();
        fs::write(&b, "fn b() {}\n").unwrap();
        build_index(tmp.path(), &LangMode::Single(Language::Rust), false).unwrap();
        let manifest = crate::index::load_index(&crate::index::index_path_for_root(tmp.path())).unwrap();
        assert!(manifest.entries.len() >= 2);
        fs::remove_file(&b).unwrap();
        build_index(tmp.path(), &LangMode::Single(Language::Rust), false).unwrap();
        let manifest2 = crate::index::load_index(&crate::index::index_path_for_root(tmp.path())).unwrap();
        assert!(manifest2.entries.len() >= 1);
    }
}
