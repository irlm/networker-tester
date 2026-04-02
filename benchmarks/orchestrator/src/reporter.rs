use crate::types::{
    BenchmarkBaseline, BenchmarkCaseSummary, BenchmarkComparison, BenchmarkEnvironmentFingerprint,
    BenchmarkReport, BenchmarkResult, BenchmarkRun, ComparisonMetricSummary, MetricSummary,
    ReportAggregation, ScenarioComparison, ScenarioSummary,
};
use anyhow::{Context, Result};
use chrono::Utc;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt::Write as FmtWrite;
use std::path::Path;

/// Write benchmark results as JSON.
pub fn generate_json(run: &BenchmarkRun, output: &Path) -> Result<()> {
    let report = build_report(run);
    let json = serde_json::to_string_pretty(&report)?;
    std::fs::write(output, json)?;
    tracing::info!("Wrote JSON report to {}", output.display());
    Ok(())
}

/// Write a publication bundle containing JSON, HTML, Markdown, CSV, and a manifest.
pub fn export_bundle(report: &BenchmarkReport, output_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("creating export dir {}", output_dir.display()))?;

    let json_path = output_dir.join("benchmark-report.json");
    let html_path = output_dir.join("benchmark-report.html");
    let md_path = output_dir.join("benchmark-report.md");
    let csv_path = output_dir.join("benchmark-results.csv");
    let manifest_path = output_dir.join("manifest.json");

    std::fs::write(&json_path, serde_json::to_string_pretty(&report)?)
        .with_context(|| format!("writing {}", json_path.display()))?;
    generate_html(&report.run, &html_path)?;
    std::fs::write(&md_path, render_markdown_report(&report))
        .with_context(|| format!("writing {}", md_path.display()))?;
    std::fs::write(&csv_path, render_results_csv(&report))
        .with_context(|| format!("writing {}", csv_path.display()))?;

    let manifest = serde_json::json!({
        "format_version": report.format_version,
        "generated_at": report.generated_at,
        "run_id": report.run.id,
        "files": [
            {"name": "benchmark-report.json", "kind": "json", "preserves": ["methodology", "comparability", "publication_readiness"]},
            {"name": "benchmark-report.html", "kind": "html", "preserves": ["methodology", "comparability", "publication_readiness"]},
            {"name": "benchmark-report.md", "kind": "markdown", "preserves": ["methodology", "comparability", "publication_readiness"]},
            {"name": "benchmark-results.csv", "kind": "csv", "preserves": ["raw_results", "scenario", "phase", "environment"]},
        ],
    });
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)
        .with_context(|| format!("writing {}", manifest_path.display()))?;

    tracing::info!("Wrote benchmark export bundle to {}", output_dir.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct CaseKey {
    language: String,
    runtime: String,
    concurrency: u32,
}

const REPORT_CONFIDENCE_LEVEL: f64 = 0.95;
const TUKEY_FENCE_MULTIPLIER: f64 = 1.5;
const BOOTSTRAP_RESAMPLES: usize = 2_048;
const MAX_COMPARABLE_RTT_RATIO: f64 = 1.5;

#[derive(Debug, Clone)]
struct DeterministicRng {
    state: u64,
}

impl DeterministicRng {
    fn from_values(values: &[f64]) -> Self {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64 ^ values.len() as u64;
        for value in values {
            state ^= value.to_bits().wrapping_mul(0xbf58_476d_1ce4_e5b9);
            state = state.rotate_left(13);
        }
        if state == 0 {
            state = 0x94d0_49bb_1331_11eb;
        }
        Self { state }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }

    fn next_index(&mut self, upper: usize) -> usize {
        (self.next_u64() as usize) % upper
    }
}

fn build_report(run: &BenchmarkRun) -> BenchmarkReport {
    let grouped = group_case_results(run);
    let case_summaries = summarise_case_groups(&grouped);
    let comparisons = run
        .baseline
        .as_ref()
        .map(|baseline| build_comparisons(&grouped, &case_summaries, baseline))
        .unwrap_or_default();
    let recommendations = build_recommendations(run, &case_summaries, &comparisons);
    let publication_ready = recommendations.is_empty();

    BenchmarkReport {
        format_version: "1.1".to_string(),
        generated_at: Utc::now(),
        run: run.clone(),
        aggregation: ReportAggregation {
            grouped_by: vec![
                "language".to_string(),
                "runtime".to_string(),
                "concurrency".to_string(),
                "scenario".to_string(),
            ],
            ranking_metric: "warm req/s".to_string(),
            primary_estimator: "median".to_string(),
            confidence_level: REPORT_CONFIDENCE_LEVEL,
            outlier_policy:
                "Tukey 1.5xIQR fences flag outliers for audit while repeated-launch summaries keep all launches and avoid cherry-picking."
                    .to_string(),
            uncertainty_method:
                "Percentile bootstrap confidence intervals around the median with deterministic resampling."
                    .to_string(),
            anti_cherry_picking_policy:
                "Case summaries aggregate repeated launches by scenario; raw results are retained and no single best run is used for ranking."
                    .to_string(),
            publication_ready,
            rerun_recommended: !publication_ready,
            recommendations,
            case_summaries,
            comparisons,
        },
    }
}

fn escape_csv(value: &str) -> String {
    let needs_quotes = value.contains([',', '"', '\n', '\r']);
    if needs_quotes {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn render_results_csv(report: &BenchmarkReport) -> String {
    let mut csv = String::new();
    csv.push_str(
        "language,runtime,concurrency,repeat_index,scenario,rps,latency_mean_ms,latency_p50_ms,latency_p99_ms,latency_p999_ms,latency_max_ms,bytes_transferred,error_count,total_requests,phase_model,phases_present,client_os,client_arch,client_cpu_cores,client_region,server_os,server_arch,server_cpu_cores,server_region,network_type,baseline_rtt_p50_ms,baseline_rtt_p95_ms\n",
    );

    for result in &report.run.results {
        let phases = if result.network.phases_present.is_empty() {
            String::new()
        } else {
            result.network.phases_present.join("|")
        };
        let env = &result.environment;
        let row = [
            escape_csv(&result.language),
            escape_csv(&result.runtime),
            result.concurrency.to_string(),
            result.repeat_index.to_string(),
            escape_csv(&result.scenario),
            result.network.rps.to_string(),
            result.network.latency_mean_ms.to_string(),
            result.network.latency_p50_ms.to_string(),
            result.network.latency_p99_ms.to_string(),
            result.network.latency_p999_ms.to_string(),
            result.network.latency_max_ms.to_string(),
            result.network.bytes_transferred.to_string(),
            result.network.error_count.to_string(),
            result.network.total_requests.to_string(),
            escape_csv(&result.network.phase_model),
            escape_csv(&phases),
            escape_csv(env.client_os.as_deref().unwrap_or("")),
            escape_csv(env.client_arch.as_deref().unwrap_or("")),
            env.client_cpu_cores
                .map(|value| value.to_string())
                .unwrap_or_default(),
            escape_csv(env.client_region.as_deref().unwrap_or("")),
            escape_csv(env.server_os.as_deref().unwrap_or("")),
            escape_csv(env.server_arch.as_deref().unwrap_or("")),
            env.server_cpu_cores
                .map(|value| value.to_string())
                .unwrap_or_default(),
            escape_csv(env.server_region.as_deref().unwrap_or("")),
            escape_csv(env.network_type.as_deref().unwrap_or("")),
            env.baseline_rtt_p50_ms
                .map(|value| value.to_string())
                .unwrap_or_default(),
            env.baseline_rtt_p95_ms
                .map(|value| value.to_string())
                .unwrap_or_default(),
        ]
        .join(",");
        csv.push_str(&row);
        csv.push('\n');
    }

    csv
}

fn render_markdown_report(report: &BenchmarkReport) -> String {
    let mut md = String::new();
    let run = &report.run;
    let aggregation = &report.aggregation;

    let _ = writeln!(md, "# AletheBench Publication Bundle");
    let _ = writeln!(md);
    let _ = writeln!(md, "- Run ID: `{}`", run.id);
    let _ = writeln!(md, "- Generated at: `{}`", report.generated_at);
    let _ = writeln!(md, "- Config: `{}`", run.config_path);
    let _ = writeln!(
        md,
        "- Case order: `{}`",
        if run.case_randomization_seed.is_some() {
            "randomized"
        } else {
            "config order"
        }
    );
    let _ = writeln!(
        md,
        "- Publication ready: `{}`",
        aggregation.publication_ready
    );
    let _ = writeln!(
        md,
        "- Rerun recommended: `{}`",
        aggregation.rerun_recommended
    );
    let _ = writeln!(md);

    let _ = writeln!(md, "## Methodology");
    let _ = writeln!(
        md,
        "- Confidence level: `{:.0}%`",
        aggregation.confidence_level * 100.0
    );
    let _ = writeln!(
        md,
        "- Primary estimator: `{}`",
        aggregation.primary_estimator
    );
    let _ = writeln!(md, "- Outlier policy: {}", aggregation.outlier_policy);
    let _ = writeln!(
        md,
        "- Uncertainty method: {}",
        aggregation.uncertainty_method
    );
    let _ = writeln!(
        md,
        "- Anti-cherry-picking policy: {}",
        aggregation.anti_cherry_picking_policy
    );
    let _ = writeln!(md);

    let _ = writeln!(md, "## Publication Blockers");
    if aggregation.recommendations.is_empty() {
        let _ = writeln!(md, "- None");
    } else {
        for recommendation in &aggregation.recommendations {
            let _ = writeln!(md, "- {}", recommendation);
        }
    }
    let _ = writeln!(md);

    let _ = writeln!(md, "## Environment");
    let fingerprints: Vec<&BenchmarkEnvironmentFingerprint> = run
        .results
        .iter()
        .map(|result| &result.environment)
        .collect();
    let env_summary = fingerprints
        .iter()
        .find(|fp| !fp.is_empty())
        .map(|fp| {
            format!(
                "client={:?} {:?}, server={:?} {:?}, network={:?}",
                fp.client_os, fp.client_arch, fp.server_os, fp.server_arch, fp.network_type
            )
        })
        .unwrap_or_else(|| "no environment fingerprint captured".to_string());
    let _ = writeln!(md, "- {}", env_summary);
    let _ = writeln!(md);

    let _ = writeln!(md, "## Case Summaries");
    for case in &aggregation.case_summaries {
        let _ = writeln!(md, "- `{}`", case.case_label);
        if let Some(warm) = &case.warm {
            let _ = writeln!(
                md,
                "  - warm median RPS: `{:.2}` with `{}` repeats",
                warm.rps.median, warm.repeat_count
            );
            let _ = writeln!(
                md,
                "  - warm p99 latency: `{:.2}ms`, CV: `{:.4}`, CI95: `[{:.2}, {:.2}]`",
                warm.latency_p99_ms.median, warm.rps.cv, warm.rps.ci95_lower, warm.rps.ci95_upper
            );
        }
        if let Some(cold) = &case.cold {
            let _ = writeln!(
                md,
                "  - cold median RPS: `{:.2}` with `{}` repeats",
                cold.rps.median, cold.repeat_count
            );
        }
    }

    if !aggregation.comparisons.is_empty() {
        let _ = writeln!(md);
        let _ = writeln!(md, "## Baseline Comparisons");
        for comparison in &aggregation.comparisons {
            let _ = writeln!(
                md,
                "- `{}` vs `{}`: comparable=`{}`",
                comparison.case_label, comparison.baseline_case_label, comparison.comparable
            );
            if !comparison.comparability_notes.is_empty() {
                for note in &comparison.comparability_notes {
                    let _ = writeln!(md, "  - {}", note);
                }
            }
            if let Some(warm) = &comparison.warm {
                let _ = writeln!(
                    md,
                    "  - warm throughput ratio: `{:.4}` verdict=`{}`",
                    warm.throughput.ratio, warm.throughput.verdict
                );
            }
            if let Some(cold) = &comparison.cold {
                let _ = writeln!(
                    md,
                    "  - cold throughput ratio: `{:.4}` verdict=`{}`",
                    cold.throughput.ratio, cold.throughput.verdict
                );
            }
        }
    }

    md
}

pub(crate) fn report_from_run(run: &BenchmarkRun) -> BenchmarkReport {
    build_report(run)
}

pub(crate) fn summarise_results(results: &[BenchmarkResult]) -> Vec<BenchmarkCaseSummary> {
    let mut grouped = BTreeMap::new();
    for result in results {
        grouped
            .entry(CaseKey {
                language: result.language.clone(),
                runtime: result.runtime.clone(),
                concurrency: result.concurrency,
            })
            .or_insert_with(Vec::new)
            .push(result);
    }
    summarise_case_groups(&grouped)
}

fn collect_observed_phases(run: &BenchmarkRun) -> Vec<String> {
    let mut observed_phases = Vec::new();
    for result in &run.results {
        for phase in &result.network.phases_present {
            if !observed_phases
                .iter()
                .any(|existing: &String| existing == phase)
            {
                observed_phases.push(phase.clone());
            }
        }
    }
    observed_phases
}

fn scenario_label(case: &BenchmarkCaseSummary, scenario: &str) -> String {
    format!("{} {} c={}", case.language, scenario, case.concurrency)
}

fn build_recommendations(
    run: &BenchmarkRun,
    case_summaries: &[BenchmarkCaseSummary],
    comparisons: &[BenchmarkComparison],
) -> Vec<String> {
    let mut recommendations = Vec::new();
    let observed_phases = collect_observed_phases(run);

    for phase in ["environment-check", "stability-check", "pilot", "measured"] {
        if !observed_phases.iter().any(|observed| observed == phase) {
            recommendations.push(format!(
                "Observed benchmark lifecycle is missing `{phase}`; rerun with the full benchmark preset before publication."
            ));
        }
    }
    if !observed_phases
        .iter()
        .any(|observed| observed == "cooldown")
    {
        recommendations.push(
            "Cooldown phase is missing from the observed lifecycle; collect post-measured cooldown samples for publication-grade runs."
                .to_string(),
        );
    }

    let mut unstable = Vec::new();
    let mut wide_ci = Vec::new();
    let mut low_repeats = Vec::new();
    for case in case_summaries {
        for scenario in [case.warm.as_ref(), case.cold.as_ref()]
            .into_iter()
            .flatten()
        {
            let label = scenario_label(case, &scenario.scenario);
            if scenario.repeat_count < 3 {
                low_repeats.push(label.clone());
            }
            if scenario.rps.quality_tier == "unreliable"
                || scenario.latency_p99_ms.quality_tier == "unreliable"
            {
                unstable.push(label.clone());
            }
            if scenario.rps.relative_margin_of_error > 0.05
                || scenario.latency_p99_ms.relative_margin_of_error > 0.05
            {
                wide_ci.push(label);
            }
        }
    }

    if !low_repeats.is_empty() {
        recommendations.push(format!(
            "Repeat count is below the default publication target (3) for: {}.",
            low_repeats.join(", ")
        ));
    }
    if !unstable.is_empty() {
        recommendations.push(format!(
            "Repeat variance remains unreliable for: {}.",
            unstable.join(", ")
        ));
    }
    if !wide_ci.is_empty() {
        recommendations.push(format!(
            "Confidence intervals remain wider than the default 5% publication target for: {}.",
            wide_ci.join(", ")
        ));
    }

    let not_comparable: Vec<String> = comparisons
        .iter()
        .filter(|comparison| !comparison.comparable)
        .map(|comparison| {
            format!(
                "{} vs {}",
                comparison.case_label, comparison.baseline_case_label
            )
        })
        .collect();
    if !not_comparable.is_empty() {
        recommendations.push(format!(
            "Baseline comparisons are not publication-grade because benchmark environments do not match closely enough for: {}.",
            not_comparable.join(", ")
        ));
    }

    recommendations
}

pub(crate) fn load_report(path: &Path) -> Result<BenchmarkReport> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;

    if let Ok(report) = serde_json::from_str::<BenchmarkReport>(&content) {
        return Ok(report);
    }

    let run: BenchmarkRun = serde_json::from_str(&content)
        .with_context(|| format!("parsing benchmark report {}", path.display()))?;
    Ok(build_report(&run))
}

fn result_scenario(result: &BenchmarkResult) -> &str {
    match result.scenario.as_str() {
        "" | "unknown" => {
            if result.network.total_requests <= 100 {
                "cold"
            } else {
                "warm"
            }
        }
        scenario => scenario,
    }
}

type GroupedCaseResults<'a> = BTreeMap<CaseKey, Vec<&'a BenchmarkResult>>;

fn group_case_results(run: &BenchmarkRun) -> GroupedCaseResults<'_> {
    let mut grouped = BTreeMap::new();

    for result in &run.results {
        grouped
            .entry(CaseKey {
                language: result.language.clone(),
                runtime: result.runtime.clone(),
                concurrency: result.concurrency,
            })
            .or_insert_with(Vec::new)
            .push(result);
    }

    grouped
}

fn percentile_from_sorted(sorted: &[f64], percentile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }

    let rank = percentile / 100.0 * (sorted.len() - 1) as f64;
    let lower_index = rank.floor() as usize;
    let upper_index = rank.ceil() as usize;
    if lower_index == upper_index {
        sorted[lower_index]
    } else {
        let lower = sorted[lower_index];
        let upper = sorted[upper_index];
        lower + (upper - lower) * (rank - lower_index as f64)
    }
}

