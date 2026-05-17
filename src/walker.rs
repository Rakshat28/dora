use std::path::Path;

use ignore::{DirEntry, WalkBuilder};

use crate::types::{AppError, Language, Result};

const EXCLUDED_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "target",
    "node_modules",
    ".tox",
    ".venv",
    "venv",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    "dist",
    "build",
    ".idea",
    ".vscode",
];

fn is_excluded_dir(entry: &DirEntry) -> bool {
    entry.file_type().is_some_and(|ft| ft.is_dir())
        && entry.file_name().to_str().is_some_and(|name| EXCLUDED_DIRS.contains(&name))
}

fn is_binary(path: &Path) -> bool {
    use std::io::Read;

    let Ok(mut file) = std::fs::File::open(path) else {
        return true;
    };

    let mut buffer = [0u8; 8192];
    let Ok(n) = file.read(&mut buffer) else {
        return true;
    };

    let sample = &buffer[..n];

    if sample.contains(&0u8) {
        return true;
    }

    std::str::from_utf8(sample).is_err()
}

#[must_use]
pub fn extensions_for_language(lang: &Language) -> &'static [&'static str] {
    match lang {
        Language::Rust => &["rs"],
        Language::Python => &["py", "pyi"],
        Language::JavaScript => &["js", "mjs", "cjs"],
        Language::TypeScript => &["ts", "mts", "cts", "tsx"],
        Language::Go => &["go"],
        Language::C => &["c", "h"],
        Language::Cpp => &["cpp", "cc", "hpp", "hxx", "cxx", "h"],
    }
}

