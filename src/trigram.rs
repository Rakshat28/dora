#![allow(dead_code)]

use std::collections::HashSet;

#[must_use]
pub fn extract_trigrams(text: &str) -> Vec<[u8; 3]> {
    sliding_window(text.as_bytes())
}

#[must_use]
pub fn extract_trigrams_from_bytes(bytes: &[u8]) -> Vec<[u8; 3]> {
    sliding_window(bytes)
}

#[must_use]
pub fn extract_unique_trigrams(text: &str) -> Vec<[u8; 3]> {
    unique_sliding_window(text.as_bytes())
}

#[must_use]
pub fn extract_unique_trigrams_from_bytes(bytes: &[u8]) -> Vec<[u8; 3]> {
    unique_sliding_window(bytes)
}

#[must_use]
pub fn extract_query_trigrams(query_literal: &str) -> Vec<[u8; 3]> {
    unique_sliding_window(query_literal.as_bytes())
}

fn sliding_window(bytes: &[u8]) -> Vec<[u8; 3]> {
    if bytes.len() < 3 {
        return Vec::new();
    }
    let mut result = Vec::with_capacity(bytes.len().saturating_sub(2));
    for index in 0..bytes.len() - 2 {
        result.push([bytes[index], bytes[index + 1], bytes[index + 2]]);
    }
    result
}