fn median_from_sorted(sorted: &[f64]) -> f64 {
    if sorted.is_empty() {
        0.0
    } else if sorted.len() % 2 == 0 {
        let upper = sorted.len() / 2;
        (sorted[upper - 1] + sorted[upper]) / 2.0
    } else {
        sorted[sorted.len() / 2]
    }
}

fn quality_tier_for_cv(cv: f64) -> String {
    if !cv.is_finite() {
        "unknown".to_string()
    } else if cv <= 0.03 {
        "excellent".to_string()
    } else if cv <= 0.08 {
        "good".to_string()
    } else if cv <= 0.15 {
        "fair".to_string()
    } else {
        "unreliable".to_string()
    }
}

fn bootstrap_median_interval(values: &[f64]) -> (f64, f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    if values.len() == 1 {
        return (0.0, values[0], values[0]);
    }

    let mut rng = DeterministicRng::from_values(values);
    let mut estimates = Vec::with_capacity(BOOTSTRAP_RESAMPLES);
    for _ in 0..BOOTSTRAP_RESAMPLES {
        let mut sample = Vec::with_capacity(values.len());
        for _ in 0..values.len() {
            sample.push(values[rng.next_index(values.len())]);
        }
        sample.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
        estimates.push(median_from_sorted(&sample));
    }

    estimates.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let estimate_mean = estimates.iter().sum::<f64>() / estimates.len() as f64;
    let estimate_variance = estimates
        .iter()
        .map(|value| (value - estimate_mean).powi(2))
        .sum::<f64>()
        / (estimates.len() as f64 - 1.0);
    let standard_error = estimate_variance.sqrt();
    let tail = (1.0 - REPORT_CONFIDENCE_LEVEL) * 50.0;
    let lower = percentile_from_sorted(&estimates, tail);
    let upper = percentile_from_sorted(&estimates, 100.0 - tail);
    (standard_error, lower, upper)
}

fn summarise_metric<I>(values: I) -> MetricSummary
where
    I: IntoIterator<Item = f64>,
{
    let mut values: Vec<f64> = values
        .into_iter()
        .filter(|value| value.is_finite())
        .collect();
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));

    if values.is_empty() {
        return MetricSummary::default();
    }

    let sample_count = values.len() as u32;
    let min = values[0];
    let max = *values.last().unwrap_or(&min);
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let median = median_from_sorted(&values);
    let variance = if values.len() > 1 {
        values
            .iter()
            .map(|value| (value - mean).powi(2))
            .sum::<f64>()
            / (values.len() as f64 - 1.0)
    } else {
        0.0
    };
    let stddev = variance.sqrt();
    let cv = if mean.abs() > f64::EPSILON {
        stddev / mean.abs()
    } else {
        0.0
    };
    let p25 = percentile_from_sorted(&values, 25.0);
    let p75 = percentile_from_sorted(&values, 75.0);
    let iqr = p75 - p25;
    let lower_fence = p25 - (iqr * TUKEY_FENCE_MULTIPLIER);
    let upper_fence = p75 + (iqr * TUKEY_FENCE_MULTIPLIER);
    let low_outlier_count = values.iter().filter(|value| **value < lower_fence).count() as u32;
    let high_outlier_count = values.iter().filter(|value| **value > upper_fence).count() as u32;
    let outlier_count = low_outlier_count + high_outlier_count;

    let mut deviations: Vec<f64> = values.iter().map(|value| (value - median).abs()).collect();
    deviations.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let mad = median_from_sorted(&deviations);

    let skewness = if values.len() > 2 && stddev > f64::EPSILON {
        let third_moment = values
            .iter()
            .map(|value| (value - mean).powi(3))
            .sum::<f64>()
            / values.len() as f64;
        third_moment / stddev.powi(3)
    } else {
        0.0
    };
    let kurtosis = if values.len() > 3 && stddev > f64::EPSILON {
        let fourth_moment = values
            .iter()
            .map(|value| (value - mean).powi(4))
            .sum::<f64>()
            / values.len() as f64;
        (fourth_moment / stddev.powi(4)) - 3.0
    } else {
        0.0
    };

    let (standard_error, ci95_lower, ci95_upper) = bootstrap_median_interval(&values);
    let relative_margin_of_error = if median.abs() > f64::EPSILON {
        ((ci95_upper - ci95_lower) / 2.0) / median.abs()
    } else {
        0.0
    };

    MetricSummary {
        sample_count,
        min,
        mean,
        median,
        max,
        stddev,
        cv,
        standard_error,
        variance,
        iqr,
        lower_fence,
        upper_fence,
        low_outlier_count,
        high_outlier_count,
        outlier_count,
        skewness,
        kurtosis,
        mad,
        ci95_lower,
        ci95_upper,
        relative_margin_of_error,
        quality_tier: quality_tier_for_cv(cv),
    }
}

