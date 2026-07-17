//! Baseline comparisons: environment comparability gating and paired
//! candidate-vs-baseline throughput/latency verdicts.

use super::assembly::{scenario_results, CaseKey, GroupedCaseResults};
use super::stats::{cohens_d, confidence_intervals_overlap, summarise_metric};
use crate::types::{
    BenchmarkBaseline, BenchmarkCaseSummary, BenchmarkComparison, BenchmarkEnvironmentFingerprint,
    BenchmarkResult, ComparisonMetricSummary, ScenarioComparison,
};
use std::cmp::Ordering;
use std::collections::BTreeMap;

pub(super) const MAX_COMPARABLE_RTT_RATIO: f64 = 1.5;
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

pub(super) fn build_comparisons(
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
