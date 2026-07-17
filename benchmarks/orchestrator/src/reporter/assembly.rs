//! Report assembly: grouping raw results into per-case scenario summaries,
//! publication-readiness recommendations, and report building/loading.

use super::comparison::build_comparisons;
use super::stats::{summarise_metric, REPORT_CONFIDENCE_LEVEL};
use crate::types::{
    BenchmarkCaseSummary, BenchmarkComparison, BenchmarkReport, BenchmarkResult, BenchmarkRun,
    ReportAggregation, ScenarioSummary,
};
use anyhow::{Context, Result};
use chrono::Utc;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct CaseKey {
    pub(super) language: String,
    pub(super) runtime: String,
    pub(super) concurrency: u32,
}

pub(super) fn build_report(run: &BenchmarkRun) -> BenchmarkReport {
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

#[cfg(test)] // test-only until wired into the runtime (clippy dead_code sweep)
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

pub(super) fn collect_observed_phases(run: &BenchmarkRun) -> Vec<String> {
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

pub(super) type GroupedCaseResults<'a> = BTreeMap<CaseKey, Vec<&'a BenchmarkResult>>;

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

pub(super) fn scenario_results<'a>(
    results: &[&'a BenchmarkResult],
    scenario: &str,
) -> Vec<&'a BenchmarkResult> {
    results
        .iter()
        .copied()
        .filter(|result| result_scenario(result) == scenario)
        .collect()
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

#[cfg(test)] // test-only until wired into the runtime (clippy dead_code sweep)
pub(crate) fn summarise_cases(run: &BenchmarkRun) -> Vec<BenchmarkCaseSummary> {
    summarise_results(&run.results)
}