fn confidence_intervals_overlap(a: &MetricSummary, b: &MetricSummary) -> bool {
    a.ci95_lower <= b.ci95_upper && b.ci95_lower <= a.ci95_upper
}

fn cohens_d(candidate: &[f64], baseline: &[f64]) -> f64 {
    if candidate.len() < 2 || baseline.len() < 2 {
        return 0.0;
    }

    let candidate_mean = candidate.iter().sum::<f64>() / candidate.len() as f64;
    let baseline_mean = baseline.iter().sum::<f64>() / baseline.len() as f64;

    let candidate_var = candidate
        .iter()
        .map(|value| (value - candidate_mean).powi(2))
        .sum::<f64>()
        / (candidate.len() as f64 - 1.0);
    let baseline_var = baseline
        .iter()
        .map(|value| (value - baseline_mean).powi(2))
        .sum::<f64>()
        / (baseline.len() as f64 - 1.0);

    let pooled_variance = (((candidate.len() - 1) as f64 * candidate_var)
        + ((baseline.len() - 1) as f64 * baseline_var))
        / ((candidate.len() + baseline.len() - 2) as f64);

    if pooled_variance <= f64::EPSILON {
        0.0
    } else {
        (candidate_mean - baseline_mean) / pooled_variance.sqrt()
    }
}

fn error_rate_pct(result: &BenchmarkResult) -> f64 {
    if result.network.total_requests == 0 {
        0.0
    } else {
        (result.network.error_count as f64 / result.network.total_requests as f64) * 100.0
    }
}

fn summarise_scenario(results: &[&BenchmarkResult], scenario: &str) -> Option<ScenarioSummary> {
    if results.is_empty() {
        return None;
    }

    let mut repeat_indices: Vec<u32> = results.iter().map(|result| result.repeat_index).collect();
    repeat_indices.sort_unstable();
    repeat_indices.dedup();

    Some(ScenarioSummary {
        scenario: scenario.to_string(),
        repeat_count: repeat_indices.len() as u32,
        repeat_indices,
        rps: summarise_metric(results.iter().map(|result| result.network.rps)),
        latency_mean_ms: summarise_metric(
            results.iter().map(|result| result.network.latency_mean_ms),
        ),
        latency_p50_ms: summarise_metric(
            results.iter().map(|result| result.network.latency_p50_ms),
        ),
        latency_p99_ms: summarise_metric(
            results.iter().map(|result| result.network.latency_p99_ms),
        ),
        latency_p999_ms: summarise_metric(
            results.iter().map(|result| result.network.latency_p999_ms),
        ),
        latency_max_ms: summarise_metric(
            results.iter().map(|result| result.network.latency_max_ms),
        ),
        error_rate_pct: summarise_metric(results.iter().map(|result| error_rate_pct(result))),
        avg_cpu_fraction: summarise_metric(
            results
                .iter()
                .map(|result| result.resources.avg_cpu_fraction),
        ),
        peak_rss_bytes: summarise_metric(
            results
                .iter()
                .map(|result| result.resources.peak_rss_bytes as f64),
        ),
        time_to_first_response_ms: summarise_metric(
            results
                .iter()
                .map(|result| result.startup.time_to_first_response_ms),
        ),
        binary_size_bytes: summarise_metric(
            results.iter().map(|result| result.binary.size_bytes as f64),
        ),
    })
}

fn primary_rps(summary: &BenchmarkCaseSummary) -> f64 {
    summary
        .warm
        .as_ref()
        .map(|warm| warm.rps.median)
        .or_else(|| summary.cold.as_ref().map(|cold| cold.rps.median))
        .unwrap_or(0.0)
}

fn case_key_matches_baseline(key: &CaseKey, baseline: &BenchmarkBaseline) -> bool {
    key.language.eq_ignore_ascii_case(&baseline.language)
        && baseline
            .runtime
            .as_ref()
            .map(|runtime| key.runtime.eq_ignore_ascii_case(runtime))
            .unwrap_or(true)
}

fn case_key_for_summary(summary: &BenchmarkCaseSummary) -> CaseKey {
    CaseKey {
        language: summary.language.clone(),
        runtime: summary.runtime.clone(),
        concurrency: summary.concurrency,
    }
}

fn push_optional_string_mismatch(
    notes: &mut Vec<String>,
    label: &str,
    candidate: Option<&str>,
    baseline: Option<&str>,
) {
    if let (Some(candidate), Some(baseline)) = (candidate, baseline) {
        if !candidate.eq_ignore_ascii_case(baseline) {
            notes.push(format!("{label} differs ({candidate} vs {baseline})"));
        }
    }
}

fn push_optional_u32_mismatch(
    notes: &mut Vec<String>,
    label: &str,
    candidate: Option<u32>,
    baseline: Option<u32>,
) {
    if let (Some(candidate), Some(baseline)) = (candidate, baseline) {
        if candidate != baseline {
            notes.push(format!("{label} differs ({candidate} vs {baseline})"));
        }
    }
}

fn materially_different_ratio(candidate: Option<f64>, baseline: Option<f64>) -> Option<f64> {
    let (candidate, baseline) = (candidate?, baseline?);
    if candidate <= f64::EPSILON || baseline <= f64::EPSILON {
        return None;
    }
    Some(candidate.max(baseline) / candidate.min(baseline))
}

pub(crate) fn environment_comparability_notes(
    candidate: &BenchmarkEnvironmentFingerprint,
    baseline: &BenchmarkEnvironmentFingerprint,
) -> Vec<String> {
    let mut notes = Vec::new();

    if candidate.is_empty() {
        notes.push("candidate benchmark environment fingerprint is missing".to_string());
    }
    if baseline.is_empty() {
        notes.push("baseline benchmark environment fingerprint is missing".to_string());
    }
    if !notes.is_empty() {
        return notes;
    }

    push_optional_string_mismatch(
        &mut notes,
        "client OS",
        candidate.client_os.as_deref(),
        baseline.client_os.as_deref(),
    );
    push_optional_string_mismatch(
        &mut notes,
        "client architecture",
        candidate.client_arch.as_deref(),
        baseline.client_arch.as_deref(),
    );
    push_optional_u32_mismatch(
        &mut notes,
        "client CPU cores",
        candidate.client_cpu_cores,
        baseline.client_cpu_cores,
    );
    push_optional_string_mismatch(
        &mut notes,
        "client region",
        candidate.client_region.as_deref(),
        baseline.client_region.as_deref(),
    );
    push_optional_string_mismatch(
        &mut notes,
        "server OS",
        candidate.server_os.as_deref(),
        baseline.server_os.as_deref(),
    );
    push_optional_string_mismatch(
        &mut notes,
        "server architecture",
        candidate.server_arch.as_deref(),
        baseline.server_arch.as_deref(),
    );
    push_optional_u32_mismatch(
        &mut notes,
        "server CPU cores",
        candidate.server_cpu_cores,
        baseline.server_cpu_cores,
    );
    push_optional_string_mismatch(
        &mut notes,
        "server region",
        candidate.server_region.as_deref(),
        baseline.server_region.as_deref(),
    );

    match (
        candidate.network_type.as_deref(),
        baseline.network_type.as_deref(),
    ) {
        (Some(candidate), Some(baseline)) if !candidate.eq_ignore_ascii_case(baseline) => {
            notes.push(format!("network type differs ({candidate} vs {baseline})"))
        }
        (None, _) | (_, None) => notes
            .push("network type is missing from the benchmark environment fingerprint".to_string()),
        _ => {}
    }

    if let Some(ratio) =
        materially_different_ratio(candidate.baseline_rtt_p50_ms, baseline.baseline_rtt_p50_ms)
    {
        if ratio > MAX_COMPARABLE_RTT_RATIO {
            notes.push(format!(
                "baseline RTT p50 differs materially ({:.2} ms vs {:.2} ms)",
                candidate.baseline_rtt_p50_ms.unwrap_or_default(),
                baseline.baseline_rtt_p50_ms.unwrap_or_default(),
            ));
        }
    } else {
        notes.push(
            "baseline RTT p50 is missing from the benchmark environment fingerprint".to_string(),
        );
    }

    if let Some(ratio) =
        materially_different_ratio(candidate.baseline_rtt_p95_ms, baseline.baseline_rtt_p95_ms)
    {
        if ratio > MAX_COMPARABLE_RTT_RATIO {
            notes.push(format!(
                "baseline RTT p95 differs materially ({:.2} ms vs {:.2} ms)",
                candidate.baseline_rtt_p95_ms.unwrap_or_default(),
                baseline.baseline_rtt_p95_ms.unwrap_or_default(),
            ));
        }
    }

    notes.sort();
    notes.dedup();
    notes
}

fn scenario_results<'a>(
    results: &[&'a BenchmarkResult],
    scenario: &str,
) -> Vec<&'a BenchmarkResult> {
    results
        .iter()
        .copied()
        .filter(|result| result_scenario(result) == scenario)
        .collect()
}

fn paired_ratio_values<F>(
    candidate_results: &[&BenchmarkResult],
    baseline_results: &[&BenchmarkResult],
    metric: F,
) -> Vec<f64>
where
    F: Fn(&BenchmarkResult) -> f64,
{
    let candidate_by_repeat: BTreeMap<u32, &BenchmarkResult> = candidate_results
        .iter()
        .copied()
        .map(|result| (result.repeat_index, result))
        .collect();
    let baseline_by_repeat: BTreeMap<u32, &BenchmarkResult> = baseline_results
        .iter()
        .copied()
        .map(|result| (result.repeat_index, result))
        .collect();

    candidate_by_repeat
        .iter()
        .filter_map(|(repeat_index, candidate)| {
            baseline_by_repeat.get(repeat_index).and_then(|baseline| {
                let baseline_value = metric(baseline);
                if baseline_value.abs() <= f64::EPSILON {
                    None
                } else {
                    Some(metric(candidate) / baseline_value)
                }
            })
        })
        .collect()
}