pub fn build_walker(root: &Path, lang: &Language) -> impl Iterator<Item = Result<DirEntry>> {
    let extensions = extensions_for_language(lang);

    WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .follow_links(false)
        .filter_entry(|entry| !is_excluded_dir(entry))
        .build()
        .filter_map(move |result| match result {
            Err(error) => Some(Err(AppError::WalkError(error))),
            Ok(entry) => {
                let is_file = entry.file_type().is_some_and(|ft| ft.is_file());
                let ext_matches =
                    entry.path().extension().and_then(|extension| extension.to_str()).is_some_and(
                        |extension| extensions.contains(&extension.to_lowercase().as_str()),
                    );

                if is_file && ext_matches && !is_binary(entry.path()) {
                    Some(Ok(entry))
                } else {
                    None
                }
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn walk_names(root: &TempDir, lang: &Language) -> Vec<String> {
        build_walker(root.path(), lang)
            .collect::<Result<Vec<_>>>()
            .expect("walk failed")
            .into_iter()
            .filter_map(|e| e.path().file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect()
    }

    fn walk_paths(root: &TempDir, lang: &Language) -> Vec<PathBuf> {
        build_walker(root.path(), lang)
            .collect::<Result<Vec<_>>>()
            .expect("walk failed")
            .into_iter()
            .map(|e| e.path().to_path_buf())
            .collect()
    }

    fn write_file(root: &TempDir, rel_path: &str, content: &[u8]) {
        let full = root.path().join(rel_path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(full, content).unwrap();
    }

    mod test_extensions {
        use super::*;

        #[test]
        fn rust_extension_set() {
            let exts = extensions_for_language(&Language::Rust);
            assert!(exts.contains(&"rs"), "Rust must include 'rs'");
            assert_eq!(exts.len(), 1, "Rust should have exactly one extension");
        }

        #[test]
        fn python_extension_set() {
            let exts = extensions_for_language(&Language::Python);
            assert!(exts.contains(&"py"));
            assert!(exts.contains(&"pyi"));
        }

        #[test]
        fn javascript_extension_set() {
            let exts = extensions_for_language(&Language::JavaScript);
            assert!(exts.contains(&"js"));
            assert!(exts.contains(&"mjs"));
            assert!(exts.contains(&"cjs"));
        }

        #[test]
        fn typescript_extension_set() {
            let exts = extensions_for_language(&Language::TypeScript);
            assert!(exts.contains(&"ts"));
            assert!(exts.contains(&"tsx"));
            assert!(exts.contains(&"mts"));
            assert!(exts.contains(&"cts"));
        }

        #[test]
        fn go_extension_set() {
            let exts = extensions_for_language(&Language::Go);
            assert!(exts.contains(&"go"));
        }

        #[test]
        fn c_extension_set() {
            let exts = extensions_for_language(&Language::C);
            assert!(exts.contains(&"c"));
            assert!(exts.contains(&"h"));
        }

        #[test]
        fn cpp_extension_set() {
            let exts = extensions_for_language(&Language::Cpp);
            assert!(exts.contains(&"cpp"));
            assert!(exts.contains(&"cc"));
            assert!(exts.contains(&"hpp"));
            assert!(exts.contains(&"hxx"));
            assert!(exts.contains(&"cxx"));
            assert!(exts.contains(&"h"));
        }

        #[test]
        fn no_extension_set_is_empty() {
            for lang in &[
                Language::Rust,
                Language::Python,
                Language::JavaScript,
                Language::TypeScript,
                Language::Go,
                Language::C,
                Language::Cpp,
            ] {
                assert!(
                    !extensions_for_language(lang).is_empty(),
                    "{:?} returned an empty extension set",
                    lang
                );
            }
        }

        #[test]
        fn only_rs_files_for_rust() {
            let dir = TempDir::new().unwrap();
            write_file(&dir, "main.rs", b"fn main() {}");
            write_file(&dir, "lib.rs", b"pub fn foo() {}");
            write_file(&dir, "script.py", b"print('hi')");
            write_file(&dir, "app.js", b"console.log('hi')");
            write_file(&dir, "notes.md", b"# notes");

            let names = walk_names(&dir, &Language::Rust);

            assert!(names.contains(&"main.rs".to_string()));
            assert!(names.contains(&"lib.rs".to_string()));
            assert!(!names.contains(&"script.py".to_string()));
            assert!(!names.contains(&"app.js".to_string()));
            assert!(!names.contains(&"notes.md".to_string()));
        }

        #[test]
        fn only_py_files_for_python() {
            let dir = TempDir::new().unwrap();
            write_file(&dir, "main.py", b"print('hi')");
            write_file(&dir, "stub.pyi", b"def foo() -> None: ...");
            write_file(&dir, "main.rs", b"fn main() {}");

            let names = walk_names(&dir, &Language::Python);

            assert!(names.contains(&"main.py".to_string()));
            assert!(names.contains(&"stub.pyi".to_string()));
            assert!(!names.contains(&"main.rs".to_string()));
        }

        #[test]
        fn js_extensions_accepted() {
            let dir = TempDir::new().unwrap();
            write_file(&dir, "app.js", b"const x = 1;");
            write_file(&dir, "mod.mjs", b"export const x = 1;");
            write_file(&dir, "cjs.cjs", b"module.exports = {};");
            write_file(&dir, "main.ts", b"const x: number = 1;");

            let names = walk_names(&dir, &Language::JavaScript);

            assert!(names.contains(&"app.js".to_string()));
            assert!(names.contains(&"mod.mjs".to_string()));
            assert!(names.contains(&"cjs.cjs".to_string()));
            assert!(
                !names.contains(&"main.ts".to_string()),
                "TypeScript files should not match JavaScript filter"
            );
        }

        #[test]
        fn extension_filter_case_insensitive() {
            let dir = TempDir::new().unwrap();
            write_file(&dir, "Main.RS", b"fn main() {}");
            write_file(&dir, "Lib.Rs", b"pub fn foo() {}");
            write_file(&dir, "other.py", b"pass");

            let names = walk_names(&dir, &Language::Rust);

            assert!(names.contains(&"Main.RS".to_string()), "Uppercase .RS should match Rust");
            assert!(names.contains(&"Lib.Rs".to_string()), "Mixed-case .Rs should match Rust");
            assert!(!names.contains(&"other.py".to_string()));
        }

        #[test]
        fn files_with_no_extension_excluded() {
            let dir = TempDir::new().unwrap();
            write_file(&dir, "Makefile", b"all:");
            write_file(&dir, "Dockerfile", b"FROM ubuntu");
            write_file(&dir, "main.rs", b"fn main() {}");

            let names = walk_names(&dir, &Language::Rust);

            assert!(!names.contains(&"Makefile".to_string()));
            assert!(!names.contains(&"Dockerfile".to_string()));
            assert!(names.contains(&"main.rs".to_string()));
        }

        #[test]
        fn nested_files_filtered_correctly() {
            let dir = TempDir::new().unwrap();
            write_file(&dir, "src/main.rs", b"fn main() {}");
            write_file(&dir, "src/util.py", b"pass");
            write_file(&dir, "src/nested/lib.rs", b"");

            let names = walk_names(&dir, &Language::Rust);

            assert!(names.contains(&"main.rs".to_string()));
            assert!(names.contains(&"lib.rs".to_string()));
            assert!(!names.contains(&"util.py".to_string()));
        }
    }

    mod test_gitignore {
        use super::*;

        #[test]
        fn gitignore_excludes_named_file() {
            let dir = TempDir::new().unwrap();
            fs::create_dir(dir.path().join(".git")).unwrap();
            write_file(&dir, ".gitignore", b"secret.rs\n");
            write_file(&dir, "secret.rs", b"fn secret() {}");
            write_file(&dir, "main.rs", b"fn main() {}");

            let names = walk_names(&dir, &Language::Rust);

            assert!(!names.contains(&"secret.rs".to_string()), "secret.rs should be gitignored");
            assert!(names.contains(&"main.rs".to_string()));
        }

        #[test]
        fn gitignore_excludes_by_glob() {
            let dir = TempDir::new().unwrap();
            fs::create_dir(dir.path().join(".git")).unwrap();
            write_file(&dir, ".gitignore", b"*.generated.rs\n");
            write_file(&dir, "foo.generated.rs", b"// generated");
            write_file(&dir, "bar.generated.rs", b"// generated");
            write_file(&dir, "main.rs", b"fn main() {}");

            let names = walk_names(&dir, &Language::Rust);

            assert!(!names.contains(&"foo.generated.rs".to_string()));
            assert!(!names.contains(&"bar.generated.rs".to_string()));
            assert!(names.contains(&"main.rs".to_string()));
        }

        #[test]
        fn gitignore_excludes_entire_subdirectory() {
            let dir = TempDir::new().unwrap();
            fs::create_dir(dir.path().join(".git")).unwrap();
            write_file(&dir, ".gitignore", b"generated/\n");
            write_file(&dir, "generated/foo.rs", b"");
            write_file(&dir, "generated/bar.rs", b"");
            write_file(&dir, "src/main.rs", b"fn main() {}");

            let names = walk_names(&dir, &Language::Rust);

            assert!(
                !names.contains(&"foo.rs".to_string()),
                "Files in gitignored dir should not appear"
            );
            assert!(!names.contains(&"bar.rs".to_string()));
            assert!(names.contains(&"main.rs".to_string()));
        }

        #[test]
        fn negated_gitignore_rules_re_include_files() {
            let dir = TempDir::new().unwrap();
            fs::create_dir(dir.path().join(".git")).unwrap();
            write_file(&dir, ".gitignore", b"*.rs\n!main.rs\n");
            write_file(&dir, "main.rs", b"fn main() {}");
            write_file(&dir, "lib.rs", b"pub fn foo() {}");

            let names = walk_names(&dir, &Language::Rust);

            assert!(
                names.contains(&"main.rs".to_string()),
                "Negated gitignore rule should re-include main.rs"
            );
            assert!(!names.contains(&"lib.rs".to_string()), "lib.rs should remain ignored");
        }

        #[test]
        fn ignore_file_respected_alongside_gitignore() {
            let dir = TempDir::new().unwrap();
            write_file(&dir, ".ignore", b"scratch.rs\n");
            write_file(&dir, "scratch.rs", b"fn scratch() {}");
            write_file(&dir, "main.rs", b"fn main() {}");

            let names = walk_names(&dir, &Language::Rust);

            assert!(!names.contains(&"scratch.rs".to_string()), ".ignore file should be respected");
            assert!(names.contains(&"main.rs".to_string()));
        }

        #[test]
        fn no_gitignore_yields_all_matching_files() {
            let dir = TempDir::new().unwrap();
            write_file(&dir, "a.rs", b"");
            write_file(&dir, "b.rs", b"");
            write_file(&dir, "c.rs", b"");

            let names = walk_names(&dir, &Language::Rust);

            assert_eq!(names.len(), 3, "Without .gitignore all .rs files should be yielded");
        }
    }

    mod test_excluded_dirs {
        use super::*;

        fn assert_dir_excluded(dir_name: &str) {
            let root = TempDir::new().unwrap();
            write_file(&root, &format!("{}/file.rs", dir_name), b"fn foo() {}");
            write_file(&root, "visible.rs", b"fn main() {}");

            let names = walk_names(&root, &Language::Rust);

            assert!(
                !names.contains(&"file.rs".to_string()),
                "file.rs inside '{}' should be excluded but was found",
                dir_name
            );
            assert!(names.contains(&"visible.rs".to_string()));
        }

        #[test]
        fn excludes_target() {
            assert_dir_excluded("target");
        }
        #[test]
        fn excludes_node_modules() {
            assert_dir_excluded("node_modules");
        }
        #[test]
        fn excludes_git() {
            assert_dir_excluded(".git");
        }
        #[test]
        fn excludes_hg() {
            assert_dir_excluded(".hg");
        }
        #[test]
        fn excludes_svn() {
            assert_dir_excluded(".svn");
        }
        #[test]
        fn excludes_tox() {
            assert_dir_excluded(".tox");
        }
        #[test]
        fn excludes_venv() {
            assert_dir_excluded("venv");
        }
        #[test]
        fn excludes_dot_venv() {
            assert_dir_excluded(".venv");
        }
        #[test]
        fn excludes_pycache() {
            assert_dir_excluded("__pycache__");
        }
        #[test]
        fn excludes_mypy_cache() {
            assert_dir_excluded(".mypy_cache");
        }
        #[test]
        fn excludes_pytest_cache() {
            assert_dir_excluded(".pytest_cache");
        }
        #[test]
        fn excludes_dist() {
            assert_dir_excluded("dist");
        }
        #[test]
        fn excludes_build() {
            assert_dir_excluded("build");
        }
        #[test]
        fn excludes_idea() {
            assert_dir_excluded(".idea");
        }
        #[test]
        fn excludes_vscode() {
            assert_dir_excluded(".vscode");
        }

        #[test]
        fn non_excluded_dirs_are_walked() {
            let dir = TempDir::new().unwrap();
            for subdir in &["src", "lib", "tests", "benches", "examples", "crates"] {
                write_file(&dir, &format!("{}/file.rs", subdir), b"fn foo() {}");
            }

            let names = walk_names(&dir, &Language::Rust);

            assert_eq!(
                names.iter().filter(|n| *n == "file.rs").count(),
                6,
                "All 6 non-excluded subdirs should contribute file.rs"
            );
        }

        #[test]
        fn excluded_dir_name_as_file_is_allowed() {
            let dir = TempDir::new().unwrap();
            write_file(&dir, "target.rs", b"fn target() {}");

            let names = walk_names(&dir, &Language::Rust);

            assert!(
                names.contains(&"target.rs".to_string()),
                "A file named target.rs should not be excluded"
            );
        }

        #[test]
        fn deeply_nested_excluded_dir_pruned() {
            let dir = TempDir::new().unwrap();
            write_file(&dir, "a/b/target/deep.rs", b"fn deep() {}");
            write_file(&dir, "a/b/real.rs", b"fn real() {}");

            let names = walk_names(&dir, &Language::Rust);

            assert!(
                !names.contains(&"deep.rs".to_string()),
                "target/ nested deep should still be excluded"
            );
            assert!(names.contains(&"real.rs".to_string()));
        }
    }

    mod test_binary {
        use super::*;

        #[test]
        fn null_byte_file_excluded() {
            let dir = TempDir::new().unwrap();
            write_file(&dir, "null.rs", b"fn main() \x00 {}");

            let names = walk_names(&dir, &Language::Rust);
            assert!(
                !names.contains(&"null.rs".to_string()),
                "File with null byte should be excluded"
            );
        }

        #[test]
        fn null_byte_at_start_excluded() {
            let dir = TempDir::new().unwrap();
            write_file(&dir, "early_null.rs", b"\x00fn main() {}");

            let names = walk_names(&dir, &Language::Rust);
            assert!(!names.contains(&"early_null.rs".to_string()));
        }

        #[test]
        fn null_byte_at_end_excluded() {
            let dir = TempDir::new().unwrap();
            let mut content = b"fn main() {}".to_vec();
            content.push(0u8);
            write_file(&dir, "end_null.rs", &content);

            let names = walk_names(&dir, &Language::Rust);
            assert!(!names.contains(&"end_null.rs".to_string()));
        }

        #[test]
        fn invalid_utf8_excluded() {
            let dir = TempDir::new().unwrap();
            write_file(&dir, "bad_utf8.rs", &[0x66, 0x6E, 0x20, 0x80, 0x81]);

            let names = walk_names(&dir, &Language::Rust);
            assert!(!names.contains(&"bad_utf8.rs".to_string()));
        }

        #[test]
        fn valid_utf8_with_non_ascii_included() {
            let dir = TempDir::new().unwrap();
            write_file(&dir, "unicode.rs", "fn greet() { println!(\"héllo wörld\"); }".as_bytes());

            let names = walk_names(&dir, &Language::Rust);
            assert!(
                names.contains(&"unicode.rs".to_string()),
                "Valid UTF-8 with non-ASCII chars should not be treated as binary"
            );
        }

        #[test]
        fn empty_file_included() {
            let dir = TempDir::new().unwrap();
            write_file(&dir, "empty.rs", b"");

            let names = walk_names(&dir, &Language::Rust);
            assert!(
                names.contains(&"empty.rs".to_string()),
                "Empty file should not be treated as binary"
            );
        }

        #[test]
        fn large_valid_file_included() {
            let dir = TempDir::new().unwrap();
            let content = "fn foo() {}\n".repeat(1700);
            write_file(&dir, "large.rs", content.as_bytes());

            let names = walk_names(&dir, &Language::Rust);
            assert!(
                names.contains(&"large.rs".to_string()),
                "Large valid UTF-8 file should be included"
            );
        }

        #[test]
        fn binary_beyond_sniff_window_included() {
            let dir = TempDir::new().unwrap();
            let mut content = vec![b'a'; 8192];
            content.push(0u8);
            write_file(&dir, "late_null.rs", &content);

            let names = walk_names(&dir, &Language::Rust);
            assert!(
                names.contains(&"late_null.rs".to_string()),
                "Null byte beyond the 8 KB sniff window is not detected — documented behavior"
            );
        }
    }

    mod test_integration {
        use super::*;

        fn make_realistic_project(root: &TempDir) {
            write_file(root, "src/main.rs", b"fn main() {}");
            write_file(root, "src/lib.rs", b"pub mod walker;");
            write_file(root, "src/walker.rs", b"pub fn build_walker() {}");
            write_file(root, "tests/integration.rs", b"#[test] fn it_works() {}");
            write_file(root, "benches/bench.rs", b"fn bench() {}");

            write_file(root, ".gitignore", b"*.snap\nfixtures/ignored/\n");
            write_file(root, "tests/snapshots/a.snap", b"---\nvalue: 42\n");
            write_file(root, "fixtures/ignored/x.rs", b"fn ignored() {}");

            write_file(root, "target/debug/main", b"\x7fELF");
            write_file(root, "target/release/lib.rs", b"// generated");
            write_file(root, ".git/config", b"[core]");

            write_file(root, "scripts/build.py", b"import sys");
            write_file(root, "frontend/app.ts", b"const x = 1;");
            write_file(root, "README.md", b"# My Project");

            write_file(root, "src/corrupted.rs", b"\x00\x01\x02\x03");
        }

        #[test]
        fn realistic_project_yields_correct_files() {
            let dir = TempDir::new().unwrap();
            make_realistic_project(&dir);

            let names = walk_names(&dir, &Language::Rust);

            assert!(names.contains(&"main.rs".to_string()));
            assert!(names.contains(&"lib.rs".to_string()));
            assert!(names.contains(&"walker.rs".to_string()));
            assert!(names.contains(&"integration.rs".to_string()));
            assert!(names.contains(&"bench.rs".to_string()));

            assert!(!names.contains(&"a.snap".to_string()));
            assert!(!names.contains(&"x.rs".to_string()));

            assert!(!names.contains(&"main".to_string()));

            assert!(!names.contains(&"build.py".to_string()));
            assert!(!names.contains(&"app.ts".to_string()));
            assert!(!names.contains(&"README.md".to_string()));

            assert!(!names.contains(&"corrupted.rs".to_string()));
        }

        #[test]
        fn walker_yields_no_directories() {
            let dir = TempDir::new().unwrap();
            make_realistic_project(&dir);

            let entries =
                build_walker(dir.path(), &Language::Rust).collect::<Result<Vec<_>>>().unwrap();

            for entry in &entries {
                assert!(
                    entry.file_type().map_or(false, |ft| ft.is_file()),
                    "Expected only files, got non-file entry: {:?}",
                    entry.path()
                );
            }
        }

        #[test]
        fn walker_is_deterministic_across_runs() {
            let dir = TempDir::new().unwrap();
            for i in 0..10 {
                write_file(&dir, &format!("src/file_{}.rs", i), b"fn foo() {}");
            }

            let mut run1 = walk_paths(&dir, &Language::Rust);
            let mut run2 = walk_paths(&dir, &Language::Rust);

            run1.sort();
            run2.sort();

            assert_eq!(run1, run2, "Walker should yield the same set of paths across runs");
        }

        #[test]
        fn empty_directory_yields_no_entries() {
            let dir = TempDir::new().unwrap();
            let result =
                build_walker(dir.path(), &Language::Rust).collect::<Result<Vec<_>>>().unwrap();
            assert!(result.is_empty(), "Empty directory should yield no entries");
        }

        #[test]
        fn single_matching_file_at_root() {
            let dir = TempDir::new().unwrap();
            write_file(&dir, "main.rs", b"fn main() {}");

            let names = walk_names(&dir, &Language::Rust);
            assert_eq!(names, vec!["main.rs".to_string()]);
        }

        #[test]
        fn nonexistent_root_returns_error() {
            let result: Result<Vec<_>> =
                build_walker(std::path::Path::new("/nonexistent/xyz/abc"), &Language::Rust)
                    .collect();
            assert!(result.is_err(), "Nonexistent root should surface an error");
        }
    }
}
