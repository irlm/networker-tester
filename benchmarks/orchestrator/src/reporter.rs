use crate::types::BenchmarkRun;
use anyhow::Result;
use std::path::Path;

/// Write benchmark results as JSON.
///
/// Stub — will be implemented in Phase 4.
pub fn generate_json(run: &BenchmarkRun, output: &Path) -> Result<()> {
    let json = serde_json::to_string_pretty(run)?;
    std::fs::write(output, json)?;
    tracing::info!("Wrote JSON report to {}", output.display());
    Ok(())
}

/// Generate an HTML comparison report.
///
/// Stub — will be implemented in Phase 4.
pub fn generate_html(_run: &BenchmarkRun, output: &Path) -> Result<()> {
    let html = "<html><body><h1>AletheBench Report</h1><p>Coming soon.</p></body></html>";
    std::fs::write(output, html)?;
    tracing::info!("Wrote HTML report to {}", output.display());
    Ok(())
}
