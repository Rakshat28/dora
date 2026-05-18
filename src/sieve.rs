#![allow(dead_code)]

use std::collections::HashSet;
use std::convert::TryInto;
use std::fs::Metadata;
use std::path::Path;

use crate::bloom::{BloomFilter, BLOOM_BYTES};
use crate::index::IndexManifest;
use crate::trigram::extract_query_trigrams;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryTrigramSet {
    pub per_query_trigrams: Vec<Vec<[u8; 3]>>,
    pub has_literals: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileIndexStatus {
    NotIndexed,
    Fresh(BloomFilter),
    Stale,
}

pub fn extract_literal_strings_from_queries(query_sources: &[String]) -> Vec<String> {
    query_sources.iter().flat_map(|query| extract_literal_strings_from_query(query)).collect()
}

pub fn build_query_trigram_set(query_sources: &[String]) -> QueryTrigramSet {
    let mut per_query_trigrams: Vec<Vec<[u8; 3]>> = Vec::with_capacity(query_sources.len());

    for query in query_sources {
        let literals = extract_literal_strings_from_query(query);
        let mut unique = HashSet::new();
        for literal in literals {
            unique.extend(extract_query_trigrams(&literal));
        }
        per_query_trigrams.push(unique.into_iter().collect());
    }

    let has_literals = per_query_trigrams.iter().any(|trigrams| !trigrams.is_empty());

    if !has_literals {
        return QueryTrigramSet { per_query_trigrams, has_literals: false };
    }

    QueryTrigramSet { per_query_trigrams, has_literals: true }
}

pub fn should_parse_file(filter: &BloomFilter, trigram_set: &QueryTrigramSet) -> bool {
    if !trigram_set.has_literals {
        return true;
    }

    for trigrams in &trigram_set.per_query_trigrams {
        if trigrams.is_empty() {
            continue;
        }
        if filter.probably_contains_all(trigrams) {
            return true;
        }
    }

    false
}

pub fn get_file_index_status(
    manifest: &IndexManifest,
    path: &Path,
    metadata: &Metadata,
) -> FileIndexStatus {
    let entry = match manifest.find_entry(path) {
        Some(entry) => entry,
        None => return FileIndexStatus::NotIndexed,
    };

    if entry.mtime_secs != metadata_mtime_secs(metadata) || entry.file_size_bytes != metadata.len() {
        return FileIndexStatus::Stale;
    }

    let bloom_bits: [u8; BLOOM_BYTES] = match entry.bloom_bits.as_slice().try_into() {
        Ok(bits) => bits,
        Err(_) => return FileIndexStatus::Stale,
    };

    FileIndexStatus::Fresh(BloomFilter::from_bytes(bloom_bits))
}

fn extract_literal_strings_from_query(query: &str) -> Vec<String> {
    let bytes = query.as_bytes();
    let mut literals = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] != b'"' {
            index += 1;
            continue;
        }

        let start = index + 1;
        index += 1;

        while index < bytes.len() {
            if bytes[index] == b'"' && !is_escaped_quote(bytes, index) {
                if index > start {
                    literals.push(query[start..index].to_string());
                }
                index += 1;
                break;
            }
            index += 1;
        }

        if index >= bytes.len() {
            break;
        }
    }

    literals
}

fn is_escaped_quote(bytes: &[u8], quote_index: usize) -> bool {
    if quote_index == 0 {
        return false;
    }

    let mut backslash_count = 0;
    let mut index = quote_index;
    while index > 0 {
        index -= 1;
        if bytes[index] == b'\\' {
            backslash_count += 1;
        } else {
            break;
        }
    }

    backslash_count % 2 == 1
}

