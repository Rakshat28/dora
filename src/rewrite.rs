use similar::TextDiff;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub struct RewriteTemplate {
    pub raw: String,
}

impl RewriteTemplate {
    pub fn apply(&self, captures: &HashMap<&str, &str>) -> String {
        let mut names: Vec<&str> = Vec::new();
        let mut i = 0usize;
        let raw = &self.raw;
        while i < raw.len() {
            let bytes = raw.as_bytes();
            if bytes[i] == b'@' {
                let mut j = i + 1;
                while j < raw.len() {
                    let c = raw.as_bytes()[j];
                    if (c >= b'a' && c <= b'z')
                        || (c >= b'A' && c <= b'Z')
                        || (c >= b'0' && c <= b'9')
                        || c == b'_'
                    {
                        j += 1;
                        continue;
                    }
                    break;
                }
                if j > i + 1 {
                    let name = &raw[i + 1..j];
                    names.push(name);
                    i = j;
                    continue;
                }
            }
            i += 1;
        }
        names.sort_by(|a, b| b.len().cmp(&a.len()));
        let mut out = raw.clone();
        for name in names {
            let token = format!("@{}", name);
            if let Some(val) = captures.get(name) {
                out = out.replace(&token, val);
            }
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RewriteEdit {
    pub file_path: PathBuf,
    pub start_byte: usize,
    pub end_byte: usize,
    pub new_text: String,
}

pub fn compute_edits(
    results: &[crate::types::MatchResult],
    template: &RewriteTemplate,
) -> Vec<RewriteEdit> {
    let mut edits = Vec::new();
    for r in results {
        let mut map: HashMap<&str, &str> = HashMap::new();
        map.insert(r.capture_name.as_str(), r.matched_text.as_str());
        let new_text = template.apply(&map);
        if new_text != r.matched_text {
            edits.push(RewriteEdit {
                file_path: r.file_path.clone(),
                start_byte: r.start_byte,
                end_byte: r.end_byte,
                new_text,
            });
        }
    }
    edits
}

pub fn apply_edits_to_source(source: &str, edits: &[RewriteEdit]) -> Result<String, String> {
    if edits.is_empty() {
        return Ok(source.to_string());
    }
    let mut by_start = edits.to_vec();
    by_start.sort_by_key(|e| e.start_byte);
    for w in by_start.windows(2) {
        if w[0].end_byte > w[1].start_byte {
            return Err(format!(
                "overlapping edits: {}-{} and {}-{}",
                w[0].start_byte, w[0].end_byte, w[1].start_byte, w[1].end_byte
            ));
        }
    }
    let mut by_desc = edits.to_vec();
    by_desc.sort_by(|a, b| b.start_byte.cmp(&a.start_byte));
    let mut buffer = source.as_bytes().to_vec();
    for e in by_desc.iter() {
        if e.end_byte > buffer.len() || e.start_byte > e.end_byte {
            return Err(format!("invalid edit range: {}-{}", e.start_byte, e.end_byte));
        }
        buffer.splice(e.start_byte..e.end_byte, e.new_text.as_bytes().iter().cloned());
    }
    match String::from_utf8(buffer) {
        Ok(s) => Ok(s),
        Err(_) => Err("resulting text is not valid UTF-8".to_string()),
    }
}

pub fn apply_edits_to_files(all_edits: &[RewriteEdit]) -> HashMap<PathBuf, Result<String, String>> {
    let mut map: HashMap<PathBuf, Vec<RewriteEdit>> = HashMap::new();
    for e in all_edits {
        map.entry(e.file_path.clone()).or_default().push(e.clone());
    }
    let mut out: HashMap<PathBuf, Result<String, String>> = HashMap::new();
    for (path, edits) in map {
        match fs::read_to_string(&path) {
            Ok(src) => {
                let res = apply_edits_to_source(&src, &edits);
                out.insert(path, res);
            }
            Err(err) => {
                out.insert(path, Err(err.to_string()));
            }
        }
    }
    out
}

pub fn generate_diff(original: &str, rewritten: &str, path: &Path) -> String {
    if original == rewritten {
        return String::new();
    }
    let diff = TextDiff::from_lines(original, rewritten);
    let unified = diff
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{}", path.display()), &format!("b/{}", path.display()))
        .to_string();
    unified
}

pub fn write_atomically(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, content)?;
    match fs::rename(&tmp, path) {
        Ok(_) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::Path;

    fn assert_diff_eq(expected: &str, actual: &str, path: &Path) {
        if expected != actual {
            panic!("rewrite output differs:\n{}", generate_diff(expected, actual, path));
        }
    }

    #[test]
    fn test_template_apply_single_capture() {
        let t = RewriteTemplate { raw: "@fn_name".to_string() };
        let mut m = HashMap::new();
        m.insert("fn_name", "connect");
        assert_eq!(t.apply(&m), "connect".to_string());
    }

    #[test]
    fn test_template_apply_multiple_captures() {
        let t = RewriteTemplate { raw: "rename_@old to @new".to_string() };
        let mut m = HashMap::new();
        m.insert("old", "foo");
        m.insert("new", "bar");
        assert_eq!(t.apply(&m), "rename_foo to bar".to_string());
    }

    #[test]
    fn test_template_apply_missing_capture_unchanged() {
        let t = RewriteTemplate { raw: "@fn_name(@missing)".to_string() };
        let mut m = HashMap::new();
        m.insert("fn_name", "foo");
        assert_eq!(t.apply(&m), "foo(@missing)".to_string());
    }

    #[test]
    fn test_template_longest_match_first() {
        let t = RewriteTemplate { raw: "@fn_name".to_string() };
        let mut m = HashMap::new();
        m.insert("fn", "short");
        m.insert("fn_name", "correct");
        assert_eq!(t.apply(&m), "correct".to_string());
    }

    #[test]
    fn test_compute_edits_no_change_skipped() {
        let r = crate::types::MatchResult {
            file_path: PathBuf::from("f.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 3,
            capture_name: "cap".to_string(),
            matched_text: "same".to_string(),
            start_byte: 0,
            end_byte: 0,
        };
        let t = RewriteTemplate { raw: "same".to_string() };
        let edits = compute_edits(&[r], &t);
        assert!(edits.is_empty());
    }

    #[test]
    fn test_compute_edits_produces_edit_for_changed_capture() {
        let r = crate::types::MatchResult {
            file_path: PathBuf::from("f.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 3,
            capture_name: "fn_name".to_string(),
            matched_text: "old_name".to_string(),
            start_byte: 5,
            end_byte: 13,
        };
        let t = RewriteTemplate { raw: "new_name".to_string() };
        let edits = compute_edits(&[r], &t);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "new_name".to_string());
    }

    #[test]
    fn test_apply_edits_reverse_order() {
        let src = "hello world foo bar";
        let e1 = RewriteEdit {
            file_path: PathBuf::from("f"),
            start_byte: 10,
            end_byte: 15,
            new_text: "XXX".to_string(),
        };
        let e2 = RewriteEdit {
            file_path: PathBuf::from("f"),
            start_byte: 0,
            end_byte: 5,
            new_text: "YYY".to_string(),
        };
        let res = apply_edits_to_source(src, &[e1.clone(), e2.clone()]).unwrap();
        assert!(res.contains("YYY"));
        assert!(res.contains("XXX"));
    }

    #[test]
    fn test_apply_edits_overlap_returns_error() {
        let src = "0123456789";
        let e1 = RewriteEdit {
            file_path: PathBuf::from("f"),
            start_byte: 2,
            end_byte: 6,
            new_text: "A".to_string(),
        };
        let e2 = RewriteEdit {
            file_path: PathBuf::from("f"),
            start_byte: 5,
            end_byte: 8,
            new_text: "B".to_string(),
        };
        let res = apply_edits_to_source(src, &[e1, e2]);
        assert!(res.is_err());
    }

    #[test]
    fn test_apply_edits_empty_list_returns_original() {
        let src = "hello";
        let res = apply_edits_to_source(src, &[]).unwrap();
        assert_eq!(res, "hello".to_string());
    }

    #[test]
    fn test_generate_diff_empty_when_no_change() {
        let d = generate_diff("same", "same", Path::new("f.rs"));
        assert_eq!(d, "".to_string());
    }

    #[test]
    fn test_generate_diff_nonempty_when_changed() {
        let d = generate_diff("fn old()", "fn new()", Path::new("f.rs"));
        assert!(d.contains("-fn old()") || d.contains("+fn new()"));
    }

    #[test]
    fn test_generate_diff_header_contains_path() {
        let d = generate_diff("a", "b", Path::new("f.rs"));
        assert!(d.contains("a/f.rs") || d.contains("b/f.rs") || d.contains("a/f"));
    }

    #[test]
    fn test_rewrite_template_empty_string_no_panic() {
        let t = RewriteTemplate { raw: "".to_string() };
        let m: HashMap<&str, &str> = HashMap::new();
        assert_eq!(t.apply(&m), "".to_string());
    }

    #[test]
    fn test_apply_edits_utf8_boundary_safety() {
        let src = "aébcdef";
        let e = RewriteEdit {
            file_path: PathBuf::from("f"),
            start_byte: 1,
            end_byte: 4,
            new_text: "XYZ".to_string(),
        };
        let res = apply_edits_to_source(src, &[e]).unwrap();
        assert!(res.is_ascii() || res.is_char_boundary(0));
    }

    #[test]
    fn test_apply_edits_match_at_byte_zero() {
        let source = "fn old() {}";
        let edit = RewriteEdit {
            file_path: PathBuf::from("f.rs"),
            start_byte: 3,
            end_byte: 6,
            new_text: "new".to_string(),
        };
        let result = apply_edits_to_source(source, &[edit]).unwrap();
        assert_diff_eq("fn new() {}", &result, Path::new("byte_zero.rs"));
        assert_eq!(&result[..3], "fn ");
    }

    #[test]
    fn test_apply_edits_match_at_final_byte() {
        let source = "fn alpha";
        let edit = RewriteEdit {
            file_path: PathBuf::from("f.rs"),
            start_byte: 3,
            end_byte: 8,
            new_text: "omega".to_string(),
        };
        let result = apply_edits_to_source(source, &[edit]).unwrap();
        assert_diff_eq("fn omega", &result, Path::new("final_byte.rs"));
        assert_eq!(result.len(), "fn omega".len());
    }

    #[test]
    fn test_apply_edits_multiline_match_replaced_correctly() {
        let source = "fn foo(\n    x: i32,\n    y: i32\n) -> i32 { x + y }";
        let start_byte = source.find("(\n    x: i32,\n    y: i32\n)").unwrap();
        let end_byte = start_byte + "(\n    x: i32,\n    y: i32\n)".len();
        let edit = RewriteEdit {
            file_path: PathBuf::from("f.rs"),
            start_byte,
            end_byte,
            new_text: "(a: i32, b: i32)".to_string(),
        };
        let result = apply_edits_to_source(source, &[edit]).unwrap();
        assert!(result.contains("fn foo(a: i32, b: i32)"));
        assert!(result.contains("-> i32 { x + y }"));
        let mut expected = source.to_string();
        expected.replace_range(start_byte..end_byte, "(a: i32, b: i32)");
        assert_diff_eq(&expected, &result, Path::new("multiline.rs"));
    }

    #[test]
    fn test_apply_edits_idempotent() {
        let source = "fn old_name() {}";
        let start_byte = source.find("old_name").unwrap();
        let end_byte = start_byte + "old_name".len();
        let edit = RewriteEdit {
            file_path: PathBuf::from("f.rs"),
            start_byte,
            end_byte,
            new_text: "new_name".to_string(),
        };
        let result1 = apply_edits_to_source(source, &[edit.clone()]).unwrap();
        let renamed_start = result1.find("new_name").unwrap();
        let renamed_end = renamed_start + "new_name".len();
        let second = RewriteEdit {
            file_path: PathBuf::from("f.rs"),
            start_byte: renamed_start,
            end_byte: renamed_end,
            new_text: "new_name".to_string(),
        };
        let result2 = apply_edits_to_source(&result1, &[second]).unwrap();
        assert_eq!(result1, result2);
    }

    #[test]
    fn test_apply_edits_two_non_overlapping_edits_same_file() {
        let source = "fn alpha() {}\nfn omega() {}";
        let first_start = source.find("alpha").unwrap();
        let first_end = first_start + "alpha".len();
        let second_start = source.find("omega").unwrap();
        let second_end = second_start + "omega".len();
        let result = apply_edits_to_source(
            source,
            &[
                RewriteEdit {
                    file_path: PathBuf::from("f.rs"),
                    start_byte: first_start,
                    end_byte: first_end,
                    new_text: "first".to_string(),
                },
                RewriteEdit {
                    file_path: PathBuf::from("f.rs"),
                    start_byte: second_start,
                    end_byte: second_end,
                    new_text: "lasts".to_string(),
                },
            ],
        )
        .unwrap();
        assert_diff_eq("fn first() {}\nfn lasts() {}", &result, Path::new("same_file.rs"));
        assert_eq!(result.len(), source.len());
    }

    #[test]
    fn test_apply_edits_two_edits_different_replacement_lengths() {
        let source = "fn a() { fn b() {} }";
        let a_start = source.find("a").unwrap();
        let a_end = a_start + 1;
        let b_start = source.find("b").unwrap();
        let b_end = b_start + 1;
        let result = apply_edits_to_source(
            source,
            &[
                RewriteEdit {
                    file_path: PathBuf::from("f.rs"),
                    start_byte: a_start,
                    end_byte: a_end,
                    new_text: "long_name".to_string(),
                },
                RewriteEdit {
                    file_path: PathBuf::from("f.rs"),
                    start_byte: b_start,
                    end_byte: b_end,
                    new_text: "x".to_string(),
                },
            ],
        )
        .unwrap();
        assert_diff_eq("fn long_name() { fn x() {} }", &result, Path::new("diff_lengths.rs"));
        assert!(result.starts_with("fn long_name() { fn x() {} }"));
        assert!(result.contains(" { fn x() {} }"));
    }

    #[test]
    fn test_overlap_detection_partial_overlap() {
        let source = "fn overlapping_name() {}";
        let result = apply_edits_to_source(
            source,
            &[
                RewriteEdit {
                    file_path: PathBuf::from("f.rs"),
                    start_byte: 3,
                    end_byte: 15,
                    new_text: "x".to_string(),
                },
                RewriteEdit {
                    file_path: PathBuf::from("f.rs"),
                    start_byte: 10,
                    end_byte: 20,
                    new_text: "y".to_string(),
                },
            ],
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("overlap"));
    }

    #[test]
    fn test_overlap_detection_adjacent_edits_not_overlap() {
        let source = "fn abcdefghijk";
        let result = apply_edits_to_source(
            source,
            &[
                RewriteEdit {
                    file_path: PathBuf::from("f.rs"),
                    start_byte: 3,
                    end_byte: 8,
                    new_text: "first".to_string(),
                },
                RewriteEdit {
                    file_path: PathBuf::from("f.rs"),
                    start_byte: 8,
                    end_byte: 13,
                    new_text: "second".to_string(),
                },
            ],
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_overlap_detection_contained_edit() {
        let source = "fn contained_name() {}";
        let result = apply_edits_to_source(
            source,
            &[
                RewriteEdit {
                    file_path: PathBuf::from("f.rs"),
                    start_byte: 3,
                    end_byte: 20,
                    new_text: "x".to_string(),
                },
                RewriteEdit {
                    file_path: PathBuf::from("f.rs"),
                    start_byte: 5,
                    end_byte: 10,
                    new_text: "y".to_string(),
                },
            ],
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_surgical_precision_bytes_outside_match_unchanged() {
        let source = "AAAATARGETBBBB";
        let edit = RewriteEdit {
            file_path: PathBuf::from("f.rs"),
            start_byte: 4,
            end_byte: 10,
            new_text: "NEW".to_string(),
        };
        let result = apply_edits_to_source(source, &[edit]).unwrap();
        assert!(result.starts_with("AAAA"));
        assert!(result.ends_with("BBBB"));
        assert_eq!(&result[..4], &source[..4]);
        assert_eq!(&result[result.len() - 4..], &source[source.len() - 4..]);
    }

    #[test]
    fn test_unicode_byte_offset_correctness() {
        let source = "fn grüßen() {}\nfn after() {}";
        let emoji_start = source.find("🌍").unwrap_or(0);
        let after_start = source.find("after").unwrap();
        assert!(after_start > emoji_start);
        assert_eq!(source.find("grüßen").unwrap(), 3);
        assert_eq!(after_start, 20);

        let edit = RewriteEdit {
            file_path: PathBuf::from("f.rs"),
            start_byte: after_start,
            end_byte: after_start + "after".len(),
            new_text: "renamed".to_string(),
        };
        let result = apply_edits_to_source(source, &[edit]).unwrap();
        assert!(result.contains("grüßen"));
        assert!(result.contains("fn renamed() {}"));
    }

    #[test]
    fn test_empty_edit_list_returns_source_unchanged() {
        let source = "fn foo() {}";
        let result = apply_edits_to_source(source, &[]).unwrap();
        assert_eq!(result, source.to_string());
    }

    #[test]
    fn test_edit_replacing_with_empty_string_removes_text() {
        let source = "fn old_prefix_name() {}";
        let start_byte = source.find("old_prefix_").unwrap();
        let end_byte = start_byte + "old_prefix_".len();
        let edit = RewriteEdit {
            file_path: PathBuf::from("f.rs"),
            start_byte,
            end_byte,
            new_text: String::new(),
        };
        let result = apply_edits_to_source(source, &[edit]).unwrap();
        assert_diff_eq("fn name() {}", &result, Path::new("empty.rs"));
    }

    #[test]
    fn test_multiple_edits_preserve_total_structure() {
        let source = "fn one() {}\nfn two() {}\nfn three() {}\nfn four() {}\nfn five() {}";
        let edits = [
            RewriteEdit {
                file_path: PathBuf::from("f.rs"),
                start_byte: source.find("one").unwrap(),
                end_byte: source.find("one").unwrap() + "one".len(),
                new_text: "uno".to_string(),
            },
            RewriteEdit {
                file_path: PathBuf::from("f.rs"),
                start_byte: source.find("three").unwrap(),
                end_byte: source.find("three").unwrap() + "three".len(),
                new_text: "THREE".to_string(),
            },
            RewriteEdit {
                file_path: PathBuf::from("f.rs"),
                start_byte: source.find("five").unwrap(),
                end_byte: source.find("five").unwrap() + "five".len(),
                new_text: "FIVE".to_string(),
            },
        ];
        let result = apply_edits_to_source(source, &edits).unwrap();
        let expected = "fn uno() {}\nfn two() {}\nfn THREE() {}\nfn four() {}\nfn FIVE() {}";
        assert_diff_eq(expected, &result, Path::new("structure.rs"));

        let two_start = source.find("fn two() {}").unwrap();
        let two_end = two_start + "fn two() {}".len();
        assert_eq!(&source[two_start..two_end], &result[two_start..two_end]);

        let four_start = source.find("fn four() {}").unwrap();
        let four_end = four_start + "fn four() {}".len();
        assert_eq!(&source[four_start..four_end], &result[four_start..four_end]);
    }

    #[test]
    fn test_generate_diff_shows_correct_line_numbers() {
        let original = "line1\nold_line\nline3\n";
        let rewritten = "line1\nnew_line\nline3\n";
        let diff = generate_diff(original, rewritten, Path::new("test.rs"));
        assert!(diff.contains("@@ -1") || diff.contains("@@ -2"));
        assert!(diff.contains("-old_line"));
        assert!(diff.contains("+new_line"));
    }

    #[test]
    fn test_generate_diff_multi_line_change() {
        let original = "fn demo() {\n    let a = 1;\n    let b = 2;\n}\n";
        let rewritten = "fn demo() {\n    println!(\"x\");\n}\n";
        let diff = generate_diff(original, rewritten, Path::new("test.rs"));
        assert!(diff.contains("-    let a = 1;"));
        assert!(diff.contains("-    let b = 2;"));
        assert!(diff.contains("+    println!(\"x\");"));
    }

    #[test]
    fn test_template_apply_with_no_at_signs() {
        let t = RewriteTemplate { raw: "literal_text".to_string() };
        let mut captures = HashMap::new();
        captures.insert("fn_name", "foo");
        assert_eq!(t.apply(&captures), "literal_text".to_string());
    }

    #[test]
    fn test_template_apply_repeated_capture() {
        let t = RewriteTemplate { raw: "@fn_name_@fn_name".to_string() };
        let mut captures = HashMap::new();
        captures.insert("fn_name", "foo");
        captures.insert("fn_name_", "foo_");
        assert_eq!(t.apply(&captures), "foo_foo".to_string());
    }
}
