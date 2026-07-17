//! Reporter unit tests, including the golden tests for the measurement math
//! (audit V9) that pin percentile/Tukey/bootstrap/Cohen's-d values.

use super::assembly::{build_report, load_report, report_from_run, summarise_cases};
use super::html::{format_bytes, format_duration_ms};
use super::stats::{
    bootstrap_median_interval, cohens_d, median_from_sorted, percentile_from_sorted,
    quality_tier_for_cv, summarise_metric, REPORT_CONFIDENCE_LEVEL,
};
use super::{export_bundle, generate_html, generate_json};
use crate::types::*;
use chrono::Utc;
use uuid::Uuid;

fn sample_environment() -> BenchmarkEnvironmentFingerprint {
    BenchmarkEnvironmentFingerprint {
        client_os: Some("macos".into()),
        client_arch: Some("aarch64".into()),
        client_cpu_cores: Some(12),
        client_region: Some("us-east".into()),
        server_os: Some("ubuntu".into()),
        server_arch: Some("x86_64".into()),
        server_cpu_cores: Some(4),
        server_region: Some("eastus".into()),
        network_type: Some("LAN".into()),
        baseline_rtt_p50_ms: Some(0.9),
        baseline_rtt_p95_ms: Some(1.4),
    }
}

fn sample_result(
    lang: &str,
    runtime: &str,
    concurrency: u32,
    repeat_index: u32,
    rps: f64,
    scenario: &str,
) -> BenchmarkResult {
    BenchmarkResult {
        language: lang.into(),
        runtime: runtime.into(),
        concurrency,
        repeat_index,
        scenario: scenario.into(),
        environment: sample_environment(),
        network: NetworkMetrics {
            rps,
            latency_mean_ms: 1.0 / rps * 1000.0,
            latency_p50_ms: 0.8 / rps * 1000.0,
            latency_p99_ms: 3.0 / rps * 1000.0,
            latency_p999_ms: 5.0 / rps * 1000.0,
            latency_max_ms: 10.0 / rps * 1000.0,
            bytes_transferred: 1_000_000,
            error_count: 0,
            total_requests: if scenario == "cold" { 50 } else { 10_000 },
            phase_model: "stability-check->overhead->pilot->measured->cooldown".into(),
            phases_present: vec![
                "stability-check".into(),
                "overhead".into(),
                "pilot".into(),
                "measured".into(),
                "cooldown".into(),
            ],
        },
        resources: ResourceMetrics {
            peak_rss_bytes: 50_000_000,
            avg_cpu_fraction: 0.45,
            peak_cpu_fraction: 0.92,
            peak_open_fds: 128,
        },
        startup: StartupMetrics {
            time_to_first_response_ms: if scenario == "cold" { 120.0 } else { 2.0 },
            time_to_ready_ms: if scenario == "cold" { 200.0 } else { 5.0 },
        },
        binary: BinaryMetrics {
            size_bytes: 8_000_000,
            compressed_size_bytes: 3_000_000,
            docker_image_bytes: None,
        },
    }
}

fn sample_run() -> BenchmarkRun {
    let mut run = BenchmarkRun {
        id: Uuid::new_v4(),
        started_at: Utc::now() - chrono::Duration::minutes(30),
        finished_at: Some(Utc::now()),
        config_path: "benchmarks/config.json".into(),
        case_randomization_enabled: true,
        case_randomization_seed: Some(42),
        auto_rerun_policy: Some(BenchmarkAutoRerunPolicy {
            target_repeat_count: 3,
            max_additional_repeats: 2,
            max_relative_margin_of_error: 0.05,
        }),
        scheduled_reruns: vec![BenchmarkScheduledRerun {
            language: "go".into(),
            runtime: "gin".into(),
            concurrency: 10,
            repeat_index: 1,
            reasons: vec!["warm repeat count 1 below target 3".into()],
        }],
        baseline: Some(BenchmarkBaseline {
            language: "rust".into(),
            runtime: Some("axum".into()),
        }),
        results: vec![
            sample_result("rust", "axum", 10, 0, 85000.0, "cold"),
            sample_result("rust", "axum", 10, 0, 120000.0, "warm"),
            sample_result("go", "gin", 10, 0, 60000.0, "cold"),
            sample_result("go", "gin", 10, 0, 95000.0, "warm"),
            sample_result("python", "fastapi", 10, 0, 8000.0, "cold"),
            sample_result("python", "fastapi", 10, 0, 12000.0, "warm"),
            sample_result("node", "express", 10, 0, 25000.0, "cold"),
            sample_result("node", "express", 10, 0, 45000.0, "warm"),
        ],
    };
    // Give different memory footprints
    run.results[1].resources.peak_rss_bytes = 20_000_000;
    run.results[3].resources.peak_rss_bytes = 35_000_000;
    run.results[5].resources.peak_rss_bytes = 120_000_000;
    run.results[7].resources.peak_rss_bytes = 80_000_000;
    run
}