fn build_comparison_metric<F>(
    candidate_results: &[&BenchmarkResult],
    baseline_results: &[&BenchmarkResult],
    higher_is_better: bool,
    metric: F,
    lower_verdict: &str,
    higher_verdict: &str,
) -> ComparisonMetricSummary
where
    F: Fn(&BenchmarkResult) -> f64 + Copy,
{
    let candidate_values: Vec<f64> = candidate_results
        .iter()
        .map(|result| metric(result))
        .collect();
    let baseline_values: Vec<f64> = baseline_results
        .iter()
        .map(|result| metric(result))
        .collect();
    let candidate_summary = summarise_metric(candidate_values.iter().copied());
    let baseline_summary = summarise_metric(baseline_values.iter().copied());
    let ratio_summary = summarise_metric(paired_ratio_values(
        candidate_results,
        baseline_results,
        metric,
    ));
    let absolute_delta = candidate_summary.median - baseline_summary.median;
    let percent_delta = if baseline_summary.median.abs() > f64::EPSILON {
        (absolute_delta / baseline_summary.median) * 100.0
    } else {
        0.0
    };
    let ratio = if baseline_summary.median.abs() > f64::EPSILON {
        candidate_summary.median / baseline_summary.median
    } else {
        0.0
    };
    let effect_size = cohens_d(&candidate_values, &baseline_values);
    let verdict = if candidate_values.is_empty() || baseline_values.is_empty() {
        "insufficient data".to_string()
    } else if percent_delta.abs() <= 5.0
        && confidence_intervals_overlap(&candidate_summary, &baseline_summary)
    {
        "same within 5%".to_string()
    } else if higher_is_better {
        if percent_delta >= 0.0 {
            higher_verdict.to_string()
        } else {
            lower_verdict.to_string()
        }
    } else if percent_delta <= 0.0 {
        lower_verdict.to_string()
    } else {
        higher_verdict.to_string()
    };

    ComparisonMetricSummary {
        higher_is_better,
        candidate_summary,
        baseline_summary,
        absolute_delta,
        percent_delta,
        ratio,
        ratio_summary,
        effect_size,
        verdict,
    }
}

fn build_scenario_comparison(
    candidate_results: &[&BenchmarkResult],
    baseline_results: &[&BenchmarkResult],
    scenario: &str,
) -> Option<ScenarioComparison> {
    if candidate_results.is_empty() || baseline_results.is_empty() {
        return None;
    }

    let shared_repeat_count = paired_ratio_values(candidate_results, baseline_results, |result| {
        result.network.rps
    })
    .len() as u32;

    Some(ScenarioComparison {
        scenario: scenario.to_string(),
        shared_repeat_count,
        throughput: build_comparison_metric(
            candidate_results,
            baseline_results,
            true,
            |result| result.network.rps,
            "slower",
            "faster",
        ),
        latency_p99: build_comparison_metric(
            candidate_results,
            baseline_results,
            false,
            |result| result.network.latency_p99_ms,
            "lower latency",
            "higher latency",
        ),
    })
}

fn build_comparisons(
    grouped: &GroupedCaseResults<'_>,
    case_summaries: &[BenchmarkCaseSummary],
    baseline: &BenchmarkBaseline,
) -> Vec<BenchmarkComparison> {
    let mut comparisons = Vec::new();

    for summary in case_summaries {
        let candidate_key = case_key_for_summary(summary);
        if case_key_matches_baseline(&candidate_key, baseline) {
            continue;
        }

        let Some((baseline_key, baseline_summary)) = case_summaries.iter().find_map(|candidate| {
            let key = case_key_for_summary(candidate);
            if key.concurrency == summary.concurrency && case_key_matches_baseline(&key, baseline) {
                Some((key, candidate))
            } else {
                None
            }
        }) else {
            continue;
        };

        let Some(candidate_results) = grouped.get(&candidate_key) else {
            continue;
        };
        let Some(baseline_results) = grouped.get(&baseline_key) else {
            continue;
        };

        let candidate_warm = scenario_results(candidate_results, "warm");
        let candidate_cold = scenario_results(candidate_results, "cold");
        let baseline_warm = scenario_results(baseline_results, "warm");
        let baseline_cold = scenario_results(baseline_results, "cold");
        let comparability_notes = candidate_results
            .first()
            .zip(baseline_results.first())
            .map(|(candidate, baseline)| {
                environment_comparability_notes(&candidate.environment, &baseline.environment)
            })
            .unwrap_or_else(|| {
                vec!["candidate or baseline benchmark result is missing".to_string()]
            });
        let comparable = comparability_notes.is_empty();

        comparisons.push(BenchmarkComparison {
            case_label: summary.case_label.clone(),
            language: summary.language.clone(),
            runtime: summary.runtime.clone(),
            concurrency: summary.concurrency,
            baseline_case_label: baseline_summary.case_label.clone(),
            comparable,
            comparability_notes,
            warm: comparable
                .then(|| build_scenario_comparison(&candidate_warm, &baseline_warm, "warm"))
                .flatten(),
            cold: comparable
                .then(|| build_scenario_comparison(&candidate_cold, &baseline_cold, "cold"))
                .flatten(),
        });
    }

    comparisons.sort_by(|a, b| match (a.comparable, b.comparable) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => {
            let a_ratio = a
                .warm
                .as_ref()
                .map(|warm| warm.throughput.ratio)
                .unwrap_or(0.0);
            let b_ratio = b
                .warm
                .as_ref()
                .map(|warm| warm.throughput.ratio)
                .unwrap_or(0.0);
            b_ratio
                .partial_cmp(&a_ratio)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.case_label.cmp(&b.case_label))
        }
    });
    comparisons
}

fn summarise_case_groups(grouped: &GroupedCaseResults<'_>) -> Vec<BenchmarkCaseSummary> {
    let mut summaries = grouped
        .iter()
        .map(|(key, results)| {
            let cold_results = scenario_results(results, "cold");
            let warm_results = scenario_results(results, "warm");

            BenchmarkCaseSummary {
                case_label: format!("{} c={}", key.language, key.concurrency),
                language: key.language.clone(),
                runtime: key.runtime.clone(),
                concurrency: key.concurrency,
                warm: summarise_scenario(&warm_results, "warm"),
                cold: summarise_scenario(&cold_results, "cold"),
            }
        })
        .collect::<Vec<_>>();

    summaries.sort_by(|a, b| {
        primary_rps(b)
            .partial_cmp(&primary_rps(a))
            .unwrap_or(Ordering::Equal)
    });
    summaries
}

