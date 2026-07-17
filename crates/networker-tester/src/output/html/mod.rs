/// Generate a self-contained HTML diagnostic report from a `TestRun`.
///
/// The report embeds a minimal inline CSS for offline viewing and optionally
/// adds a `<link rel="stylesheet">` for the external `report.css` file so
/// operators can customize the look without editing generated HTML.
use crate::{
    capture::PacketCaptureSummary,
    metrics::{
        attempt_payload_bytes, compute_stats, primary_metric_label, primary_metric_value, HostInfo,
        NetworkType, Protocol, RequestAttempt, TestRun,
    },
};
use chrono::DateTime;
use std::fmt::Write as FmtWrite;
use std::path::Path;

// ─────────────────────────────────────────────────────────────────────────────
// Submodules (mechanical split of the former single-file html.rs)
// ─────────────────────────────────────────────────────────────────────────────

mod charts;
mod protocol_sections;
mod render_multi;
mod run_sections;
mod tables;

pub use render_multi::render_multi;

use charts::*;
use protocol_sections::*;
use render_multi::*;
use run_sections::*;
use tables::*;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers – cloud hostname detection & short display names
// ─────────────────────────────────────────────────────────────────────────────

/// Returns `true` for hostnames that are cloud-provider internal names and
/// should NOT be shown to the user (e.g. AWS `ip-172-31-78-2`).
fn is_cloud_internal_hostname(hostname: &str) -> bool {
    // AWS default hostnames: ip-10-x-x-x, ip-172-x-x-x
    if hostname.starts_with("ip-")
        && hostname
            .chars()
            .skip(3)
            .all(|c| c.is_ascii_digit() || c == '-')
    {
        return true;
    }
    false
}

/// Derive a short OS label from the full OS string.
fn os_short_label(os: &str) -> &'static str {
    if os.contains("Windows") {
        "Windows"
    } else if os.contains("Ubuntu") {
        "Ubuntu"
    } else if os.contains("Debian") {
        "Debian"
    } else {
        "Linux"
    }
}

/// Derive a cloud provider tag from the region string.
fn provider_from_region(region: &str) -> Option<&'static str> {
    if region.starts_with("azure/") {
        Some("Azure")
    } else if region.starts_with("aws/") {
        Some("AWS")
    } else if region.starts_with("gcp/") {
        Some("GCP")
    } else {
        None
    }
}

/// Build a short display name for a target from its server info.
///
/// Returns names like "Azure Ubuntu", "AWS Windows", "GCP Ubuntu", "my-vm",
/// or falls back to `fallback` when no server info is available.
fn derive_display_name(info: Option<&HostInfo>, fallback: &str) -> String {
    let Some(s) = info else {
        return fallback.to_string();
    };
    let hostname = s.hostname.as_deref().unwrap_or("");
    let os = s.os_version.as_deref().unwrap_or(&s.os);
    let provider = s.region.as_deref().and_then(provider_from_region);

    if hostname.is_empty() || hostname == "unknown" || is_cloud_internal_hostname(hostname) {
        let os_short = os_short_label(os);
        match provider {
            Some(p) => format!("{p} {os_short}"),
            None => os_short.to_string(),
        }
    } else {
        hostname.to_string()
    }
}

/// Build short names for each target in a multi-target report.
///
/// If names collide (e.g. two "AWS Ubuntu"), appends a numeric suffix.
fn build_target_short_names(runs: &[TestRun]) -> Vec<String> {
    let raw: Vec<String> = runs
        .iter()
        .enumerate()
        .map(|(i, r)| derive_display_name(r.server_info.as_ref(), &format!("Target {}", i + 1)))
        .collect();

    // Disambiguate duplicates by appending #2, #3, etc.
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut result: Vec<String> = Vec::with_capacity(raw.len());
    // First pass: count occurrences
    for name in &raw {
        *counts.entry(name.clone()).or_insert(0) += 1;
    }
    // Second pass: assign suffixes where needed
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for name in &raw {
        if counts[name] > 1 {
            let seq = seen.entry(name.clone()).or_insert(0);
            *seq += 1;
            result.push(format!("{name} #{seq}"));
        } else {
            result.push(name.clone());
        }
    }
    result
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

pub fn save(
    run: &TestRun,
    path: &Path,
    css_href: Option<&str>,
    packet_capture: Option<&PacketCaptureSummary>,
) -> anyhow::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let html = render(run, css_href, packet_capture);
    std::fs::write(path, html)?;
    Ok(())
}

/// Save a combined HTML report for one or more `TestRun`s.
///
/// When `runs.len() == 1` the output is identical to `save()`.
/// When `runs.len() > 1` the report includes a cross-target summary and
/// protocol-comparison table followed by per-target collapsible sections.
pub fn save_multi(
    runs: &[TestRun],
    path: &Path,
    css_href: Option<&str>,
    packet_capture: Option<&PacketCaptureSummary>,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    std::fs::write(path, render_multi(runs, css_href, packet_capture))?;
    Ok(())
}

