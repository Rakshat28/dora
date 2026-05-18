use dora::parser::{get_language, parse_file_with_threshold};
use dora::query::{compile_multi_query, extract_multi_matches};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::io::Write;
use std::path::PathBuf;
use tempfile::{NamedTempFile, TempDir};

fn make_rust_source(fn_count: usize) -> String {
    (0..fn_count).map(|i| format!("fn function_{i}(x: i32, y: i32) -> i32 {{ x + y }}\n")).collect()
}

fn make_rust_file(fn_count: usize) -> NamedTempFile {
    let mut file = NamedTempFile::new().unwrap();
    write!(file, "{}", make_rust_source(fn_count)).unwrap();
    file
}

fn make_rust_dir(file_count: usize, fns_per_file: usize) -> TempDir {
    let dir = TempDir::new().unwrap();
    let content = make_rust_source(fns_per_file);
    for i in 0..file_count {
        std::fs::write(dir.path().join(format!("file_{i}.rs")), content.as_bytes()).unwrap();
    }
    dir
}

fn bench_query_compilation(c: &mut Criterion) {
    let lang = get_language("rust").unwrap();
    let query_str = "(function_item name: (identifier) @fn_name)";

    c.bench_function("compile_single_query_rust", |b| {
        b.iter(|| {
            compile_multi_query(black_box(&lang), black_box(&[query_str.to_string()])).unwrap()
        });
    });

    let multi_queries: Vec<String> = vec![
        "(function_item name: (identifier) @fn_name)".to_string(),
        "(struct_item name: (type_identifier) @struct_name)".to_string(),
        "(impl_item type: (type_identifier) @impl_type)".to_string(),
        "(trait_item name: (type_identifier) @trait_name)".to_string(),
        "(enum_item name: (type_identifier) @enum_name)".to_string(),
    ];

    c.bench_function("compile_five_queries_rust", |b| {
        b.iter(|| compile_multi_query(black_box(&lang), black_box(&multi_queries)).unwrap());
    });
}

fn bench_single_file_parse_query(c: &mut Criterion) {
    let lang = get_language("rust").unwrap();
    let query_str = "(function_item name: (identifier) @fn_name)";
    let multi = compile_multi_query(&lang, &[query_str.to_string()]).unwrap();

    let mut group = c.benchmark_group("single_file_parse_query");

    for fn_count in [10, 100, 500, 1000] {
        let file = make_rust_file(fn_count);
        let path = file.path().to_path_buf();

        group.throughput(Throughput::Elements(fn_count as u64));
        group.bench_with_input(BenchmarkId::new("functions", fn_count), &fn_count, |b, _| {
            b.iter(|| {
                let (tree, source) =
                    parse_file_with_threshold(black_box(&path), black_box(&lang), u64::MAX)
                        .unwrap();
                let results = extract_multi_matches(
                    black_box(&tree),
                    black_box(source.as_bytes()),
                    black_box(&multi),
                    black_box(&path),
                );
                drop(tree);
                drop(source);
                black_box(results)
            });
        });
    }

    group.finish();
}

fn bench_parse_heap_vs_mmap(c: &mut Criterion) {
    let lang = get_language("rust").unwrap();

    let file = make_rust_file(500);
    let path = file.path().to_path_buf();

    let mut group = c.benchmark_group("parse_heap_vs_mmap");

    group.bench_function("heap_read", |b| {
        b.iter(|| {
            let (tree, source) =
                parse_file_with_threshold(black_box(&path), black_box(&lang), u64::MAX).unwrap();
            drop(tree);
            drop(source);
            black_box(())
        });
    });

    group.bench_function("mmap_read", |b| {
        b.iter(|| {
            let (tree, source) =
                parse_file_with_threshold(black_box(&path), black_box(&lang), 0).unwrap();
            drop(tree);
            drop(source);
            black_box(())
        });
    });

    group.finish();
}