#[test]
fn test_generate_html_creates_file() {
    let run = sample_run();
    let dir = std::env::temp_dir();
    let path = dir.join("alethabench-test-report.html");
    generate_html(&run, &path).unwrap();
    assert!(path.exists());
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("Networker Bench Report"));
    assert!(content.contains("rust"));
    assert!(content.contains("go"));
    assert!(content.contains("python"));
    assert!(content.contains("node"));
    std::fs::remove_file(&path).ok();
}

#[test]
fn test_html_contains_all_sections() {
    let run = sample_run();
    let dir = std::env::temp_dir();
    let path = dir.join("alethabench-test-sections.html");
    generate_html(&run, &path).unwrap();
    let content = std::fs::read_to_string(&path).unwrap();

    // Header
    assert!(content.contains("Networker Bench Report"));
    assert!(content.contains(&run.id.to_string()));

    // Executive summary
    assert!(content.contains("Executive Summary"));
    assert!(content.contains("Highest Throughput"));
    assert!(content.contains("Lowest p99 Latency"));
    assert!(content.contains("Lowest Memory"));

    // Leaderboard
    assert!(content.contains("Case Summary"));
    assert!(content.contains("Warm req/s"));
    assert!(content.contains("Warm CV"));
    assert!(content.contains("Baseline Comparison"));
    assert!(content.contains("Req/s Ratio 95% CI"));

    // Charts (SVG)
    assert!(content.contains("Cold vs Warm Throughput"));
    assert!(content.contains("Latency Distribution"));
    assert!(content.contains("Resource Usage"));
    assert!(content.contains("<svg"));

    // Methodology
    assert!(content.contains("Methodology"));
    assert!(content.contains("config.json"));
    assert!(content.contains("Case order"));
    assert!(content.contains("randomized (seed 42)"));
    assert!(content.contains("Auto reruns"));
    assert!(content.contains("Executed reruns"));
    assert!(content.contains("Outlier policy"));
    assert!(content.contains("Comparison gating"));
    assert!(content.contains("Observed phase models"));
    assert!(content.contains("Observed lifecycle phases"));
    assert!(content.contains("Publication readiness"));
    assert!(content.contains("Recommendations"));
    assert!(content.contains("stability-check"));
    assert!(content.contains("cooldown"));
    assert!(content.contains("environment-check"));
    assert!(content.contains("Tukey 1.5xIQR"));
    assert!(content.contains("deterministic resampling"));

    // Inline JS
    assert!(content.contains("sort-asc"));

    // Dark theme
    assert!(content.contains("#0a0b0f"));
    assert!(content.contains("#47bfff"));

    std::fs::remove_file(&path).ok();
}

#[test]
fn test_html_leaderboard_sorted_by_rps() {
    let run = sample_run();
    let dir = std::env::temp_dir();
    let path = dir.join("alethabench-test-sorted.html");
    generate_html(&run, &path).unwrap();
    let content = std::fs::read_to_string(&path).unwrap();

    // Rust (120k) should appear before Go (95k) in the table
    let rust_pos = content.find(">rust<").unwrap_or(content.len());
    let go_pos = content.find(">go<").unwrap_or(content.len());
    assert!(
        rust_pos < go_pos,
        "rust should appear before go in leaderboard"
    );

    std::fs::remove_file(&path).ok();
}