pub(crate) fn summarise_cases(run: &BenchmarkRun) -> Vec<BenchmarkCaseSummary> {
    summarise_results(&run.results)
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

fn format_duration_ms(ms: f64) -> String {
    if ms >= 1000.0 {
        format!("{:.2}s", ms / 1000.0)
    } else {
        format!("{:.1}ms", ms)
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ---------------------------------------------------------------------------
// SVG chart builders
// ---------------------------------------------------------------------------

/// Color palette for languages (cycles if more than available).
const LANG_COLORS: &[&str] = &[
    "#47bfff", "#ff6b6b", "#51cf66", "#fcc419", "#cc5de8", "#ff922b", "#20c997", "#748ffc",
    "#f06595", "#ced4da",
];

fn lang_color(index: usize) -> &'static str {
    LANG_COLORS[index % LANG_COLORS.len()]
}

fn warm_summary(summary: &BenchmarkCaseSummary) -> Option<&ScenarioSummary> {
    summary.warm.as_ref()
}

fn cold_summary(summary: &BenchmarkCaseSummary) -> Option<&ScenarioSummary> {
    summary.cold.as_ref()
}

/// Build a grouped bar chart comparing cold vs warm median RPS per case.
fn svg_cold_warm_chart(summaries: &[BenchmarkCaseSummary]) -> String {
    let chart_w = 800.0_f64;
    let chart_h = 400.0_f64;
    let margin_l = 80.0;
    let margin_r = 20.0;
    let margin_t = 30.0;
    let margin_b = 100.0;
    let plot_w = chart_w - margin_l - margin_r;
    let plot_h = chart_h - margin_t - margin_b;

    let n = summaries.len().max(1) as f64;
    let group_w = plot_w / n;
    let bar_w = (group_w * 0.35).min(50.0);

    let max_rps = summaries
        .iter()
        .flat_map(|s| {
            let cold_rps = cold_summary(s).map_or(0.0, |cold| cold.rps.max);
            let warm_rps = warm_summary(s).map_or(0.0, |warm| warm.rps.max);
            vec![warm_rps, cold_rps]
        })
        .fold(0.0_f64, f64::max)
        * 1.15;
    let max_rps = if max_rps == 0.0 { 100.0 } else { max_rps };

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg viewBox=\"0 0 {} {}\" xmlns=\"http://www.w3.org/2000/svg\" \
         style=\"width:100%;max-width:{}px;font-family:'JetBrains Mono',monospace;font-size:11px;\">",
        chart_w, chart_h, chart_w
    );

    // Y-axis gridlines + labels
    let ticks = 5;
    for i in 0..=ticks {
        let val = max_rps * i as f64 / ticks as f64;
        let y = margin_t + plot_h - (plot_h * i as f64 / ticks as f64);
        let _ =
            write!(
            svg,
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"#2a2d35\" stroke-width=\"1\"/>",
            margin_l, y, margin_l + plot_w, y
        );
        let _ = write!(
            svg,
            "<text x=\"{}\" y=\"{}\" fill=\"#8b8fa3\" text-anchor=\"end\" \
             dominant-baseline=\"middle\">{:.0}</text>",
            margin_l - 8.0,
            y,
            val
        );
    }

    // Y-axis label
    let _ = write!(
        svg,
        "<text x=\"14\" y=\"{}\" fill=\"#8b8fa3\" text-anchor=\"middle\" \
         dominant-baseline=\"middle\" transform=\"rotate(-90, 14, {})\" \
         font-size=\"12\">req/s</text>",
        margin_t + plot_h / 2.0,
        margin_t + plot_h / 2.0
    );

    // Bars
    for (i, s) in summaries.iter().enumerate() {
        let gx = margin_l + i as f64 * group_w + group_w / 2.0;
        let cold = cold_summary(s);
        let warm = warm_summary(s);
        let cold_rps = cold.map_or(0.0, |scenario| scenario.rps.median);

        // Cold bar
        let cold_h = (cold_rps / max_rps) * plot_h;
        let cold_y = margin_t + plot_h - cold_h;
        let _ = write!(
            svg,
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" \
             fill=\"#ff6b6b\" opacity=\"0.7\" rx=\"2\">\
             <title>Cold median: {:.0} req/s (n={})</title></rect>",
            gx - bar_w - 2.0,
            cold_y,
            bar_w,
            cold_h,
            cold_rps,
            cold.map_or(0, |scenario| scenario.repeat_count)
        );
        if let Some(cold) = cold {
            draw_range_whisker(
                &mut svg,
                gx - bar_w / 2.0 - 2.0,
                cold.rps.min,
                cold.rps.max,
                max_rps,
                margin_t,
                plot_h,
                "#ff6b6b",
            );
        }

        // Warm bar
        let warm_rps = warm.map_or(0.0, |scenario| scenario.rps.median);
        let warm_h = (warm_rps / max_rps) * plot_h;
        let warm_y = margin_t + plot_h - warm_h;
        let _ = write!(
            svg,
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" \
             fill=\"#47bfff\" opacity=\"0.85\" rx=\"2\">\
             <title>Warm median: {:.0} req/s (n={})</title></rect>",
            gx + 2.0,
            warm_y,
            bar_w,
            warm_h,
            warm_rps,
            warm.map_or(0, |scenario| scenario.repeat_count)
        );
        if let Some(warm) = warm {
            draw_range_whisker(
                &mut svg,
                gx + bar_w / 2.0 + 2.0,
                warm.rps.min,
                warm.rps.max,
                max_rps,
                margin_t,
                plot_h,
                "#47bfff",
            );
        }

        // X-axis label
        let label_y = margin_t + plot_h + 16.0;
        let _ = write!(
            svg,
            "<text x=\"{}\" y=\"{}\" fill=\"#e5e7eb\" text-anchor=\"middle\" \
             font-size=\"11\" transform=\"rotate(-35, {}, {})\">{}</text>",
            gx,
            label_y,
            gx,
            label_y,
            escape_html(&s.case_label)
        );
    }

    // Legend
    let ly = chart_h - 10.0;
    let _ = write!(
        svg,
        "<rect x=\"{}\" y=\"{}\" width=\"12\" height=\"12\" \
         fill=\"#ff6b6b\" opacity=\"0.7\" rx=\"2\"/>",
        margin_l,
        ly - 10.0
    );
    let _ = write!(
        svg,
        "<text x=\"{}\" y=\"{}\" fill=\"#e5e7eb\" font-size=\"11\">Cold</text>",
        margin_l + 16.0,
        ly
    );
    let _ = write!(
        svg,
        "<rect x=\"{}\" y=\"{}\" width=\"12\" height=\"12\" \
         fill=\"#47bfff\" opacity=\"0.85\" rx=\"2\"/>",
        margin_l + 70.0,
        ly - 10.0
    );
    let _ = write!(
        svg,
        "<text x=\"{}\" y=\"{}\" fill=\"#e5e7eb\" font-size=\"11\">Warm</text>",
        margin_l + 86.0,
        ly
    );

    svg.push_str("</svg>");
    svg
}

fn draw_range_whisker(
    svg: &mut String,
    x: f64,
    min: f64,
    max: f64,
    max_value: f64,
    margin_t: f64,
    plot_h: f64,
    color: &str,
) {
    if max <= min || max_value <= 0.0 {
        return;
    }

    let y_min = margin_t + plot_h - (min / max_value) * plot_h;
    let y_max = margin_t + plot_h - (max / max_value) * plot_h;
    let _ = write!(
        svg,
        "<line x1=\"{x}\" y1=\"{y_max}\" x2=\"{x}\" y2=\"{y_min}\" stroke=\"{color}\" stroke-width=\"1.5\" opacity=\"0.95\"/>\
         <line x1=\"{}\" y1=\"{y_max}\" x2=\"{}\" y2=\"{y_max}\" stroke=\"{color}\" stroke-width=\"1.5\" opacity=\"0.95\"/>\
         <line x1=\"{}\" y1=\"{y_min}\" x2=\"{}\" y2=\"{y_min}\" stroke=\"{color}\" stroke-width=\"1.5\" opacity=\"0.95\"/>",
        x - 5.0,
        x + 5.0,
        x - 5.0,
        x + 5.0,
    );
}

/// Build a grouped bar chart of warm latency quantiles per case.
fn svg_latency_chart(summaries: &[BenchmarkCaseSummary]) -> String {
    let chart_w = 800.0_f64;
    let chart_h = 400.0_f64;
    let margin_l = 80.0;
    let margin_r = 20.0;
    let margin_t = 30.0;
    let margin_b = 100.0;
    let plot_w = chart_w - margin_l - margin_r;
    let plot_h = chart_h - margin_t - margin_b;

    let n = summaries.len().max(1) as f64;
    let group_w = plot_w / n;
    let bar_w = (group_w * 0.25).min(35.0);

    let max_lat = summaries
        .iter()
        .filter_map(|summary| warm_summary(summary).map(|warm| warm.latency_p999_ms.max))
        .fold(0.0_f64, f64::max)
        * 1.2;
    let max_lat = if max_lat == 0.0 { 10.0 } else { max_lat };

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg viewBox=\"0 0 {} {}\" xmlns=\"http://www.w3.org/2000/svg\" \
         style=\"width:100%;max-width:{}px;font-family:'JetBrains Mono',monospace;font-size:11px;\">",
        chart_w, chart_h, chart_w
    );

    // Y-axis gridlines
    let ticks = 5;
    for i in 0..=ticks {
        let val = max_lat * i as f64 / ticks as f64;
        let y = margin_t + plot_h - (plot_h * i as f64 / ticks as f64);
        let _ =
            write!(
            svg,
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"#2a2d35\" stroke-width=\"1\"/>",
            margin_l, y, margin_l + plot_w, y
        );
        let _ = write!(
            svg,
            "<text x=\"{}\" y=\"{}\" fill=\"#8b8fa3\" text-anchor=\"end\" \
             dominant-baseline=\"middle\">{:.1}</text>",
            margin_l - 8.0,
            y,
            val
        );
    }

    let _ = write!(
        svg,
        "<text x=\"14\" y=\"{}\" fill=\"#8b8fa3\" text-anchor=\"middle\" \
         dominant-baseline=\"middle\" transform=\"rotate(-90, 14, {})\" \
         font-size=\"12\">latency (ms)</text>",
        margin_t + plot_h / 2.0,
        margin_t + plot_h / 2.0
    );

    let percentile_colors = ["#47bfff", "#fcc419", "#ff6b6b"];

    for (i, s) in summaries.iter().enumerate() {
        let gx = margin_l + i as f64 * group_w + group_w / 2.0;
        let Some(warm) = warm_summary(s) else {
            continue;
        };
        let vals = [
            warm.latency_p50_ms.median,
            warm.latency_p99_ms.median,
            warm.latency_p999_ms.median,
        ];
        let labels = ["p50", "p99", "p99.9"];

        for (j, (&val, &color)) in vals.iter().zip(percentile_colors.iter()).enumerate() {
            let bx = gx + (j as f64 - 1.0) * (bar_w + 3.0);
            let h = (val / max_lat) * plot_h;
            let y = margin_t + plot_h - h;
            let _ = write!(
                svg,
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" \
                 fill=\"{}\" opacity=\"0.8\" rx=\"2\">\
                 <title>{}: {:.2}ms</title></rect>",
                bx - bar_w / 2.0,
                y,
                bar_w,
                h,
                color,
                labels[j],
                val
            );
        }

        let label_y = margin_t + plot_h + 16.0;
        let _ = write!(
            svg,
            "<text x=\"{}\" y=\"{}\" fill=\"#e5e7eb\" text-anchor=\"middle\" \
             font-size=\"11\" transform=\"rotate(-35, {}, {})\">{}</text>",
            gx,
            label_y,
            gx,
            label_y,
            escape_html(&s.case_label)
        );
    }

    // Legend
    let ly = chart_h - 10.0;
    for (j, (&color, label)) in percentile_colors
        .iter()
        .zip(["p50", "p99", "p99.9"])
        .enumerate()
    {
        let lx = margin_l + j as f64 * 70.0;
        let _ = write!(
            svg,
            "<rect x=\"{}\" y=\"{}\" width=\"12\" height=\"12\" \
             fill=\"{}\" opacity=\"0.8\" rx=\"2\"/>",
            lx,
            ly - 10.0,
            color
        );
        let _ = write!(
            svg,
            "<text x=\"{}\" y=\"{}\" fill=\"#e5e7eb\" font-size=\"11\">{}</text>",
            lx + 16.0,
            ly,
            label
        );
    }

    svg.push_str("</svg>");
    svg
}