pub fn render(
    run: &TestRun,
    css_href: Option<&str>,
    packet_capture: Option<&PacketCaptureSummary>,
) -> String {
    let mut out = String::with_capacity(64 * 1024);
    write_html_head(&run.target_url, css_href, &mut out);
    write_run_sections(run, &mut out);
    write_packet_capture_section(packet_capture, &mut out);
    write_html_footer(run.finished_at.unwrap_or(run.started_at), &mut out);
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// Inline CSS (minimal, works offline; external CSS can override)
// ─────────────────────────────────────────────────────────────────────────────

/// Public alias so `main.rs` can write a fallback CSS file.
pub const FALLBACK_CSS: &str = INLINE_CSS;

const INLINE_CSS: &str = r#"
  *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0 }
  body { font-family: system-ui, sans-serif; background: #f0f2f5; color: #1a1a2e; line-height: 1.5; }
  .page-header { background: #1a1a2e; color: #fff; padding: 1.5rem 2rem; }
  .page-header h1 { font-size: 1.6rem; }
  .subtitle { opacity: .7; font-size: .9rem; margin-top: .25rem; }
  .card { background: #fff; border-radius: 8px; box-shadow: 0 1px 4px rgba(0,0,0,.1);
          margin: 1.5rem 2rem; padding: 1.5rem; }
  .card h2 { font-size: 1.1rem; margin-bottom: 1rem; color: #1a1a2e; border-bottom: 1px solid #eee; padding-bottom: .5rem; }
  dl.summary-grid { display: grid; grid-template-columns: 160px 1fr; gap: .4rem .75rem; }
  dt { font-weight: 600; color: #555; }
  table { width: 100%; border-collapse: collapse; font-size: .85rem; }
  th { background: #1a1a2e; color: #fff; padding: .5rem .75rem; text-align: left; font-weight: 600; }
  td { padding: .45rem .75rem; border-bottom: 1px solid #f0f2f5; vertical-align: top; }
  tr:last-child td { border-bottom: none; }
  tr:hover td { background: #f7f9fb; }
  tr.row-err td { background: #fff5f5; }
  .ok   { color: #2e7d32; font-weight: 600; }
  .warn { color: #e65100; font-weight: 600; }
  .err  { color: #c62828; font-weight: 600; }
  code  { background: #f0f2f5; padding: .1em .35em; border-radius: 3px; font-size: .85em; }
  a     { color: #1565c0; }
  footer { text-align: center; padding: 2rem; font-size: .8rem; color: #888; }
  .error-section h2 { color: #c62828; }
  details{margin-bottom:.6rem}
  details summary{cursor:pointer;padding:.45rem .75rem;background:#f0f0f0;
    border-radius:4px;display:flex;align-items:center;gap:.8rem;
    list-style:none;user-select:none;font-size:.88rem}
  details summary::-webkit-details-marker{display:none}
  details summary::before{content:"▶";font-size:.7rem;flex-shrink:0}
  details[open]>summary::before{content:"▼"}
  .grp-lbl{font-weight:600;flex:1}
  .note{font-size:.85rem;color:#666;font-style:italic;margin:.6rem 0 0}
  .grp-meta{opacity:.7;font-family:monospace;font-size:.82rem}
  details table{margin-top:.5rem}
  .charts-grid{display:flex;flex-wrap:wrap;gap:1.5rem;align-items:flex-start;margin-top:.5rem}
  .analysis{background:#f8f9fa;border-left:3px solid #4e79a7;padding:.8rem 1.2rem;
            border-radius:0 6px 6px 0;margin-top:1rem}
  .analysis h3{font-size:.95rem;margin-bottom:.5rem;color:#1a1a2e}
  .analysis ul{list-style:none;padding:0}
  .analysis li{padding:.2rem 0;font-size:.86rem;line-height:1.45}
  .analysis li::before{content:"\2022";margin-right:.5rem;color:#4e79a7;font-weight:bold}
  .multi-target-details{margin:1.5rem 2rem;padding:0}
  .multi-target-details>summary{cursor:pointer;font-size:1rem;padding:.6rem 1rem;
    background:#f0f2f5;border-radius:6px 6px 0 0;color:#1a1a2e;list-style:none;
    font-weight:600;display:flex;align-items:center;gap:.5rem}
  .multi-target-details>summary::-webkit-details-marker{display:none}
  .multi-target-details>summary::before{content:"▶ ";font-size:.8em;flex-shrink:0}
  .multi-target-details[open]>summary::before{content:"▼ "}
  .multi-target-details>.page-header{border-radius:0;margin:0}
  .diff-fast{color:#2e7d32;font-size:.8em;margin-left:.3em}
  .diff-slow{color:#c62828;font-size:.8em;margin-left:.3em}
  .target-charts-grid{display:grid;grid-template-columns:repeat(2,1fr);gap:1.5rem;margin-top:.5rem}
  .target-chart-cell{background:#fafbfc;border:1px solid #e8e8e8;border-radius:6px;padding:1rem}
  .target-chart-cell .analysis{margin-top:.5rem}
  .target-server-info{font-size:.82rem;color:#555;padding:.4rem .6rem;background:#f0f2f5;
    border-radius:4px;margin-bottom:.6rem;line-height:1.4}
  .target-group-header{grid-column:1/-1;font-size:1rem;font-weight:600;color:#1a1a2e;
    padding:.3rem 0;margin-top:.5rem;border-bottom:2px solid #e8e8e8}
  .target-group-comparison{grid-column:1/-1;margin:0}
  @media(max-width:900px){.target-charts-grid{grid-template-columns:1fr}}
"#;

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