#[test]
fn test_html_with_empty_results() {
    let run = BenchmarkRun {
        id: Uuid::new_v4(),
        started_at: Utc::now(),
        finished_at: Some(Utc::now()),
        config_path: "empty.json".into(),
        case_randomization_enabled: false,
        case_randomization_seed: None,
        auto_rerun_policy: None,
        scheduled_reruns: vec![],
        baseline: None,
        results: vec![],
    };
    let dir = std::env::temp_dir();
    let path = dir.join("alethabench-test-empty.html");
    generate_html(&run, &path).unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("Networker Bench Report"));
    assert!(content.contains("0 benchmark cases"));
    std::fs::remove_file(&path).ok();
}

#[test]
fn test_generate_json_roundtrip() {
    let run = sample_run();
    let dir = std::env::temp_dir();
    let path = dir.join("alethabench-test-json.json");
    generate_json(&run, &path).unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    let loaded: BenchmarkReport = serde_json::from_str(&content).unwrap();
    assert_eq!(loaded.format_version, "1.1");
    assert_eq!(loaded.run.id, run.id);
    assert_eq!(loaded.run.results.len(), run.results.len());
    assert_eq!(loaded.aggregation.case_summaries.len(), 4);
    assert_eq!(loaded.aggregation.comparisons.len(), 3);
    assert_eq!(loaded.aggregation.confidence_level, REPORT_CONFIDENCE_LEVEL);
    std::fs::remove_file(&path).ok();
}

#[test]
fn test_load_report_roundtrip_preserves_generated_metadata() {
    let run = sample_run();
    let report = report_from_run(&run);
    let dir = std::env::temp_dir();
    let path = dir.join("alethabench-test-load-report.json");
    generate_json(&run, &path).unwrap();

    let loaded = load_report(&path).unwrap();
    assert_eq!(loaded.format_version, report.format_version);
    assert_eq!(loaded.run.id, report.run.id);
    assert_eq!(
        loaded.run.case_randomization_enabled,
        report.run.case_randomization_enabled
    );
    assert_eq!(
        loaded.run.case_randomization_seed,
        report.run.case_randomization_seed
    );
    assert_eq!(
        loaded.run.scheduled_reruns.len(),
        report.run.scheduled_reruns.len()
    );
    assert_eq!(
        loaded.aggregation.case_summaries.len(),
        report.aggregation.case_summaries.len()
    );
    assert_eq!(
        loaded.aggregation.comparisons.len(),
        report.aggregation.comparisons.len()
    );
    assert_eq!(
        loaded.aggregation.publication_ready,
        report.aggregation.publication_ready
    );
    assert_eq!(
        loaded.aggregation.rerun_recommended,
        report.aggregation.rerun_recommended
    );
    std::fs::remove_file(&path).ok();
}

