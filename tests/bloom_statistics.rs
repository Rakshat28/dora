use doora::bloom::{BloomFilter, BLOOM_BITS};
use doora::sieve::{build_query_trigram_set, should_parse_file};
use doora::trigram::extract_unique_trigrams_from_bytes;
use std::collections::HashSet;

fn generate_fixture_file(index: usize) -> String {
    let mut lines = Vec::new();
    let line_count = 50 + (index % 150);

    for line_idx in 0..line_count {
        let sym_num = index.wrapping_mul(31).wrapping_add(line_idx);
        let sym_xor = index ^ line_idx;
        let sym = format!("sym_{}_{}", sym_num, sym_xor);

        let template_choice = (index ^ line_idx) % 7;
        let line = match template_choice {
            0 => format!("fn {}() {{}}", sym),
            1 => format!("let {} = 42;", sym),
            2 => format!("struct {};", sym),
            3 => format!("impl {} {{}}", sym),
            4 => format!("use {};", sym),
            5 => format!("pub fn {}(x: i32) -> i32 {{ x }}", sym),
            _ => format!("const {}: u32 = {};", sym, sym_num),
        };

        lines.push(line);
    }

    lines.join("\n")
}

fn index_file_to_bloom(source: &str) -> BloomFilter {
    let mut filter = BloomFilter::new();
    let trigrams = extract_unique_trigrams_from_bytes(source.as_bytes());
    for trigram in trigrams {
        filter.insert(&trigram);
    }
    filter
}

fn measure_false_positive_rate(filter: &BloomFilter, absent_trigrams: &[[u8; 3]]) -> f64 {
    if absent_trigrams.is_empty() {
        return 0.0;
    }

    let false_positives = absent_trigrams.iter().filter(|t| filter.probably_contains(t)).count();

    false_positives as f64 / absent_trigrams.len() as f64
}

fn generate_synthetic_absent_trigrams(count: usize) -> Vec<[u8; 3]> {
    let mut trigrams = Vec::new();
    let byte_patterns: &[[u8; 3]] = &[
        [0xFF, 0xFE, 0xFD],
        [0x80, 0x81, 0x82],
        [0xF0, 0xF1, 0xF2],
        [0xE0, 0xE1, 0xE2],
        [0xD0, 0xD1, 0xD2],
    ];

    for i in 0..count {
        let pattern_idx = i % byte_patterns.len();
        let pattern = byte_patterns[pattern_idx];
        let offset: u8 = (i / byte_patterns.len()) as u8;
        trigrams.push([
            pattern[0].wrapping_add(offset),
            pattern[1].wrapping_add(offset),
            pattern[2].wrapping_add(offset),
        ]);
    }

    trigrams
}

fn compute_pearson_correlation(xs: &[f64], ys: &[f64]) -> f64 {
    if xs.len() != ys.len() || xs.is_empty() {
        return 0.0;
    }

    let n = xs.len() as f64;
    let mean_x: f64 = xs.iter().sum::<f64>() / n;
    let mean_y: f64 = ys.iter().sum::<f64>() / n;

    let mut cov = 0.0;
    let mut var_x = 0.0;
    let mut var_y = 0.0;

    for i in 0..xs.len() {
        let dx = xs[i] - mean_x;
        let dy = ys[i] - mean_y;
        cov += dx * dy;
        var_x += dx * dx;
        var_y += dy * dy;
    }

    if var_x == 0.0 || var_y == 0.0 {
        return 0.0;
    }

    cov / (var_x * var_y).sqrt()
}

#[test]
fn test_zero_false_negatives_1000_files() {
    let mut failed = Vec::new();

    for file_idx in 0..1000 {
        let source = generate_fixture_file(file_idx);
        let filter = index_file_to_bloom(&source);
        let trigrams = extract_unique_trigrams_from_bytes(source.as_bytes());

        for trigram in &trigrams {
            if !filter.probably_contains(trigram) {
                failed.push((file_idx, *trigram));
            }
        }
    }

    assert!(
        failed.is_empty(),
        "Found {} false negatives. First failure: file {}, trigram {:?}",
        failed.len(),
        failed.get(0).map(|(f, _)| f).unwrap_or(&0),
        failed.get(0).map(|(_, t)| t).unwrap_or(&[0, 0, 0])
    );
}

#[test]
fn test_false_positive_rate_within_bounds() {
    let mut all_fprs = Vec::new();

    for file_idx in 0..1000 {
        let source = generate_fixture_file(file_idx);
        let filter = index_file_to_bloom(&source);

        let probe_trigrams = generate_synthetic_absent_trigrams(200);
        let fpr = measure_false_positive_rate(&filter, &probe_trigrams);
        all_fprs.push(fpr);
    }

    let mean_fpr: f64 = all_fprs.iter().sum::<f64>() / all_fprs.len() as f64;
    let max_fpr = all_fprs.iter().copied().fold(f64::NEG_INFINITY, f64::max);

    println!("FPR stats: mean={:.4}, max={:.4}", mean_fpr, max_fpr);

    assert!(mean_fpr < 0.15, "Mean FPR {:.4} exceeds empirical bound of 0.15", mean_fpr);
    assert!(max_fpr < 0.30, "Max FPR {:.4} exceeds empirical bound of 0.30", max_fpr);
}