fn metadata_mtime_secs(metadata: &Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bloom::BloomFilter;
    use crate::index::{IndexEntry, IndexManifest};
    use crate::trigram::extract_unique_trigrams_from_bytes;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_extract_literal_strings_eq_predicate() {
        let queries = vec![("(function_item name: (identifier) @fn (#eq? @fn \"authenticate\"))").to_string()];
        let literals = extract_literal_strings_from_queries(&queries);
        assert!(literals.contains(&"authenticate".to_string()));
    }

    #[test]
    fn test_extract_literal_strings_match_predicate() {
        let queries = vec![("(function_item name: (identifier) @fn (#match? @fn \"^handle_\"))").to_string()];
        let literals = extract_literal_strings_from_queries(&queries);
        assert!(literals.contains(&"^handle_".to_string()));
    }

    #[test]
    fn test_extract_literal_strings_multiple_predicates() {
        let queries = vec![("(function_item name: (identifier) @fn (#eq? @fn \"authenticate\") (#match? @fn \"^handle_\"))").to_string()];
        let literals = extract_literal_strings_from_queries(&queries);
        assert!(literals.contains(&"authenticate".to_string()));
        assert!(literals.contains(&"^handle_".to_string()));
    }

    #[test]
    fn test_extract_literal_strings_no_predicates() {
        let queries = vec![("(function_item name: (identifier) @fn_name)").to_string()];
        let literals = extract_literal_strings_from_queries(&queries);
        assert!(literals.is_empty());
    }

    #[test]
    fn test_extract_literal_strings_multiple_queries() {
        let queries = vec![
            "(function_item name: (identifier) @fn (#eq? @fn \"authenticate\"))".to_string(),
            "(function_item name: (identifier) @fn (#match? @fn \"^handle_\"))".to_string(),
        ];
        let literals = extract_literal_strings_from_queries(&queries);
        assert!(literals.contains(&"authenticate".to_string()));
        assert!(literals.contains(&"^handle_".to_string()));
    }

    #[test]
    fn test_extract_literal_strings_escaped_quotes() {
        let queries = vec![("(function_item name: (identifier) @fn (#eq? @fn \"a\\\"b\"))").to_string()];
        let literals = extract_literal_strings_from_queries(&queries);
        assert!(literals.contains(&"a\\\"b".to_string()));
    }

    #[test]
    fn test_build_query_trigram_set_no_literals() {
        let queries = vec![("(function_item name: (identifier) @fn_name)").to_string()];
        let set = build_query_trigram_set(&queries);
        assert!(!set.has_literals);
        assert!(set.per_query_trigrams.is_empty() || set.per_query_trigrams.iter().all(Vec::is_empty));
    }

    #[test]
    fn test_build_query_trigram_set_with_literal() {
        let queries = vec![("(function_item name: (identifier) @fn (#eq? @fn \"connect\"))").to_string()];
        let set = build_query_trigram_set(&queries);
        assert!(set.has_literals);
        assert!(set.per_query_trigrams.iter().any(|trigrams| !trigrams.is_empty()));
        let trigrams = set.per_query_trigrams.iter().find(|trigrams| !trigrams.is_empty()).unwrap();
        let unique: HashSet<_> = trigrams.iter().collect();
        assert_eq!(unique.len(), trigrams.len());
    }

    #[test]
    fn test_build_query_trigram_set_short_literal() {
        let queries = vec![("(function_item name: (identifier) @fn (#eq? @fn \"fn\"))").to_string()];
        let set = build_query_trigram_set(&queries);
        assert!(!set.has_literals);
        assert!(set.per_query_trigrams.iter().all(Vec::is_empty));
    }

    #[test]
    fn test_should_parse_file_no_literals_always_true() {
        let filter = BloomFilter::new();
        let trigram_set = QueryTrigramSet { per_query_trigrams: vec![], has_literals: false };
        assert!(should_parse_file(&filter, &trigram_set));
    }

    #[test]
    fn test_should_parse_file_trigrams_present_returns_true() {
        let mut filter = BloomFilter::new();
        for trigram in extract_unique_trigrams_from_bytes(b"authenticate") {
            filter.insert(&trigram);
        }
        let queries = vec![("(function_item name: (identifier) @fn (#eq? @fn \"authenticate\"))").to_string()];
        let trigram_set = build_query_trigram_set(&queries);
        assert!(should_parse_file(&filter, &trigram_set));
    }

    #[test]
    fn test_should_parse_file_trigrams_absent_returns_false() {
        let filter = BloomFilter::new();
        let queries = vec![("(function_item name: (identifier) @fn (#eq? @fn \"authenticate\"))").to_string()];
        let trigram_set = build_query_trigram_set(&queries);
        assert!(!should_parse_file(&filter, &trigram_set));
    }

    #[test]
    fn test_should_parse_file_multi_query_one_present() {
        let mut filter = BloomFilter::new();
        for trigram in extract_unique_trigrams_from_bytes(b"connect") {
            filter.insert(&trigram);
        }
        let queries = vec![
            "(function_item name: (identifier) @fn (#eq? @fn \"authenticate\"))".to_string(),
            "(function_item name: (identifier) @fn (#eq? @fn \"connect\"))".to_string(),
        ];
        let trigram_set = build_query_trigram_set(&queries);
        assert!(should_parse_file(&filter, &trigram_set));
    }

    #[test]
    fn test_should_parse_file_multi_query_all_absent() {
        let filter = BloomFilter::new();
        let queries = vec![
            "(function_item name: (identifier) @fn (#eq? @fn \"authenticate\"))".to_string(),
            "(function_item name: (identifier) @fn (#eq? @fn \"connect\"))".to_string(),
        ];
        let trigram_set = build_query_trigram_set(&queries);
        assert!(!should_parse_file(&filter, &trigram_set));
    }

    #[test]
    fn test_get_file_index_status_not_indexed() {
        let manifest = IndexManifest::new(std::path::PathBuf::from("/tmp/root"));
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a.rs");
        fs::write(&file, "fn main() {}\n").unwrap();
        let metadata = fs::metadata(&file).unwrap();
        assert!(matches!(get_file_index_status(&manifest, &file, &metadata), FileIndexStatus::NotIndexed));
    }

    #[test]
    fn test_get_file_index_status_fresh() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a.rs");
        fs::write(&file, "fn authenticate() {}\n").unwrap();
        let metadata = fs::metadata(&file).unwrap();
        let bloom_bits = {
            let mut filter = BloomFilter::new();
            for trigram in extract_unique_trigrams_from_bytes(b"authenticate") {
                filter.insert(&trigram);
            }
            filter.to_bytes().to_vec()
        };
        let mut manifest = IndexManifest::new(tmp.path().to_path_buf());
        manifest.upsert_entry(IndexEntry {
            path: file.clone(),
            mtime_secs: metadata_mtime_secs(&metadata),
            file_size_bytes: metadata.len(),
            bloom_bits,
            language: "rust".to_string(),
        });
        assert!(matches!(get_file_index_status(&manifest, &file, &metadata), FileIndexStatus::Fresh(_)));
    }

    #[test]
    fn test_get_file_index_status_stale_mtime() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a.rs");
        fs::write(&file, "fn authenticate() {}\n").unwrap();
        let metadata = fs::metadata(&file).unwrap();
        let bloom_bits = BloomFilter::new().to_bytes().to_vec();
        let mut manifest = IndexManifest::new(tmp.path().to_path_buf());
        manifest.upsert_entry(IndexEntry {
            path: file.clone(),
            mtime_secs: metadata_mtime_secs(&metadata) + 1,
            file_size_bytes: metadata.len(),
            bloom_bits,
            language: "rust".to_string(),
        });
        assert!(matches!(get_file_index_status(&manifest, &file, &metadata), FileIndexStatus::Stale));
    }

    #[test]
    fn test_get_file_index_status_stale_size() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a.rs");
        fs::write(&file, "fn authenticate() {}\n").unwrap();
        let metadata = fs::metadata(&file).unwrap();
        let bloom_bits = BloomFilter::new().to_bytes().to_vec();
        let mut manifest = IndexManifest::new(tmp.path().to_path_buf());
        manifest.upsert_entry(IndexEntry {
            path: file.clone(),
            mtime_secs: metadata_mtime_secs(&metadata),
            file_size_bytes: metadata.len() + 1,
            bloom_bits,
            language: "rust".to_string(),
        });
        assert!(matches!(get_file_index_status(&manifest, &file, &metadata), FileIndexStatus::Stale));
    }

    #[test]
    fn test_get_file_index_status_corrupt_bloom_bits() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a.rs");
        fs::write(&file, "fn authenticate() {}\n").unwrap();
        let metadata = fs::metadata(&file).unwrap();
        let mut manifest = IndexManifest::new(tmp.path().to_path_buf());
        manifest.upsert_entry(IndexEntry {
            path: file.clone(),
            mtime_secs: metadata_mtime_secs(&metadata),
            file_size_bytes: metadata.len(),
            bloom_bits: vec![0u8; BLOOM_BYTES - 1],
            language: "rust".to_string(),
        });
        assert!(matches!(get_file_index_status(&manifest, &file, &metadata), FileIndexStatus::Stale));
    }

    #[test]
    fn test_zero_false_negatives_end_to_end() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a.rs");
        fs::write(&file, "fn authenticate() -> bool { true }\n").unwrap();
        let metadata = fs::metadata(&file).unwrap();
        let mut filter = BloomFilter::new();
        for trigram in extract_unique_trigrams_from_bytes(b"fn authenticate() -> bool { true }\n") {
            filter.insert(&trigram);
        }
        let mut manifest = IndexManifest::new(tmp.path().to_path_buf());
        manifest.upsert_entry(IndexEntry {
            path: file.clone(),
            mtime_secs: metadata_mtime_secs(&metadata),
            file_size_bytes: metadata.len(),
            bloom_bits: filter.to_bytes().to_vec(),
            language: "rust".to_string(),
        });
        let trigram_set = build_query_trigram_set(&[("(function_item name: (identifier) @fn (#eq? @fn \"authenticate\"))").to_string()]);
        match get_file_index_status(&manifest, &file, &metadata) {
            FileIndexStatus::Fresh(found_filter) => {
                assert!(should_parse_file(&found_filter, &trigram_set), "zero false negatives violated");
            }
            other => panic!("expected fresh index status, got {:?}", other),
        }
    }

    #[test]
    fn test_sieve_correctly_rejects_file_without_term() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a.rs");
        fs::write(&file, "fn present() -> bool { true }\n").unwrap();
        let metadata = fs::metadata(&file).unwrap();
        let mut filter = BloomFilter::new();
        for trigram in extract_unique_trigrams_from_bytes(b"fn present() -> bool { true }\n") {
            filter.insert(&trigram);
        }
        let mut manifest = IndexManifest::new(tmp.path().to_path_buf());
        manifest.upsert_entry(IndexEntry {
            path: file.clone(),
            mtime_secs: metadata_mtime_secs(&metadata),
            file_size_bytes: metadata.len(),
            bloom_bits: filter.to_bytes().to_vec(),
            language: "rust".to_string(),
        });
        let trigram_set = build_query_trigram_set(&[("(function_item name: (identifier) @fn (#eq? @fn \"xyzzy_not_present\"))").to_string()]);
        match get_file_index_status(&manifest, &file, &metadata) {
            FileIndexStatus::Fresh(found_filter) => {
                assert!(!should_parse_file(&found_filter, &trigram_set), "Bloom filter unexpectedly accepted a file that should be rejected");
            }
            other => panic!("expected fresh index status, got {:?}", other),
        }
    }
}