fn unique_sliding_window(bytes: &[u8]) -> Vec<[u8; 3]> {
    if bytes.len() < 3 {
        return Vec::new();
    }
    let capacity = (bytes.len() - 2) / 4;
    let mut seen = HashSet::with_capacity(capacity);
    for index in 0..bytes.len() - 2 {
        seen.insert([bytes[index], bytes[index + 1], bytes[index + 2]]);
    }
    seen.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_empty_string_produces_no_trigrams() {
        assert!(extract_trigrams("").is_empty());
    }

    #[test]
    fn test_one_byte_produces_no_trigrams() {
        assert!(extract_trigrams("a").is_empty());
    }

    #[test]
    fn test_two_bytes_produces_no_trigrams() {
        assert!(extract_trigrams("ab").is_empty());
    }

    #[test]
    fn test_exactly_three_bytes_produces_one_trigram() {
        let trigrams = extract_trigrams("abc");
        assert_eq!(trigrams, vec![[b'a', b'b', b'c']]);
        assert_eq!(trigrams.len(), 1);
    }

    #[test]
    fn test_four_bytes_produces_two_trigrams() {
        let trigrams = extract_trigrams("abcd");
        assert_eq!(trigrams, vec![[b'a', b'b', b'c'], [b'b', b'c', b'd']]);
        assert_eq!(trigrams.len(), 2);
    }

    #[test]
    fn test_hello_produces_three_trigrams() {
        let trigrams = extract_trigrams("hello");
        assert_eq!(trigrams.len(), 3);
        assert_eq!(trigrams[0], [b'h', b'e', b'l']);
        assert_eq!(trigrams[1], [b'e', b'l', b'l']);
        assert_eq!(trigrams[2], [b'l', b'l', b'o']);
    }

    #[test]
    fn test_trigram_count_equals_len_minus_two() {
        for length in [3usize, 10, 100, 1000] {
            let text = "x".repeat(length);
            assert_eq!(extract_trigrams(&text).len(), length - 2);
        }
    }

    #[test]
    fn test_sliding_window_advances_by_one_byte() {
        let trigrams = extract_trigrams("abcde");
        assert_eq!(trigrams.len(), 3);
        assert_eq!(trigrams[0], [b'a', b'b', b'c']);
        assert_eq!(trigrams[1], [b'b', b'c', b'd']);
        assert_eq!(trigrams[2], [b'c', b'd', b'e']);
    }

    #[test]
    fn test_unique_trigrams_removes_duplicates() {
        let trigrams = extract_unique_trigrams("aaa");
        assert_eq!(trigrams, vec![[b'a', b'a', b'a']]);
        assert_eq!(trigrams.len(), 1);
    }

    #[test]
    fn test_unique_trigrams_subset_of_all_trigrams() {
        let text = "the quick brown fox";
        let all: HashSet<[u8; 3]> = extract_trigrams(text).into_iter().collect();
        for trigram in extract_unique_trigrams(text) {
            assert!(all.contains(&trigram));
        }
    }

    #[test]
    fn test_unique_trigrams_no_duplicates() {
        let result = extract_unique_trigrams("abcabcabc");
        let as_set: HashSet<[u8; 3]> = result.iter().cloned().collect();
        assert_eq!(result.len(), as_set.len());
    }

    #[test]
    fn test_repeated_pattern_deduplicates_heavily() {
        let text = "fn fn fn fn fn fn";
        let all = extract_trigrams(text);
        let unique = extract_unique_trigrams(text);
        assert!(unique.len() < all.len());
    }

    #[test]
    fn test_ascii_only_source() {
        let text = "fn main() {}";
        assert_eq!(extract_trigrams(text).len(), text.len() - 2);
    }

    #[test]
    fn test_non_ascii_utf8_operates_on_bytes() {
        let text = "héllo";
        let trigrams = extract_trigrams(text);
        assert_eq!(trigrams.len(), text.len() - 2);
        assert_eq!(trigrams[0], [b'h', 0xC3, 0xA9]);
    }

    #[test]
    fn test_bytes_variant_matches_str_variant() {
        let text = "fn greet(name: &str)";
        assert_eq!(extract_trigrams(text), extract_trigrams_from_bytes(text.as_bytes()));
    }

    #[test]
    fn test_pure_bytes_with_null_bytes() {
        let bytes: &[u8] = &[0x00, 0x01, 0x02, 0x03];
        let trigrams = extract_trigrams_from_bytes(bytes);
        assert_eq!(trigrams.len(), 2);
        assert_eq!(trigrams[0], [0x00, 0x01, 0x02]);
        assert_eq!(trigrams[1], [0x01, 0x02, 0x03]);
    }

    #[test]
    fn test_query_trigrams_short_string_returns_empty() {
        assert!(extract_query_trigrams("fn").is_empty());
        assert!(extract_query_trigrams("f").is_empty());
        assert!(extract_query_trigrams("").is_empty());
    }

    #[test]
    fn test_query_trigrams_exact_match_string() {
        let result = extract_query_trigrams("authenticate");
        assert!(!result.is_empty());
        let as_set: HashSet<[u8; 3]> = result.iter().cloned().collect();
        assert_eq!(result.len(), as_set.len());
    }

    #[test]
    fn test_query_trigrams_are_unique() {
        let result = extract_query_trigrams("abcabcabc");
        let as_set: HashSet<[u8; 3]> = result.iter().cloned().collect();
        assert_eq!(result.len(), as_set.len());
    }

    #[test]
    fn test_query_trigrams_three_byte_string() {
        let result = extract_query_trigrams("foo");
        assert_eq!(result, vec![[b'f', b'o', b'o']]);
    }

    #[test]
    fn test_trigrams_cover_all_byte_positions() {
        let text = "abcdef";
        let trigrams = extract_trigrams(text);
        let bytes = text.as_bytes();
        let first_bytes: Vec<u8> = trigrams.iter().map(|trigram| trigram[0]).collect();
        assert_eq!(first_bytes, bytes[..bytes.len() - 2].to_vec());
    }

    #[test]
    fn test_no_out_of_bounds_on_any_ascii_length() {
        for length in 0..=20 {
            let text = "x".repeat(length);
            let trigrams = extract_trigrams(&text);
            let expected = length.saturating_sub(2);
            assert_eq!(trigrams.len(), expected);
        }
    }

    #[test]
    fn test_trigram_bytes_are_exact_source_bytes() {
        let text = "rustacean";
        let trigrams = extract_trigrams(text);
        let bytes = text.as_bytes();
        for (index, trigram) in trigrams.iter().enumerate() {
            assert_eq!(trigram[0], bytes[index]);
            assert_eq!(trigram[1], bytes[index + 1]);
            assert_eq!(trigram[2], bytes[index + 2]);
        }
    }

    #[test]
    fn test_large_source_performance_does_not_panic() {
        let text = "x".repeat(1_000_000);
        let trigrams = extract_unique_trigrams(&text);
        assert!(!trigrams.is_empty());
    }

    #[test]
    fn test_realistic_rust_source_trigrams() {
        let source = "fn authenticate(user: &str, password: &str) -> bool { true }";
        let trigrams = extract_unique_trigrams(source);
        assert!(trigrams.contains(&[b'a', b'u', b't']));
        assert!(trigrams.contains(&[b'f', b'n', b' ']));
        assert!(trigrams.contains(&[b'b', b'o', b'o']));
    }

    #[test]
    fn test_query_trigrams_from_s_expression_literal() {
        let result = extract_query_trigrams("connect");
        assert!(result.contains(&[b'c', b'o', b'n']));
        assert!(result.contains(&[b'o', b'n', b'n']));
        assert!(result.contains(&[b'n', b'n', b'e']));
        assert!(result.contains(&[b'n', b'e', b'c']));
        assert!(result.contains(&[b'e', b'c', b't']));
        assert_eq!(result.len(), 5);
    }
}