fn bench_multi_query_overhead(c: &mut Criterion) {
    let lang = get_language("rust").unwrap();

    let single_query = vec!["(function_item name: (identifier) @fn_name)".to_string()];
    let five_queries = vec![
        "(function_item name: (identifier) @fn_name)".to_string(),
        "(struct_item name: (type_identifier) @struct_name)".to_string(),
        "(impl_item type: (type_identifier) @impl_type)".to_string(),
        "(trait_item name: (type_identifier) @trait_name)".to_string(),
        "(enum_item name: (type_identifier) @enum_name)".to_string(),
    ];

    let single = compile_multi_query(&lang, &single_query).unwrap();
    let five = compile_multi_query(&lang, &five_queries).unwrap();

    let file = make_rust_file(100);
    let path = file.path().to_path_buf();
    let (tree, source) = parse_file_with_threshold(&path, &lang, u64::MAX).unwrap();

    let mut group = c.benchmark_group("multi_query_overhead");

    group.bench_function("one_query", |b| {
        b.iter(|| {
            black_box(extract_multi_matches(
                black_box(&tree),
                black_box(source.as_bytes()),
                black_box(&single),
                black_box(&path),
            ))
        });
    });

    group.bench_function("five_queries", |b| {
        b.iter(|| {
            black_box(extract_multi_matches(
                black_box(&tree),
                black_box(source.as_bytes()),
                black_box(&five),
                black_box(&path),
            ))
        });
    });

    group.finish();
}

fn bench_parallel_search_100_files(c: &mut Criterion) {
    use dora::types::MatchResult;
    use rayon::prelude::*;
    use std::sync::{Arc, Mutex};

    let lang_val = get_language("rust").unwrap();
    let lang_arc = Arc::new(lang_val.clone());

    let query_str = "(function_item name: (identifier) @fn_name)";
    let multi = Arc::new(compile_multi_query(&lang_val, &[query_str.to_string()]).unwrap());

    let dir = make_rust_dir(100, 20);
    let paths: Vec<PathBuf> =
        std::fs::read_dir(dir.path()).unwrap().filter_map(|e| e.ok().map(|e| e.path())).collect();

    let mut group = c.benchmark_group("parallel_search_100_files");
    group.sample_size(20);
    group.measurement_time(std::time::Duration::from_secs(30));

    group.bench_function("20fn_each", |b| {
        b.iter(|| {
            let results = Arc::new(Mutex::new(Vec::<MatchResult>::new()));
            let results_ref = Arc::clone(&results);
            let multi_ref = Arc::clone(&multi);
            let lang_ref = Arc::clone(&lang_arc);

            paths.par_iter().for_each(|path| {
                let (tree, source) = match parse_file_with_threshold(path, &lang_ref, u64::MAX) {
                    Ok(p) => p,
                    Err(_) => return,
                };
                let mut matches = extract_multi_matches(&tree, source.as_bytes(), &multi_ref, path);
                drop(tree);
                drop(source);
                results_ref.lock().unwrap().append(&mut matches);
            });

            drop(results_ref);
            let mut final_results = Arc::try_unwrap(results).unwrap().into_inner().unwrap();
            final_results.sort();
            final_results.dedup();
            black_box(final_results)
        });
    });

    group.finish();
}

fn bench_query_compilation_all_languages(c: &mut Criterion) {
    use dora::parser::get_all_languages;

    let query_str = "(identifier) @id";

    c.bench_function("compile_universal_query_all_languages", |b| {
        b.iter(|| {
            black_box(
                get_all_languages()
                    .into_iter()
                    .filter_map(|(_, ts_lang)| {
                        compile_multi_query(
                            black_box(&ts_lang),
                            black_box(&[query_str.to_string()]),
                        )
                        .ok()
                    })
                    .collect::<Vec<_>>(),
            )
        });
    });
}

criterion_group!(
    benches,
    bench_query_compilation,
    bench_single_file_parse_query,
    bench_parse_heap_vs_mmap,
    bench_multi_query_overhead,
    bench_parallel_search_100_files,
    bench_query_compilation_all_languages,
);
criterion_main!(benches);
