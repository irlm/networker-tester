//! Benchmark report generation, split by concern: statistical primitives
//! (`stats`), case-summary assembly (`assembly`), baseline comparisons
//! (`comparison`), Markdown/CSV rendering (`text`), SVG charts (`charts`),
//! and HTML rendering (`html`). The JSON/bundle entry points live here.

mod assembly;
mod charts;
mod comparison;
mod html;
mod stats;
#[cfg(test)]
mod tests;
mod text;

pub use self::html::generate_html;

#[cfg(test)]
pub(crate) use self::assembly::report_from_run;
pub(crate) use self::assembly::{load_report, summarise_results};
pub(crate) use self::comparison::environment_comparability_notes;

use self::assembly::build_report;
use self::text::{render_markdown_report, render_results_csv};
use crate::types::{BenchmarkReport, BenchmarkRun};
use anyhow::{Context, Result};
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
    std::fs::write(&md_path, render_markdown_report(report))
        .with_context(|| format!("writing {}", md_path.display()))?;
    std::fs::write(&csv_path, render_results_csv(report))
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