/// Build a horizontal bar chart for CPU% and Memory per language.
fn svg_resource_chart(summaries: &[BenchmarkCaseSummary]) -> String {
    let chart_w = 800.0_f64;
    let bar_h = 22.0;
    let row_h = 56.0;
    let margin_l = 120.0;
    let margin_r = 100.0;
    let margin_t = 30.0;
    let chart_h = margin_t + summaries.len() as f64 * row_h + 20.0;
    let plot_w = chart_w - margin_l - margin_r;

    let max_mem = summaries
        .iter()
        .filter_map(|summary| warm_summary(summary).map(|warm| warm.peak_rss_bytes.max))
        .fold(1.0_f64, f64::max)
        * 1.15;
    let max_cpu = summaries
        .iter()
        .filter_map(|summary| warm_summary(summary).map(|warm| warm.avg_cpu_fraction.max))
        .fold(0.0_f64, f64::max)
        * 1.15;
    let max_cpu = if max_cpu == 0.0 { 1.0 } else { max_cpu };

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg viewBox=\"0 0 {} {}\" xmlns=\"http://www.w3.org/2000/svg\" \
         style=\"width:100%;max-width:{}px;font-family:'JetBrains Mono',monospace;font-size:11px;\">",
        chart_w, chart_h, chart_w
    );

    for (i, s) in summaries.iter().enumerate() {
        let gy = margin_t + i as f64 * row_h;
        let Some(warm) = warm_summary(s) else {
            continue;
        };

        // Language label
        let _ = write!(
            svg,
            "<text x=\"{}\" y=\"{}\" fill=\"#e5e7eb\" text-anchor=\"end\" \
             dominant-baseline=\"middle\" font-size=\"12\">{}</text>",
            margin_l - 10.0,
            gy + bar_h / 2.0,
            escape_html(&s.case_label)
        );

        // CPU bar
        let cpu_w = (warm.avg_cpu_fraction.median / max_cpu) * plot_w;
        let _ = write!(
            svg,
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" \
             fill=\"#47bfff\" opacity=\"0.8\" rx=\"2\"/>",
            margin_l, gy, cpu_w, bar_h
        );
        let _ = write!(
            svg,
            "<text x=\"{}\" y=\"{}\" fill=\"#e5e7eb\" dominant-baseline=\"middle\" \
             font-size=\"10\">{:.1}%</text>",
            margin_l + cpu_w + 5.0,
            gy + bar_h / 2.0,
            warm.avg_cpu_fraction.median * 100.0
        );

        // Memory bar
        let mem_y = gy + bar_h + 4.0;
        let mem_w = (warm.peak_rss_bytes.median / max_mem) * plot_w;
        let _ = write!(
            svg,
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" \
             fill=\"#cc5de8\" opacity=\"0.7\" rx=\"2\"/>",
            margin_l, mem_y, mem_w, bar_h
        );
        let _ = write!(
            svg,
            "<text x=\"{}\" y=\"{}\" fill=\"#e5e7eb\" dominant-baseline=\"middle\" \
             font-size=\"10\">{}</text>",
            margin_l + mem_w + 5.0,
            mem_y + bar_h / 2.0,
            format_bytes(warm.peak_rss_bytes.median.round() as u64)
        );
    }

    // Legend
    let ly = chart_h - 8.0;
    let _ = write!(
        svg,
        "<rect x=\"{}\" y=\"{}\" width=\"12\" height=\"12\" \
         fill=\"#47bfff\" opacity=\"0.8\" rx=\"2\"/>",
        margin_l,
        ly - 10.0
    );
    let _ = write!(
        svg,
        "<text x=\"{}\" y=\"{}\" fill=\"#e5e7eb\" font-size=\"11\">Avg CPU</text>",
        margin_l + 16.0,
        ly
    );
    let _ = write!(
        svg,
        "<rect x=\"{}\" y=\"{}\" width=\"12\" height=\"12\" \
         fill=\"#cc5de8\" opacity=\"0.7\" rx=\"2\"/>",
        margin_l + 100.0,
        ly - 10.0
    );
    let _ = write!(
        svg,
        "<text x=\"{}\" y=\"{}\" fill=\"#e5e7eb\" font-size=\"11\">Peak Memory</text>",
        margin_l + 116.0,
        ly
    );

    svg.push_str("</svg>");
    svg
}

// ---------------------------------------------------------------------------
// HTML generation
// ---------------------------------------------------------------------------

/// Generate a standalone HTML comparison report with inline CSS, JS, and SVG charts.
pub fn generate_html(run: &BenchmarkRun, output: &Path) -> Result<()> {
    let report = build_report(run);
    let summaries = report.aggregation.case_summaries.clone();

    let run_name = run
        .config_path
        .rsplit('/')
        .next()
        .unwrap_or(&run.config_path)
        .trim_end_matches(".json");

    let date_str = run.started_at.format("%Y-%m-%d %H:%M UTC").to_string();
    let duration_str = run
        .finished_at
        .map(|f| {
            let dur = f - run.started_at;
            let secs = dur.num_seconds();
            if secs >= 3600 {
                format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
            } else if secs >= 60 {
                format!("{}m {}s", secs / 60, secs % 60)
            } else {
                format!("{}s", secs)
            }
        })
        .unwrap_or_else(|| "in progress".into());

    let mut html = String::with_capacity(32_768);

    // -- Document start --
    html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n");
    let _ = write!(
        html,
        "<title>AletheBench Report — {}</title>\n",
        escape_html(run_name)
    );

    // -- Inline CSS --
    html.push_str("<style>\n");
    write_inline_css(&mut html);
    html.push_str("\n</style>\n</head>\n<body>\n");

    // -- Header --
    html.push_str("<header class=\"page-header\">\n");
    let _ = write!(
        html,
        "<h1>AletheBench Report &mdash; {}</h1>\n",
        escape_html(run_name)
    );
    let _ = write!(
        html,
        "<div class=\"subtitle\">{} &middot; Duration: {} &middot; {} benchmark cases &middot; {} data points</div>\n",
        date_str, duration_str, summaries.len(), run.results.len()
    );
    html.push_str("</header>\n\n");

    // -- Executive Summary --
    write_executive_summary(&mut html, &summaries);

    // -- Leaderboard Table --
    write_leaderboard(&mut html, &summaries);

    if !report.aggregation.comparisons.is_empty() {
        write_baseline_comparisons(&mut html, &report.aggregation.comparisons);
    }

    // -- Cold vs Warm Chart --
    html.push_str("<section class=\"card\">\n<h2>Cold vs Warm Throughput</h2>\n");
    html.push_str(&svg_cold_warm_chart(&summaries));
    html.push_str(
        "\n<p class=\"chart-caption\">Bars show the median req/s across repeated launches. Whiskers show the min-max spread for each case.</p>\n",
    );
    html.push_str("\n</section>\n\n");

    // -- Latency Chart --
    html.push_str("<section class=\"card\">\n<h2>Latency Distribution (Warm)</h2>\n");
    html.push_str(&svg_latency_chart(&summaries));
    html.push_str(
        "\n<p class=\"chart-caption\">Warm latency bars use median values across repeats for p50, p99, and p99.9.</p>\n",
    );
    html.push_str("\n</section>\n\n");

    // -- Resource Usage Chart --
    html.push_str("<section class=\"card\">\n<h2>Resource Usage</h2>\n");
    html.push_str(&svg_resource_chart(&summaries));
    html.push_str(
        "\n<p class=\"chart-caption\">Resource bars use the median warm-launch CPU and memory values for each case.</p>\n",
    );
    html.push_str("\n</section>\n\n");

    // -- Methodology --
    write_methodology(&mut html, &report);

    // -- Inline JS for sorting --
    html.push_str("<script>\n");
    write_inline_js(&mut html);
    html.push_str("\n</script>\n");

    // -- Footer --
    html.push_str("<footer>Generated by AletheBench &middot; ");
    let _ = write!(html, "Run ID: {}</footer>\n", run.id);
    html.push_str("</body>\n</html>\n");

    std::fs::write(output, &html)?;
    tracing::info!("Wrote HTML report to {}", output.display());
    Ok(())
}

fn write_executive_summary(html: &mut String, summaries: &[BenchmarkCaseSummary]) {
    html.push_str("<section class=\"card\">\n<h2>Executive Summary</h2>\n");
    html.push_str("<div class=\"summary-grid\">\n");

    // Top 3 by warm throughput
    html.push_str(
        "<div class=\"summary-item\">\n<h3>Highest Throughput (warm median)</h3>\n<ol>\n",
    );
    for s in summaries
        .iter()
        .filter(|summary| summary.warm.is_some())
        .take(3)
    {
        let warm = warm_summary(s).unwrap();
        let _ = write!(
            html,
            "<li><span class=\"lang-tag\">{}</span> <span class=\"metric\">{:.0} req/s (c={}, n={})</span></li>\n",
            escape_html(&s.language),
            warm.rps.median,
            s.concurrency,
            warm.repeat_count
        );
    }
    html.push_str("</ol>\n</div>\n");

    // Lowest p99 latency
    let mut by_p99: Vec<&BenchmarkCaseSummary> = summaries
        .iter()
        .filter(|summary| summary.warm.is_some())
        .collect();
    by_p99.sort_by(|a, b| {
        warm_summary(a)
            .unwrap()
            .latency_p99_ms
            .median
            .partial_cmp(&warm_summary(b).unwrap().latency_p99_ms.median)
            .unwrap()
    });
    html.push_str(
        "<div class=\"summary-item\">\n<h3>Lowest p99 Latency (warm median)</h3>\n<ol>\n",
    );
    for s in by_p99.iter().take(3) {
        let warm = warm_summary(s).unwrap();
        let _ = write!(
            html,
            "<li><span class=\"lang-tag\">{}</span> <span class=\"metric\">{} (c={})</span></li>\n",
            escape_html(&s.language),
            format_duration_ms(warm.latency_p99_ms.median),
            s.concurrency
        );
    }
    html.push_str("</ol>\n</div>\n");

    // Lowest memory
    let mut by_mem: Vec<&BenchmarkCaseSummary> = summaries
        .iter()
        .filter(|summary| summary.warm.is_some())
        .collect();
    by_mem.sort_by(|a, b| {
        warm_summary(a)
            .unwrap()
            .peak_rss_bytes
            .median
            .partial_cmp(&warm_summary(b).unwrap().peak_rss_bytes.median)
            .unwrap_or(Ordering::Equal)
    });
    html.push_str("<div class=\"summary-item\">\n<h3>Lowest Memory</h3>\n<ol>\n");
    for s in by_mem.iter().take(3) {
        let warm = warm_summary(s).unwrap();
        let _ = write!(
            html,
            "<li><span class=\"lang-tag\">{}</span> <span class=\"metric\">{} (c={})</span></li>\n",
            escape_html(&s.language),
            format_bytes(warm.peak_rss_bytes.median.round() as u64),
            s.concurrency
        );
    }
    html.push_str("</ol>\n</div>\n");

    html.push_str(
        "<div class=\"summary-item\">\n<h3>Methodology Signal</h3>\n<ul class=\"summary-notes\">\n",
    );
    html.push_str("<li>Rankings use median warm throughput by case, not single best runs.</li>\n");
    html.push_str("<li>Cases are grouped by language, runtime, and concurrency.</li>\n");
    html.push_str("<li>Launch-to-launch spread is shown with CV% in the leaderboard.</li>\n");
    html.push_str("</ul>\n</div>\n");

    html.push_str("</div>\n</section>\n\n");
}

