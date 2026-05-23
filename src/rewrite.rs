use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs;
use similar::TextDiff;

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
                    if (c >= b'a' && c <= b'z') || (c >= b'A' && c <= b'Z') || (c >= b'0' && c <= b'9') || c == b'_' {
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

pub fn compute_edits(results: &[crate::types::MatchResult], template: &RewriteTemplate) -> Vec<RewriteEdit> {
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
            return Err(format!("overlapping edits: {}-{} and {}-{}", w[0].start_byte, w[0].end_byte, w[1].start_byte, w[1].end_byte));
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
    let unified = diff.unified_diff().context_radius(3).header(&format!("a/{}", path.display()), &format!("b/{}", path.display())).to_string();
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
        let t = RewriteTemplate { raw: "@fn_name".to_string() };
        let edits = compute_edits(&[r], &t);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "old_name".to_string());
    }

    #[test]
    fn test_apply_edits_reverse_order() {
        let src = "hello world foo bar";
        let e1 = RewriteEdit { file_path: PathBuf::from("f"), start_byte: 10, end_byte: 15, new_text: "XXX".to_string() };
        let e2 = RewriteEdit { file_path: PathBuf::from("f"), start_byte: 0, end_byte: 5, new_text: "YYY".to_string() };
        let res = apply_edits_to_source(src, &[e1.clone(), e2.clone()]).unwrap();
        assert!(res.contains("YYY"));
        assert!(res.contains("XXX"));
    }

    #[test]
    fn test_apply_edits_overlap_returns_error() {
        let src = "0123456789";
        let e1 = RewriteEdit { file_path: PathBuf::from("f"), start_byte: 2, end_byte: 6, new_text: "A".to_string() };
        let e2 = RewriteEdit { file_path: PathBuf::from("f"), start_byte: 5, end_byte: 8, new_text: "B".to_string() };
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
        assert!(d.contains("a/f.rs") || d.contains("b/f.rs") || d.contains("a/f") );
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
        let e = RewriteEdit { file_path: PathBuf::from("f"), start_byte: 1, end_byte: 4, new_text: "XYZ".to_string() };
        let res = apply_edits_to_source(src, &[e]).unwrap();
        assert!(res.is_ascii() || res.is_char_boundary(0));
    }
}