#[test]
fn test_false_positive_rate_increases_with_saturation() {
    let mut filter = BloomFilter::new();
    let probe_set = generate_synthetic_absent_trigrams(200);

    let mut fprs = Vec::new();
    let insert_counts = vec![50, 200, 500, 1000, 2000];

    for insert_count in insert_counts {
        let batch_trigrams = generate_synthetic_absent_trigrams(insert_count - fprs.len() * 50);

        for trigram in
            batch_trigrams.iter().take(insert_count.saturating_sub(filter.bit_count() / 2))
        {
            filter.insert(trigram);
        }

        let fpr = measure_false_positive_rate(&filter, &probe_set);
        fprs.push(fpr);
    }

    for i in 1..fprs.len() {
        assert!(
            fprs[i] >= fprs[i - 1] * 0.99,
            "FPR not monotonically non-decreasing: {} at step {} vs {} at step {}",
            fprs[i - 1],
            i - 1,
            fprs[i],
            i
        );
    }

    assert!(
        fprs[fprs.len() - 1] > fprs[0],
        "FPR at saturation should be higher than at low occupancy"
    );
}

#[test]
fn test_bloom_estimate_correlates_with_empirical_rate() {
    let mut estimates = Vec::new();
    let mut empiricals = Vec::new();

    for file_idx in 0..100 {
        let source = generate_fixture_file(file_idx);
        let filter = index_file_to_bloom(&source);

        let estimate = filter.false_positive_estimate();

        let mut probe_trigrams = Vec::new();
        for other_idx in (file_idx + 200)..(file_idx + 220) {
            let other_idx_wrapped = other_idx % 1000;
            let other_source = generate_fixture_file(other_idx_wrapped);
            let other_trigrams = extract_unique_trigrams_from_bytes(other_source.as_bytes());
            probe_trigrams
                .extend_from_slice(&other_trigrams[..std::cmp::min(5, other_trigrams.len())]);
        }

        probe_trigrams.extend_from_slice(&generate_synthetic_absent_trigrams(50));

        let empirical = measure_false_positive_rate(&filter, &probe_trigrams);

        estimates.push(estimate);
        empiricals.push(empirical);
    }

    let correlation = compute_pearson_correlation(&estimates, &empiricals);

    println!("Pearson correlation (estimate vs empirical): {:.4}", correlation);

    assert!(correlation > 0.5, "Correlation {} does not exceed 0.5 threshold", correlation);
}

#[test]
fn test_sieve_rejection_correctness_1000_files() {
    let mut zero_false_negative_violations = 0;

    for file_idx in 0..1000 {
        let source = generate_fixture_file(file_idx);
        let filter = index_file_to_bloom(&source);

        let search_term = format!("sym_{}_{}", file_idx.wrapping_mul(31), file_idx);
        let query_trigram_set = build_query_trigram_set(&[search_term]);

        if query_trigram_set.has_literals {
            let should_parse = should_parse_file(&filter, &query_trigram_set);
            if !should_parse {
                zero_false_negative_violations += 1;
            }
        }
    }

    let mut rejection_count = 0;
    let mut total_rejection_checks = 0;

    for file_idx in 0..1000 {
        let source = generate_fixture_file(file_idx);
        let filter = index_file_to_bloom(&source);

        let search_term = format!("sym_{}_{}", (file_idx + 500).wrapping_mul(31), (file_idx + 500));
        let query_trigram_set = build_query_trigram_set(&[search_term]);

        if query_trigram_set.has_literals {
            total_rejection_checks += 1;
            let should_parse = should_parse_file(&filter, &query_trigram_set);
            if !should_parse {
                rejection_count += 1;
            }
        }
    }

    assert_eq!(
        zero_false_negative_violations, 0,
        "Found {} false negatives in sieve logic",
        zero_false_negative_violations
    );

    if total_rejection_checks > 0 {
        let rejection_rate = rejection_count as f64 / total_rejection_checks as f64;
        println!(
            "Sieve rejection rate for absent terms: {:.2}% ({}/{})",
            rejection_rate * 100.0,
            rejection_count,
            total_rejection_checks
        );

        assert!(
            rejection_rate > 0.80,
            "Rejection rate {:.2}% does not exceed 80% threshold",
            rejection_rate * 100.0
        );
    }
}