#[test]
fn test_export_bundle_writes_publication_package() {
    let run = sample_run();
    let report = report_from_run(&run);
    let dir = std::env::temp_dir().join(format!("alethabench-export-{}", report.run.id));
    export_bundle(&report, &dir).unwrap();

    let json_path = dir.join("benchmark-report.json");
    let html_path = dir.join("benchmark-report.html");
    let md_path = dir.join("benchmark-report.md");
    let csv_path = dir.join("benchmark-results.csv");
    let manifest_path = dir.join("manifest.json");

    for path in [&json_path, &html_path, &md_path, &csv_path, &manifest_path] {
        assert!(path.exists(), "{} should exist", path.display());
    }

    let json = std::fs::read_to_string(&json_path).unwrap();
    let loaded: BenchmarkReport = serde_json::from_str(&json).unwrap();
    assert_eq!(loaded.run.id, report.run.id);
    assert_eq!(
        loaded.aggregation.publication_ready,
        report.aggregation.publication_ready
    );
    assert_eq!(
        loaded.aggregation.recommendations.len(),
        report.aggregation.recommendations.len()
    );

    let markdown = std::fs::read_to_string(&md_path).unwrap();
    assert!(markdown.contains("Publication Bundle"));
    assert!(markdown.contains("Methodology"));
    assert!(markdown.contains("Publication Blockers"));
    assert!(markdown.contains("Case Summaries"));

    let csv = std::fs::read_to_string(&csv_path).unwrap();
    assert!(csv.starts_with("language,runtime,concurrency"));
    assert!(csv.contains("rust,axum"));

    let manifest = std::fs::read_to_string(&manifest_path).unwrap();
    assert!(manifest.contains("benchmark-report.json"));
    assert!(manifest.contains("benchmark-results.csv"));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn test_format_bytes() {
    assert_eq!(format_bytes(500), "500 B");
    assert_eq!(format_bytes(2048), "2.0 KB");
    assert_eq!(format_bytes(5_242_880), "5.0 MB");
    assert_eq!(format_bytes(2_147_483_648), "2.0 GB");
}

#[test]
fn test_format_duration_ms() {
    assert_eq!(format_duration_ms(0.5), "0.5ms");
    assert_eq!(format_duration_ms(150.0), "150.0ms");
    assert_eq!(format_duration_ms(2500.0), "2.50s");
}

#[test]
fn test_summarise_metric_reports_fences_and_outliers() {
    let summary = summarise_metric([100.0, 101.0, 102.0, 200.0]);

    assert_eq!(summary.sample_count, 4);
    assert_eq!(summary.outlier_count, 1);
    assert_eq!(summary.high_outlier_count, 1);
    assert_eq!(summary.low_outlier_count, 0);
    assert!(summary.upper_fence < 200.0);
    assert!(summary.iqr > 0.0);
    assert!(summary.mad > 0.0);
    assert!(summary.ci95_upper >= summary.ci95_lower);
    assert!(summary.relative_margin_of_error >= 0.0);
    assert_eq!(summary.quality_tier, "unreliable");
}

// ── Golden tests for the measurement math (audit V9) ────────────────────
// These pin known-input → known-output values for the percentile/summary
// functions every published number flows through. Exactness matters: a
// silent change to interpolation or fence math changes rankings.

#[test]
fn test_golden_percentile_from_sorted_linear_interpolation() {
    let sorted: Vec<f64> = (1..=10).map(|v| v as f64).collect();

    // rank = p/100 * (n-1), linear interpolation between neighbours.
    assert_eq!(percentile_from_sorted(&sorted, 0.0), 1.0);
    assert_eq!(percentile_from_sorted(&sorted, 100.0), 10.0);
    assert!((percentile_from_sorted(&sorted, 25.0) - 3.25).abs() < 1e-12);
    assert!((percentile_from_sorted(&sorted, 50.0) - 5.5).abs() < 1e-12);
    assert!((percentile_from_sorted(&sorted, 95.0) - 9.55).abs() < 1e-12);
    assert!((percentile_from_sorted(&sorted, 99.0) - 9.91).abs() < 1e-12);

    // Degenerate inputs.
    assert_eq!(percentile_from_sorted(&[], 50.0), 0.0);
    assert_eq!(percentile_from_sorted(&[42.0], 99.0), 42.0);
}

#[test]
fn test_golden_median_from_sorted() {
    assert_eq!(median_from_sorted(&[]), 0.0);
    assert_eq!(median_from_sorted(&[7.0]), 7.0);
    assert_eq!(median_from_sorted(&[1.0, 3.0]), 2.0);
    assert_eq!(median_from_sorted(&[1.0, 3.0, 5.0]), 3.0);
    assert_eq!(median_from_sorted(&[1.0, 2.0, 3.0, 4.0]), 2.5);
}

#[test]
fn test_golden_summarise_metric_known_values() {
    // sorted: [10, 11, 12, 13, 100] — one high outlier.
    let summary = summarise_metric([10.0, 12.0, 11.0, 13.0, 100.0]);

    assert_eq!(summary.sample_count, 5);
    assert_eq!(summary.min, 10.0);
    assert_eq!(summary.max, 100.0);
    assert!((summary.mean - 29.2).abs() < 1e-9);
    assert_eq!(summary.median, 12.0);
    // Sample variance (n-1 denominator): 6270.8 / 4.
    assert!((summary.variance - 1567.7).abs() < 1e-9);
    assert!((summary.stddev - 1567.7_f64.sqrt()).abs() < 1e-9);
    assert!((summary.cv - 1567.7_f64.sqrt() / 29.2).abs() < 1e-9);
    // p25 = rank 1.0 → 11; p75 = rank 3.0 → 13; Tukey 1.5×IQR fences.
    assert!((summary.iqr - 2.0).abs() < 1e-12);
    assert!((summary.lower_fence - 8.0).abs() < 1e-12);
    assert!((summary.upper_fence - 16.0).abs() < 1e-12);
    assert_eq!(summary.low_outlier_count, 0);
    assert_eq!(summary.high_outlier_count, 1);
    assert_eq!(summary.outlier_count, 1);
    // |x - median| = [2, 1, 0, 1, 88] → sorted median 1.
    assert!((summary.mad - 1.0).abs() < 1e-12);
    assert_eq!(summary.quality_tier, "unreliable");
}

#[test]
fn test_golden_summarise_metric_filters_non_finite() {
    let summary = summarise_metric([1.0, f64::NAN, 3.0, f64::INFINITY]);
    assert_eq!(summary.sample_count, 2);
    assert_eq!(summary.min, 1.0);
    assert_eq!(summary.max, 3.0);
    assert!((summary.mean - 2.0).abs() < 1e-12);
    assert_eq!(summary.median, 2.0);
}

#[test]
fn test_golden_bootstrap_median_interval() {
    // Degenerate inputs have exact, spec'd outputs.
    assert_eq!(bootstrap_median_interval(&[]), (0.0, 0.0, 0.0));
    assert_eq!(bootstrap_median_interval(&[7.0]), (0.0, 7.0, 7.0));

    // Constant samples: every resampled median is the constant, so the
    // interval collapses exactly (se=0, lower=upper=value).
    let (se, lower, upper) = bootstrap_median_interval(&[5.0, 5.0, 5.0, 5.0]);
    assert_eq!(se, 0.0);
    assert_eq!(lower, 5.0);
    assert_eq!(upper, 5.0);

    // The RNG is seeded from the input values — the same input must
    // produce bit-identical intervals on every run and platform.
    let values = [12.0, 15.0, 11.0, 14.0, 13.0, 16.0];
    let first = bootstrap_median_interval(&values);
    let second = bootstrap_median_interval(&values);
    assert_eq!(first, second);
    let (se, lower, upper) = first;
    assert!(se > 0.0);
    assert!(lower <= upper);
    // The CI must bracket the sample median (13.5) and stay within range.
    assert!(lower <= 13.5 && 13.5 <= upper);
    assert!(lower >= 11.0 && upper <= 16.0);
}

#[test]
fn test_golden_quality_tier_boundaries() {
    assert_eq!(quality_tier_for_cv(0.0), "excellent");
    assert_eq!(quality_tier_for_cv(0.03), "excellent");
    assert_eq!(quality_tier_for_cv(0.031), "good");
    assert_eq!(quality_tier_for_cv(0.08), "good");
    assert_eq!(quality_tier_for_cv(0.081), "fair");
    assert_eq!(quality_tier_for_cv(0.15), "fair");
    assert_eq!(quality_tier_for_cv(0.151), "unreliable");
    assert_eq!(quality_tier_for_cv(f64::NAN), "unknown");
    assert_eq!(quality_tier_for_cv(f64::INFINITY), "unknown");
}

#[test]
fn test_golden_cohens_d() {
    // means 4 vs 3, both variances 4 → pooled variance 4 → d = 1/2.
    let d = cohens_d(&[2.0, 4.0, 6.0], &[1.0, 3.0, 5.0]);
    assert!((d - 0.5).abs() < 1e-12);

    // Identical samples → zero effect.
    assert_eq!(cohens_d(&[3.0, 3.0, 3.0], &[3.0, 3.0, 3.0]), 0.0);
    // Fewer than 2 samples on either side → defined as 0.
    assert_eq!(cohens_d(&[1.0], &[1.0, 2.0]), 0.0);
}

#[test]
fn test_summarise_cases_orders_by_warm_median_rps() {
    let run = sample_run();
    let summaries = summarise_cases(&run);
    assert_eq!(summaries.len(), 4);
    // Should be sorted by warm median RPS descending
    assert_eq!(summaries[0].language, "rust");
    assert_eq!(summaries[1].language, "go");
    assert_eq!(summaries[2].language, "node");
    assert_eq!(summaries[3].language, "python");
}

#[test]
fn test_summarise_cases_aggregates_repeats_without_cherry_picking() {
    let run = BenchmarkRun {
        id: Uuid::new_v4(),
        started_at: Utc::now(),
        finished_at: Some(Utc::now()),
        config_path: "repeated.json".into(),
        case_randomization_enabled: false,
        case_randomization_seed: None,
        auto_rerun_policy: None,
        scheduled_reruns: vec![],
        baseline: Some(BenchmarkBaseline {
            language: "rust".into(),
            runtime: Some("axum".into()),
        }),
        results: vec![
            sample_result("rust", "axum", 32, 0, 90_000.0, "warm"),
            sample_result("rust", "axum", 32, 1, 110_000.0, "warm"),
            sample_result("rust", "axum", 32, 0, 65_000.0, "cold"),
            sample_result("rust", "axum", 32, 1, 75_000.0, "cold"),
        ],
    };

    let summaries = summarise_cases(&run);
    assert_eq!(summaries.len(), 1);

    let summary = &summaries[0];
    let warm = summary.warm.as_ref().unwrap();
    let cold = summary.cold.as_ref().unwrap();

    assert_eq!(warm.repeat_count, 2);
    assert_eq!(warm.repeat_indices, vec![0, 1]);
    assert_eq!(warm.rps.max, 110_000.0);
    assert_eq!(warm.rps.median, 100_000.0);
    assert!(warm.rps.mean < warm.rps.max);

    assert_eq!(cold.repeat_count, 2);
    assert_eq!(cold.rps.median, 70_000.0);
}

#[test]
fn test_build_report_generates_baseline_comparisons() {
    let report = build_report(&sample_run());
    assert_eq!(report.aggregation.comparisons.len(), 3);

    let go = report
        .aggregation
        .comparisons
        .iter()
        .find(|comparison| comparison.language == "go")
        .unwrap();
    let warm = go.warm.as_ref().unwrap();

    assert_eq!(go.baseline_case_label, "rust c=10");
    assert_eq!(warm.shared_repeat_count, 1);
    assert!(warm.throughput.ratio < 1.0);
    assert_eq!(warm.throughput.verdict, "slower");
    assert!(go.comparable);
    assert!(go.comparability_notes.is_empty());
    assert!(warm.throughput.ratio_summary.ci95_upper >= warm.throughput.ratio_summary.ci95_lower);
    assert!(warm.throughput.candidate_summary.relative_margin_of_error >= 0.0);
}

#[test]
fn test_build_report_gates_baseline_comparisons_when_environments_differ() {
    let mut run = sample_run();
    for result in run
        .results
        .iter_mut()
        .filter(|result| result.language == "go")
    {
        result.environment.server_region = Some("westus".into());
        result.environment.baseline_rtt_p50_ms = Some(2.1);
        result.environment.baseline_rtt_p95_ms = Some(3.1);
    }

    let report = build_report(&run);
    let go = report
        .aggregation
        .comparisons
        .iter()
        .find(|comparison| comparison.language == "go")
        .unwrap();

    assert!(!go.comparable);
    assert!(go.warm.is_none());
    assert!(go.cold.is_none());
    assert!(go
        .comparability_notes
        .iter()
        .any(|note| note.contains("server region")));
    assert!(go
        .comparability_notes
        .iter()
        .any(|note| note.contains("baseline RTT p50")));
    assert!(report
        .aggregation
        .recommendations
        .iter()
        .any(|recommendation| recommendation
            .contains("Baseline comparisons are not publication-grade")));
}

#[test]
fn test_build_report_marks_rerun_recommended_when_publication_checks_fail() {
    let report = build_report(&sample_run());

    assert!(!report.aggregation.publication_ready);
    assert!(report.aggregation.rerun_recommended);
    assert!(report
        .aggregation
        .recommendations
        .iter()
        .any(|recommendation| recommendation.contains("environment-check")));
    assert!(report
        .aggregation
        .recommendations
        .iter()
        .any(|recommendation| recommendation.contains("Repeat count")));
}

#[test]
fn test_html_is_standalone() {
    let run = sample_run();
    let dir = std::env::temp_dir();
    let path = dir.join("alethabench-test-standalone.html");
    generate_html(&run, &path).unwrap();
    let content = std::fs::read_to_string(&path).unwrap();

    // No external references
    assert!(!content.contains("href=\"http"));
    assert!(!content.contains("src=\"http"));
    assert!(!content.contains("<link rel=\"stylesheet\""));

    // Has inline style and script
    assert!(content.contains("<style>"));
    assert!(content.contains("<script>"));

    std::fs::remove_file(&path).ok();
}