fn write_leaderboard(html: &mut String, summaries: &[BenchmarkCaseSummary]) {
    html.push_str("<section class=\"card\">\n<h2>Case Summary</h2>\n");
    html.push_str("<table id=\"leaderboard\">\n<thead>\n<tr>");

    let headers = [
        "Rank",
        "Language",
        "Runtime",
        "C",
        "Repeats",
        "Warm req/s",
        "Warm CV",
        "Warm p99",
        "Cold req/s",
        "Memory",
        "Binary Size",
    ];
    for (i, h) in headers.iter().enumerate() {
        let _ = write!(html, "<th data-col=\"{}\" class=\"sortable\">{}</th>", i, h);
    }
    html.push_str("</tr>\n</thead>\n<tbody>\n");

    for (rank, s) in summaries.iter().enumerate() {
        let warm = warm_summary(s);
        let cold = cold_summary(s);
        let warm_rps = warm.map_or(0.0, |summary| summary.rps.median);
        let warm_cv = warm.map_or(0.0, |summary| summary.rps.cv * 100.0);
        let warm_p99 = warm.map_or(0.0, |summary| summary.latency_p99_ms.median);
        let cold_rps = cold.map_or(0.0, |summary| summary.rps.median);
        let repeat_count = warm
            .map(|summary| summary.repeat_count)
            .or_else(|| cold.map(|summary| summary.repeat_count))
            .unwrap_or(0);
        let memory_bytes = warm
            .map(|summary| summary.peak_rss_bytes.median.round() as u64)
            .unwrap_or(0);
        let binary_size = warm
            .map(|summary| summary.binary_size_bytes.median.round() as u64)
            .or_else(|| cold.map(|summary| summary.binary_size_bytes.median.round() as u64))
            .unwrap_or(0);

        let rank_class = match rank {
            0 => " class=\"rank-gold\"",
            1 => " class=\"rank-silver\"",
            2 => " class=\"rank-bronze\"",
            _ => "",
        };

        let _ = write!(
            html,
            "<tr>\
             <td{}>{}</td>\
             <td><span class=\"lang-dot\" style=\"background:{}\"></span>{}</td>\
             <td>{}</td>\
             <td data-sort=\"{}\">{}</td>\
             <td data-sort=\"{}\">{}</td>\
             <td data-sort=\"{}\">{:.0}</td>\
             <td data-sort=\"{}\">{:.1}%</td>\
             <td data-sort=\"{}\">{:.2}</td>\
             <td data-sort=\"{}\">{:.0}</td>\
             <td data-sort=\"{}\">{}</td>\
             <td data-sort=\"{}\">{}</td>\
             </tr>\n",
            rank_class,
            rank + 1,
            lang_color(rank),
            escape_html(&s.language),
            escape_html(&s.runtime),
            s.concurrency,
            s.concurrency,
            repeat_count,
            repeat_count,
            warm_rps,
            warm_rps,
            warm_cv,
            warm_cv,
            warm_p99,
            warm_p99,
            cold_rps,
            cold_rps,
            memory_bytes,
            format_bytes(memory_bytes),
            binary_size,
            format_bytes(binary_size),
        );
    }

    html.push_str("</tbody>\n</table>\n</section>\n\n");
}

fn write_baseline_comparisons(html: &mut String, comparisons: &[BenchmarkComparison]) {
    html.push_str("<section class=\"card\">\n<h2>Baseline Comparison</h2>\n");
    html.push_str(
        "<p class=\"chart-caption\">Comparisons are evaluated against the configured baseline at the same concurrency level only when the recorded benchmark environments match closely enough. Warm and cold scenarios remain separate, and ratio intervals use deterministic bootstrap confidence intervals around the median.</p>\n",
    );
    html.push_str("<table>\n<thead>\n<tr>");
    let headers = [
        "Case",
        "Baseline",
        "Scenario",
        "Req/s Ratio",
        "Req/s Ratio 95% CI",
        "Req/s Delta",
        "Req/s Verdict",
        "p99 Ratio",
        "p99 Ratio 95% CI",
        "p99 Delta",
        "p99 Verdict",
        "Shared Repeats",
        "Comparability",
    ];
    for header in headers {
        let _ = write!(html, "<th>{}</th>", header);
    }
    html.push_str("</tr>\n</thead>\n<tbody>\n");

    for comparison in comparisons {
        if !comparison.comparable {
            let _ = write!(
                html,
                "<tr>\
                 <td>{}</td>\
                 <td>{}</td>\
                 <td>gated</td>\
                 <td>&mdash;</td>\
                 <td>&mdash;</td>\
                 <td>&mdash;</td>\
                 <td>not comparable</td>\
                 <td>&mdash;</td>\
                 <td>&mdash;</td>\
                 <td>&mdash;</td>\
                 <td>not comparable</td>\
                 <td>0</td>\
                 <td>{}</td>\
                 </tr>\n",
                escape_html(&comparison.case_label),
                escape_html(&comparison.baseline_case_label),
                escape_html(&comparison.comparability_notes.join("; ")),
            );
            continue;
        }

        for scenario in [&comparison.warm, &comparison.cold].into_iter().flatten() {
            let _ = write!(
                html,
                "<tr>\
                 <td>{}</td>\
                 <td>{}</td>\
                 <td>{}</td>\
                 <td data-sort=\"{:.4}\">{:.2}x</td>\
                 <td>[{:.2}, {:.2}]</td>\
                 <td data-sort=\"{:.4}\">{:+.1}%</td>\
                 <td>{}</td>\
                 <td data-sort=\"{:.4}\">{:.2}x</td>\
                 <td>[{:.2}, {:.2}]</td>\
                 <td data-sort=\"{:.4}\">{:+.1}%</td>\
                 <td>{}</td>\
                 <td data-sort=\"{}\">{}</td>\
                 <td>comparable</td>\
                 </tr>\n",
                escape_html(&comparison.case_label),
                escape_html(&comparison.baseline_case_label),
                escape_html(&scenario.scenario),
                scenario.throughput.ratio,
                scenario.throughput.ratio,
                scenario.throughput.ratio_summary.ci95_lower,
                scenario.throughput.ratio_summary.ci95_upper,
                scenario.throughput.percent_delta,
                scenario.throughput.percent_delta,
                scenario.throughput.verdict,
                scenario.latency_p99.ratio,
                scenario.latency_p99.ratio,
                scenario.latency_p99.ratio_summary.ci95_lower,
                scenario.latency_p99.ratio_summary.ci95_upper,
                scenario.latency_p99.percent_delta,
                scenario.latency_p99.percent_delta,
                scenario.latency_p99.verdict,
                scenario.shared_repeat_count,
                scenario.shared_repeat_count,
            );
        }
    }

    html.push_str("</tbody>\n</table>\n</section>\n\n");
}