#[test]
fn test_bit_count_correctness() {
    for file_idx in 0..1000 {
        let source = generate_fixture_file(file_idx);
        let filter = index_file_to_bloom(&source);
        let trigrams = extract_unique_trigrams_from_bytes(source.as_bytes());

        let reported_bit_count = filter.bit_count();

        assert!(
            reported_bit_count <= BLOOM_BITS,
            "File {}: bit_count {} exceeds BLOOM_BITS {}",
            file_idx,
            reported_bit_count,
            BLOOM_BITS
        );

        assert!(
            reported_bit_count >= 1,
            "File {}: bit_count {} is less than 1",
            file_idx,
            reported_bit_count
        );

        let max_possible_bits = 2 * trigrams.len();
        assert!(
            reported_bit_count <= max_possible_bits,
            "File {}: bit_count {} exceeds 2 * trigram_count ({} * 2 = {})",
            file_idx,
            reported_bit_count,
            trigrams.len(),
            max_possible_bits
        );
    }
}

#[test]
fn test_serialization_preserves_bloom_properties() {
    for file_idx in 0..50 {
        let source = generate_fixture_file(file_idx);
        let original_filter = index_file_to_bloom(&source);

        let bytes = original_filter.to_bytes();
        let reconstructed_filter = BloomFilter::from_bytes(bytes);

        let original_fpr = original_filter.false_positive_estimate();
        let reconstructed_fpr = reconstructed_filter.false_positive_estimate();

        let fpr_diff = (original_fpr - reconstructed_fpr).abs();
        assert!(fpr_diff < 0.001, "File {}: FPR diff {:.6} exceeds threshold", file_idx, fpr_diff);

        let original_bit_count = original_filter.bit_count();
        let reconstructed_bit_count = reconstructed_filter.bit_count();

        assert_eq!(
            original_bit_count, reconstructed_bit_count,
            "File {}: bit_count mismatch {} vs {}",
            file_idx, original_bit_count, reconstructed_bit_count
        );

        let probe_trigrams = generate_synthetic_absent_trigrams(100);
        for trigram in &probe_trigrams {
            let original_result = original_filter.probably_contains(trigram);
            let reconstructed_result = reconstructed_filter.probably_contains(trigram);

            assert_eq!(
                original_result, reconstructed_result,
                "File {}: Deserialized filter gives different result for trigram {:?}",
                file_idx, trigram
            );
        }
    }
}

#[test]
fn test_print_bloom_statistics_report() {
    let mut all_trigram_counts = Vec::new();
    let mut all_unique_trigram_counts = Vec::new();
    let mut all_bit_counts = Vec::new();
    let mut all_fprs = Vec::new();
    let mut all_empirical_fprs = Vec::new();
    let mut zero_false_negative_violations = 0;

    for file_idx in 0..1000 {
        let source = generate_fixture_file(file_idx);
        let filter = index_file_to_bloom(&source);
        let trigrams = extract_unique_trigrams_from_bytes(source.as_bytes());

        all_trigram_counts.push(trigrams.len());

        let unique_trigrams: HashSet<_> = trigrams.iter().collect();
        all_unique_trigram_counts.push(unique_trigrams.len());

        all_bit_counts.push(filter.bit_count());

        let estimated_fpr = filter.false_positive_estimate();
        all_fprs.push(estimated_fpr);

        let probe_trigrams = generate_synthetic_absent_trigrams(100);
        let empirical_fpr = measure_false_positive_rate(&filter, &probe_trigrams);
        all_empirical_fprs.push(empirical_fpr);

        for trigram in &trigrams {
            if !filter.probably_contains(trigram) {
                zero_false_negative_violations += 1;
            }
        }
    }

    let total_trigrams: usize = all_trigram_counts.iter().sum();
    let mean_trigrams_per_file = total_trigrams as f64 / 1000.0;
    let mean_unique_per_file: f64 = all_unique_trigram_counts.iter().sum::<usize>() as f64 / 1000.0;
    let mean_bit_saturation: f64 = all_bit_counts.iter().sum::<usize>() as f64 / 1000.0;
    let mean_estimated_fpr: f64 = all_fprs.iter().sum::<f64>() / 1000.0;
    let mean_empirical_fpr: f64 = all_empirical_fprs.iter().sum::<f64>() / 1000.0;
    let max_empirical_fpr: f64 =
        all_empirical_fprs.iter().copied().fold(f64::NEG_INFINITY, f64::max);

    let bit_saturation_pct = (mean_bit_saturation / BLOOM_BITS as f64) * 100.0;

    println!();
    println!("=== Bloom Filter Statistics (1000 fixture files) ===");
    println!("total trigrams inserted:     {}", total_trigrams);
    println!("mean trigrams per file:      {:.1}", mean_trigrams_per_file);
    println!("mean unique trigrams/file:   {:.1}", mean_unique_per_file);
    println!(
        "mean bit saturation:         {:.0} / {} bits ({:.1}%)",
        mean_bit_saturation, BLOOM_BITS, bit_saturation_pct
    );
    println!("mean estimated FPR:          {:.2}%", mean_estimated_fpr * 100.0);
    println!("empirical mean FPR:          {:.2}%", mean_empirical_fpr * 100.0);
    println!("empirical max FPR:           {:.2}%", max_empirical_fpr * 100.0);
    println!("false negative violations:   {}", zero_false_negative_violations);
    println!();
}
