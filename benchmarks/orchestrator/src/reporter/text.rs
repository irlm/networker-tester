//! Markdown and CSV rendering for the publication bundle.

use crate::types::{BenchmarkEnvironmentFingerprint, BenchmarkReport};
use std::fmt::Write as FmtWrite;

fn escape_csv(value: &str) -> String {
    let needs_quotes = value.contains([',', '"', '\n', '\r']);
    if needs_quotes {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

pub(super) fn render_results_csv(report: &BenchmarkReport) -> String {
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

pub(super) fn render_markdown_report(report: &BenchmarkReport) -> String {
    let mut md = String::new();
    let run = &report.run;
    let aggregation = &report.aggregation;

    let _ = writeln!(md, "# Networker Bench Publication Bundle");
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