fn write_methodology(html: &mut String, report: &BenchmarkReport) {
    let run = &report.run;
    let aggregation = &report.aggregation;
    html.push_str("<section class=\"card\">\n<h2>Methodology</h2>\n");
    html.push_str("<dl class=\"method-grid\">\n");

    let _ = write!(html, "<dt>Run ID</dt><dd><code>{}</code></dd>\n", run.id);
    let _ = write!(
        html,
        "<dt>Config</dt><dd><code>{}</code></dd>\n",
        escape_html(&run.config_path)
    );
    let _ = write!(
        html,
        "<dt>Case order</dt><dd>{}</dd>\n",
        if let Some(seed) = run.case_randomization_seed {
            format!("randomized (seed {seed})")
        } else {
            "config order".to_string()
        }
    );
    if let Some(policy) = &run.auto_rerun_policy {
        let _ = write!(
            html,
            "<dt>Auto reruns</dt><dd>enabled (target repeats {}, max additional repeats {}, max relative margin {:.0}%)</dd>\n",
            policy.target_repeat_count,
            policy.max_additional_repeats,
            policy.max_relative_margin_of_error * 100.0
        );
    }
    if !run.scheduled_reruns.is_empty() {
        let _ = write!(
            html,
            "<dt>Executed reruns</dt><dd>{}</dd>\n",
            run.scheduled_reruns.len()
        );
    }
    let _ = write!(
        html,
        "<dt>Started</dt><dd>{}</dd>\n",
        run.started_at.format("%Y-%m-%d %H:%M:%S UTC")
    );
    if let Some(f) = run.finished_at {
        let _ = write!(
            html,
            "<dt>Finished</dt><dd>{}</dd>\n",
            f.format("%Y-%m-%d %H:%M:%S UTC")
        );
    }
    let _ = write!(html, "<dt>Data points</dt><dd>{}</dd>\n", run.results.len());
    let _ = write!(
        html,
        "<dt>Aggregation</dt><dd>{}</dd>\n",
        escape_html(&report.aggregation.anti_cherry_picking_policy)
    );
    let _ = write!(
        html,
        "<dt>Grouped by</dt><dd>{}</dd>\n",
        report.aggregation.grouped_by.join(", ")
    );
    let _ = write!(
        html,
        "<dt>Primary estimator</dt><dd>{}</dd>\n",
        escape_html(&report.aggregation.primary_estimator)
    );
    let _ = write!(
        html,
        "<dt>Confidence level</dt><dd>{:.0}%</dd>\n",
        report.aggregation.confidence_level * 100.0
    );
    let _ = write!(
        html,
        "<dt>Uncertainty method</dt><dd>{}</dd>\n",
        escape_html(&report.aggregation.uncertainty_method)
    );
    let _ = write!(
        html,
        "<dt>Outlier policy</dt><dd>{}</dd>\n",
        escape_html(&report.aggregation.outlier_policy)
    );
    if let Some(baseline) = &run.baseline {
        let baseline_label = baseline
            .runtime
            .as_ref()
            .map(|runtime| format!("{}/{}", baseline.language, runtime))
            .unwrap_or_else(|| baseline.language.clone());
        let _ = write!(
            html,
            "<dt>Configured baseline</dt><dd>{}</dd>\n",
            escape_html(&baseline_label)
        );
        let _ = write!(
            html,
            "<dt>Comparison gating</dt><dd>Baseline comparisons require matching client/server OS, architecture, CPU core counts, regions, network type, and no more than {:.1}x drift in baseline RTT medians before publication-grade verdicts are emitted.</dd>\n",
            MAX_COMPARABLE_RTT_RATIO
        );
    }

    let mut phase_models = Vec::new();
    for result in &run.results {
        let phase_model = result.network.phase_model.trim();
        if !phase_model.is_empty()
            && !phase_models
                .iter()
                .any(|existing: &String| existing == phase_model)
        {
            phase_models.push(phase_model.to_string());
        }
    }
    if !phase_models.is_empty() {
        let _ = write!(
            html,
            "<dt>Observed phase models</dt><dd>{}</dd>\n",
            escape_html(&phase_models.join(", "))
        );
    }

    let observed_phases = collect_observed_phases(run);
    if !observed_phases.is_empty() {
        let _ = write!(
            html,
            "<dt>Observed lifecycle phases</dt><dd>{}</dd>\n",
            escape_html(&observed_phases.join(", "))
        );
    }
    let publication_readiness = if aggregation.publication_ready {
        "ready"
    } else {
        "rerun recommended"
    };
    let _ = write!(
        html,
        "<dt>Publication readiness</dt><dd>{}</dd>\n",
        publication_readiness
    );
    if aggregation.rerun_recommended {
        html.push_str("<dt>Recommendations</dt><dd><ul class=\"summary-notes\">\n");
        for recommendation in &aggregation.recommendations {
            let _ = write!(html, "<li>{}</li>\n", escape_html(recommendation));
        }
        html.push_str("</ul></dd>\n");
    }

    // Extract unique concurrency levels from results
    let mut conc_levels: Vec<u32> = run.results.iter().map(|r| r.concurrency).collect();
    conc_levels.sort();
    conc_levels.dedup();
    if !conc_levels.is_empty() {
        let _ = write!(
            html,
            "<dt>Concurrency levels</dt><dd>{}</dd>\n",
            conc_levels
                .iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let mut repeats: Vec<u32> = run.results.iter().map(|r| r.repeat_index).collect();
    repeats.sort();
    repeats.dedup();
    let _ = write!(
        html,
        "<dt>Repeat count</dt><dd>{}</dd>\n",
        repeats.len().max(1)
    );
    html.push_str(
        "<dt>Reporting policy</dt><dd>Warm and cold scenarios remain separate. Rankings use aggregated repeated launches instead of single best runs.</dd>\n",
    );
    html.push_str(
        "<dt>Comparison verdicts</dt><dd>Baseline verdicts use a default ±5% equivalence threshold plus 95% confidence-interval overlap on repeated launches.</dd>\n",
    );

    html.push_str("</dl>\n</section>\n\n");
}

// ---------------------------------------------------------------------------
// Inline CSS (dark theme, AletheDash aesthetic)
// ---------------------------------------------------------------------------

fn write_inline_css(html: &mut String) {
    html.push_str(
        ":root {\n\
  --bg: #0a0b0f;\n\
  --surface: #12141a;\n\
  --surface-2: #1a1d26;\n\
  --border: #2a2d35;\n\
  --text: #e5e7eb;\n\
  --text-dim: #8b8fa3;\n\
  --accent: #47bfff;\n\
  --accent-2: #cc5de8;\n\
  --gold: #fcc419;\n\
  --silver: #ced4da;\n\
  --bronze: #f4a460;\n\
  --green: #51cf66;\n\
  --red: #ff6b6b;\n\
  --font-mono: 'JetBrains Mono', 'Cascadia Code', ui-monospace, monospace;\n\
  --font-sans: system-ui, -apple-system, 'Segoe UI', sans-serif;\n\
  --radius: 6px;\n\
}\n\
\n\
*, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }\n\
html { scroll-behavior: smooth; }\n\
\n\
body {\n\
  font-family: var(--font-mono);\n\
  background: var(--bg);\n\
  color: var(--text);\n\
  line-height: 1.6;\n\
  font-size: 14px;\n\
}\n\
\n\
.page-header {\n\
  background: var(--surface);\n\
  border-bottom: 2px solid var(--accent);\n\
  padding: 2rem 2.5rem;\n\
}\n\
\n\
.page-header h1 {\n\
  font-size: 1.5rem;\n\
  font-weight: 700;\n\
  letter-spacing: -0.02em;\n\
  color: var(--accent);\n\
}\n\
\n\
.subtitle {\n\
  margin-top: 0.4rem;\n\
  color: var(--text-dim);\n\
  font-size: 0.8rem;\n\
}\n\
\n\
.card {\n\
  background: var(--surface);\n\
  border: 1px solid var(--border);\n\
  border-radius: var(--radius);\n\
  margin: 1.5rem 2.5rem;\n\
  padding: 1.75rem;\n\
}\n\
\n\
.card h2 {\n\
  font-size: 0.85rem;\n\
  font-weight: 700;\n\
  text-transform: uppercase;\n\
  letter-spacing: 0.08em;\n\
  color: var(--accent);\n\
  border-bottom: 1px solid var(--border);\n\
  padding-bottom: 0.75rem;\n\
  margin-bottom: 1.25rem;\n\
}\n\
\n\
.summary-grid {\n\
  display: grid;\n\
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));\n\
  gap: 1.5rem;\n\
}\n\
\n\
.summary-item h3 {\n\
  font-size: 0.75rem;\n\
  text-transform: uppercase;\n\
  letter-spacing: 0.06em;\n\
  color: var(--text-dim);\n\
  margin-bottom: 0.5rem;\n\
}\n\
\n\
.summary-item ol {\n\
  list-style: none;\n\
  counter-reset: rank;\n\
}\n\
\n\
.summary-notes {\n\
  list-style: disc;\n\
  padding-left: 1.2rem;\n\
  color: var(--text-dim);\n\
}\n\
\n\
.summary-notes li {\n\
  margin: 0.35rem 0;\n\
}\n\
\n\
.summary-item ol li {\n\
  counter-increment: rank;\n\
  padding: 0.3rem 0;\n\
  display: flex;\n\
  align-items: center;\n\
  gap: 0.5rem;\n\
}\n\
\n\
.summary-item ol li::before {\n\
  content: counter(rank) \".\";\n\
  color: var(--text-dim);\n\
  font-size: 0.8rem;\n\
  min-width: 1.5em;\n\
}\n\
\n\
.lang-tag {\n\
  color: var(--accent);\n\
  font-weight: 600;\n\
}\n\
\n\
.metric {\n\
  color: var(--text-dim);\n\
  font-size: 0.85rem;\n\
}\n\
\n\
table {\n\
  width: 100%;\n\
  border-collapse: collapse;\n\
  font-size: 0.82rem;\n\
}\n\
\n\
th {\n\
  background: var(--surface-2);\n\
  color: var(--text-dim);\n\
  padding: 0.6rem 0.9rem;\n\
  text-align: left;\n\
  font-weight: 600;\n\
  letter-spacing: 0.04em;\n\
  font-size: 0.75rem;\n\
  text-transform: uppercase;\n\
  white-space: nowrap;\n\
  border-bottom: 1px solid var(--border);\n\
  user-select: none;\n\
}\n\
\n\
th.sortable { cursor: pointer; }\n\
th.sortable:hover { color: var(--accent); }\n\
th.sort-asc::after { content: \" \\25B2\"; font-size: 0.6rem; }\n\
th.sort-desc::after { content: \" \\25BC\"; font-size: 0.6rem; }\n\
\n\
td {\n\
  padding: 0.5rem 0.9rem;\n\
  border-bottom: 1px solid var(--border);\n\
  vertical-align: middle;\n\
}\n\
\n\
tr:hover td { background: var(--surface-2); }\n\
\n\
.rank-gold { color: var(--gold); font-weight: 700; }\n\
.rank-silver { color: var(--silver); font-weight: 700; }\n\
.rank-bronze { color: var(--bronze); font-weight: 700; }\n\
\n\
.lang-dot {\n\
  display: inline-block;\n\
  width: 8px;\n\
  height: 8px;\n\
  border-radius: 50%;\n\
  margin-right: 0.5rem;\n\
  vertical-align: middle;\n\
}\n\
\n\
code {\n\
  font-family: var(--font-mono);\n\
  font-size: 0.82em;\n\
  background: var(--surface-2);\n\
  padding: 0.15em 0.4em;\n\
  border-radius: 3px;\n\
  border: 1px solid var(--border);\n\
}\n\
\n\
.method-grid {\n\
  display: grid;\n\
  grid-template-columns: 180px 1fr;\n\
  gap: 0.4rem 1rem;\n\
}\n\
\n\
.method-grid dt {\n\
  color: var(--text-dim);\n\
  font-size: 0.82rem;\n\
  font-weight: 600;\n\
}\n\
\n\
.method-grid dd {\n\
  font-size: 0.82rem;\n\
}\n\
\n\
.chart-caption {\n\
  margin-top: 0.85rem;\n\
  color: var(--text-dim);\n\
  font-size: 0.8rem;\n\
}\n\
\n\
footer {\n\
  text-align: center;\n\
  padding: 2rem;\n\
  font-size: 0.75rem;\n\
  color: var(--text-dim);\n\
  border-top: 1px solid var(--border);\n\
  margin-top: 1rem;\n\
}\n\
\n\
@media (max-width: 768px) {\n\
  .card { margin: 1rem; padding: 1rem; }\n\
  .page-header { padding: 1rem; }\n\
  .summary-grid { grid-template-columns: 1fr; }\n\
  .method-grid { grid-template-columns: 1fr; }\n\
}\n\
\n\
@media print {\n\
  body { background: #fff; color: #000; }\n\
  .card { border: 1px solid #ccc; break-inside: avoid; }\n\
  .page-header { background: #1a1a2e; color: #fff; -webkit-print-color-adjust: exact; }\n\
}\n",
    );
}

// ---------------------------------------------------------------------------
// Inline JS (table sorting)
// ---------------------------------------------------------------------------

fn write_inline_js(html: &mut String) {
    html.push_str(
        "(function() {\n\
  var table = document.getElementById('leaderboard');\n\
  if (!table) return;\n\
  var thead = table.querySelector('thead');\n\
  var tbody = table.querySelector('tbody');\n\
  var headers = thead.querySelectorAll('th.sortable');\n\
  var currentCol = -1;\n\
  var ascending = true;\n\
\n\
  headers.forEach(function(th) {\n\
    th.addEventListener('click', function() {\n\
      var col = parseInt(th.getAttribute('data-col'), 10);\n\
      if (currentCol === col) {\n\
        ascending = !ascending;\n\
      } else {\n\
        currentCol = col;\n\
        ascending = true;\n\
      }\n\
\n\
      headers.forEach(function(h) {\n\
        h.classList.remove('sort-asc', 'sort-desc');\n\
      });\n\
      th.classList.add(ascending ? 'sort-asc' : 'sort-desc');\n\
\n\
      var rows = Array.from(tbody.querySelectorAll('tr'));\n\
      rows.sort(function(a, b) {\n\
        var cellA = a.children[col];\n\
        var cellB = b.children[col];\n\
        var valA = cellA.getAttribute('data-sort') || cellA.textContent.trim();\n\
        var valB = cellB.getAttribute('data-sort') || cellB.textContent.trim();\n\
        var numA = parseFloat(valA);\n\
        var numB = parseFloat(valB);\n\
        if (!isNaN(numA) && !isNaN(numB)) {\n\
          return ascending ? numA - numB : numB - numA;\n\
        }\n\
        return ascending ? valA.localeCompare(valB) : valB.localeCompare(valA);\n\
      });\n\
\n\
      rows.forEach(function(row) { tbody.appendChild(row); });\n\
    });\n\
  });\n\
})();\n",
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(content.contains("AletheBench Report"));
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
        assert!(content.contains("AletheBench Report"));
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
        assert!(content.contains("AletheBench Report"));
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
        assert!(
            warm.throughput.ratio_summary.ci95_upper >= warm.throughput.ratio_summary.ci95_lower
        );
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
}
