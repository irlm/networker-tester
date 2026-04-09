use crate::types::{BenchmarkResult, BenchmarkRun};
use anyhow::Result;
use std::fmt::Write as FmtWrite;
use std::path::Path;

/// Write benchmark results as JSON.
pub fn generate_json(run: &BenchmarkRun, output: &Path) -> Result<()> {
    let json = serde_json::to_string_pretty(run)?;
    std::fs::write(output, json)?;
    tracing::info!("Wrote JSON report to {}", output.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Classify a result as cold or warm based on total_requests threshold.
fn is_cold(r: &BenchmarkResult) -> bool {
    r.phase == "cold" || r.network.total_requests <= 100
}

/// Aggregate results per language using warm/cold repeat sets rather than
/// cherry-picking the single best run.
struct LangSummary {
    language: String,
    runtime: String,
    warm: BenchmarkResult,
    cold: Option<BenchmarkResult>,
    repeat_count: usize,
}

fn summarise_by_language(run: &BenchmarkRun) -> Vec<LangSummary> {
    use std::collections::BTreeMap;

    let mut warm_map: BTreeMap<String, Vec<&BenchmarkResult>> = BTreeMap::new();
    let mut cold_map: BTreeMap<String, Vec<&BenchmarkResult>> = BTreeMap::new();

    for r in &run.results {
        if is_cold(r) {
            cold_map.entry(r.language.clone()).or_default().push(r);
        } else {
            warm_map.entry(r.language.clone()).or_default().push(r);
        }
    }

    let mut summaries: Vec<LangSummary> = Vec::new();

    for (lang, warm_results) in &warm_map {
        let aggregate_warm = aggregate_results(warm_results);

        let aggregate_cold = cold_map
            .get(lang)
            .map(|cs| aggregate_results(cs));

        summaries.push(LangSummary {
            language: lang.clone(),
            runtime: aggregate_warm.runtime.clone(),
            warm: aggregate_warm,
            cold: aggregate_cold,
            repeat_count: warm_results.len(),
        });
    }

    // Include languages that only have cold results
    for (lang, cold_results) in &cold_map {
        if !warm_map.contains_key(lang) {
            let aggregate_cold = aggregate_results(cold_results);
            summaries.push(LangSummary {
                language: lang.clone(),
                runtime: aggregate_cold.runtime.clone(),
                warm: aggregate_cold.clone(),
                cold: Some(aggregate_cold),
                repeat_count: cold_results.len(),
            });
        }
    }

    // Sort by aggregate warm RPS descending
    summaries.sort_by(|a, b| b.warm.network.rps.partial_cmp(&a.warm.network.rps).unwrap());
    summaries
}

fn aggregate_results(results: &[&BenchmarkResult]) -> BenchmarkResult {
    let first = (*results[0]).clone();
    let n = results.len() as f64;

    let sum = |f: fn(&BenchmarkResult) -> f64| results.iter().map(|r| f(r)).sum::<f64>();
    let sum_u64 = |f: fn(&BenchmarkResult) -> u64| results.iter().map(|r| f(r)).sum::<u64>();

    let mut aggregated = first.clone();
    aggregated.network.rps = sum(|r| r.network.rps) / n;
    aggregated.network.latency_mean_ms = sum(|r| r.network.latency_mean_ms) / n;
    aggregated.network.latency_p50_ms = sum(|r| r.network.latency_p50_ms) / n;
    aggregated.network.latency_p99_ms = sum(|r| r.network.latency_p99_ms) / n;
    aggregated.network.latency_p999_ms = sum(|r| r.network.latency_p999_ms) / n;
    aggregated.network.latency_max_ms = results
        .iter()
        .map(|r| r.network.latency_max_ms)
        .fold(0.0_f64, f64::max);
    aggregated.network.bytes_transferred = sum_u64(|r| r.network.bytes_transferred) / results.len() as u64;
    aggregated.network.error_count = sum_u64(|r| r.network.error_count);
    aggregated.network.total_requests = sum_u64(|r| r.network.total_requests);

    aggregated.resources.peak_rss_bytes = results
        .iter()
        .map(|r| r.resources.peak_rss_bytes)
        .max()
        .unwrap_or(0);
    aggregated.resources.avg_cpu_fraction = sum(|r| r.resources.avg_cpu_fraction) / n;
    aggregated.resources.peak_cpu_fraction = results
        .iter()
        .map(|r| r.resources.peak_cpu_fraction)
        .fold(0.0_f64, f64::max);
    aggregated.resources.peak_open_fds = results
        .iter()
        .map(|r| r.resources.peak_open_fds)
        .max()
        .unwrap_or(0);

    aggregated.startup.time_to_first_response_ms = sum(|r| r.startup.time_to_first_response_ms) / n;
    aggregated.startup.time_to_ready_ms = sum(|r| r.startup.time_to_ready_ms) / n;

    aggregated.result_validity.state = if results.iter().any(|r| r.result_validity.state == crate::types::ValidityState::Invalid) {
        crate::types::ValidityState::Invalid
    } else if results.iter().any(|r| r.result_validity.state == crate::types::ValidityState::Degraded) {
        crate::types::ValidityState::Degraded
    } else {
        crate::types::ValidityState::Valid
    };
    aggregated.result_validity.is_valid = aggregated.result_validity.state == crate::types::ValidityState::Valid;
    let mut warnings: Vec<String> = results
        .iter()
        .flat_map(|r| r.result_validity.warnings.clone())
        .collect();
    warnings.sort();
    warnings.dedup();
    aggregated.result_validity.warnings = warnings;
    aggregated
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

/// Build a grouped bar chart comparing cold vs warm RPS per language.
fn svg_cold_warm_chart(summaries: &[LangSummary]) -> String {
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
            let cold_rps = s.cold.as_ref().map_or(0.0, |c| c.network.rps);
            vec![s.warm.network.rps, cold_rps]
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
        let _ = write!(
            svg,
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"#2a2d35\" stroke-width=\"1\"/>",
            margin_l, y, margin_l + plot_w, y
        );
        let _ = write!(
            svg,
            "<text x=\"{}\" y=\"{}\" fill=\"#8b8fa3\" text-anchor=\"end\" \
             dominant-baseline=\"middle\">{:.0}</text>",
            margin_l - 8.0, y, val
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
        let cold_rps = s.cold.as_ref().map_or(0.0, |c| c.network.rps);

        // Cold bar
        let cold_h = (cold_rps / max_rps) * plot_h;
        let cold_y = margin_t + plot_h - cold_h;
        let _ = write!(
            svg,
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" \
             fill=\"#ff6b6b\" opacity=\"0.7\" rx=\"2\">\
             <title>Cold: {:.0} req/s</title></rect>",
            gx - bar_w - 2.0, cold_y, bar_w, cold_h, cold_rps
        );

        // Warm bar
        let warm_h = (s.warm.network.rps / max_rps) * plot_h;
        let warm_y = margin_t + plot_h - warm_h;
        let _ = write!(
            svg,
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" \
             fill=\"#47bfff\" opacity=\"0.85\" rx=\"2\">\
             <title>Warm: {:.0} req/s</title></rect>",
            gx + 2.0, warm_y, bar_w, warm_h, s.warm.network.rps
        );

        // X-axis label
        let label_y = margin_t + plot_h + 16.0;
        let _ = write!(
            svg,
            "<text x=\"{}\" y=\"{}\" fill=\"#e5e7eb\" text-anchor=\"middle\" \
             font-size=\"11\" transform=\"rotate(-35, {}, {})\">{}</text>",
            gx, label_y, gx, label_y, escape_html(&s.language)
        );
    }

    // Legend
    let ly = chart_h - 10.0;
    let _ = write!(
        svg,
        "<rect x=\"{}\" y=\"{}\" width=\"12\" height=\"12\" \
         fill=\"#ff6b6b\" opacity=\"0.7\" rx=\"2\"/>",
        margin_l, ly - 10.0
    );
    let _ = write!(
        svg,
        "<text x=\"{}\" y=\"{}\" fill=\"#e5e7eb\" font-size=\"11\">Cold</text>",
        margin_l + 16.0, ly
    );
    let _ = write!(
        svg,
        "<rect x=\"{}\" y=\"{}\" width=\"12\" height=\"12\" \
         fill=\"#47bfff\" opacity=\"0.85\" rx=\"2\"/>",
        margin_l + 70.0, ly - 10.0
    );
    let _ = write!(
        svg,
        "<text x=\"{}\" y=\"{}\" fill=\"#e5e7eb\" font-size=\"11\">Warm</text>",
        margin_l + 86.0, ly
    );

    svg.push_str("</svg>");
    svg
}

/// Build a grouped bar chart of p50/p95/p99 latency per language.
fn svg_latency_chart(summaries: &[LangSummary]) -> String {
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
        .map(|s| s.warm.network.latency_p99_ms)
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
        let _ = write!(
            svg,
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"#2a2d35\" stroke-width=\"1\"/>",
            margin_l, y, margin_l + plot_w, y
        );
        let _ = write!(
            svg,
            "<text x=\"{}\" y=\"{}\" fill=\"#8b8fa3\" text-anchor=\"end\" \
             dominant-baseline=\"middle\">{:.1}</text>",
            margin_l - 8.0, y, val
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
        let vals = [
            s.warm.network.latency_p50_ms,
            (s.warm.network.latency_p50_ms + s.warm.network.latency_p99_ms) / 2.0,
            s.warm.network.latency_p99_ms,
        ];
        let labels = ["p50", "p95", "p99"];

        for (j, (&val, &color)) in vals.iter().zip(percentile_colors.iter()).enumerate() {
            let bx = gx + (j as f64 - 1.0) * (bar_w + 3.0);
            let h = (val / max_lat) * plot_h;
            let y = margin_t + plot_h - h;
            let _ = write!(
                svg,
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" \
                 fill=\"{}\" opacity=\"0.8\" rx=\"2\">\
                 <title>{}: {:.2}ms</title></rect>",
                bx - bar_w / 2.0, y, bar_w, h, color, labels[j], val
            );
        }

        let label_y = margin_t + plot_h + 16.0;
        let _ = write!(
            svg,
            "<text x=\"{}\" y=\"{}\" fill=\"#e5e7eb\" text-anchor=\"middle\" \
             font-size=\"11\" transform=\"rotate(-35, {}, {})\">{}</text>",
            gx, label_y, gx, label_y, escape_html(&s.language)
        );
    }

    // Legend
    let ly = chart_h - 10.0;
    for (j, (&color, label)) in percentile_colors.iter().zip(["p50", "p95", "p99"]).enumerate() {
        let lx = margin_l + j as f64 * 70.0;
        let _ = write!(
            svg,
            "<rect x=\"{}\" y=\"{}\" width=\"12\" height=\"12\" \
             fill=\"{}\" opacity=\"0.8\" rx=\"2\"/>",
            lx, ly - 10.0, color
        );
        let _ = write!(
            svg,
            "<text x=\"{}\" y=\"{}\" fill=\"#e5e7eb\" font-size=\"11\">{}</text>",
            lx + 16.0, ly, label
        );
    }

    svg.push_str("</svg>");
    svg
}

/// Build a horizontal bar chart for CPU% and Memory per language.
fn svg_resource_chart(summaries: &[LangSummary]) -> String {
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
        .map(|s| s.warm.resources.peak_rss_bytes)
        .max()
        .unwrap_or(1) as f64
        * 1.15;
    let max_cpu = summaries
        .iter()
        .map(|s| s.warm.resources.avg_cpu_fraction)
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

        // Language label
        let _ = write!(
            svg,
            "<text x=\"{}\" y=\"{}\" fill=\"#e5e7eb\" text-anchor=\"end\" \
             dominant-baseline=\"middle\" font-size=\"12\">{}</text>",
            margin_l - 10.0, gy + bar_h / 2.0, escape_html(&s.language)
        );

        // CPU bar
        let cpu_w = (s.warm.resources.avg_cpu_fraction / max_cpu) * plot_w;
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
            margin_l + cpu_w + 5.0, gy + bar_h / 2.0,
            s.warm.resources.avg_cpu_fraction * 100.0
        );

        // Memory bar
        let mem_y = gy + bar_h + 4.0;
        let mem_w = (s.warm.resources.peak_rss_bytes as f64 / max_mem) * plot_w;
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
            margin_l + mem_w + 5.0, mem_y + bar_h / 2.0,
            format_bytes(s.warm.resources.peak_rss_bytes)
        );
    }

    // Legend
    let ly = chart_h - 8.0;
    let _ = write!(
        svg,
        "<rect x=\"{}\" y=\"{}\" width=\"12\" height=\"12\" \
         fill=\"#47bfff\" opacity=\"0.8\" rx=\"2\"/>",
        margin_l, ly - 10.0
    );
    let _ = write!(
        svg,
        "<text x=\"{}\" y=\"{}\" fill=\"#e5e7eb\" font-size=\"11\">Avg CPU</text>",
        margin_l + 16.0, ly
    );
    let _ = write!(
        svg,
        "<rect x=\"{}\" y=\"{}\" width=\"12\" height=\"12\" \
         fill=\"#cc5de8\" opacity=\"0.7\" rx=\"2\"/>",
        margin_l + 100.0, ly - 10.0
    );
    let _ = write!(
        svg,
        "<text x=\"{}\" y=\"{}\" fill=\"#e5e7eb\" font-size=\"11\">Peak Memory</text>",
        margin_l + 116.0, ly
    );

    svg.push_str("</svg>");
    svg
}

// ---------------------------------------------------------------------------
// HTML generation
// ---------------------------------------------------------------------------

/// Generate a standalone HTML comparison report with inline CSS, JS, and SVG charts.
pub fn generate_html(run: &BenchmarkRun, output: &Path) -> Result<()> {
    let summaries = summarise_by_language(run);

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
        "<div class=\"subtitle\">{} &middot; Duration: {} &middot; {} languages &middot; {} data points</div>\n",
        date_str, duration_str, summaries.len(), run.results.len()
    );
    html.push_str("</header>\n\n");

    // -- Executive Summary --
    write_executive_summary(&mut html, &summaries);

    // -- Leaderboard Table --
    write_leaderboard(&mut html, &summaries);

    // -- Cold vs Warm Chart --
    html.push_str("<section class=\"card\">\n<h2>Cold vs Warm Throughput</h2>\n");
    html.push_str(&svg_cold_warm_chart(&summaries));
    html.push_str("\n</section>\n\n");

    // -- Latency Chart --
    html.push_str("<section class=\"card\">\n<h2>Latency Distribution (Warm)</h2>\n");
    html.push_str(&svg_latency_chart(&summaries));
    html.push_str("\n</section>\n\n");

    // -- Resource Usage Chart --
    html.push_str("<section class=\"card\">\n<h2>Resource Usage</h2>\n");
    html.push_str(&svg_resource_chart(&summaries));
    html.push_str("\n</section>\n\n");

    // -- Methodology --
    write_methodology(&mut html, run);

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

fn write_executive_summary(html: &mut String, summaries: &[LangSummary]) {
    html.push_str("<section class=\"card\">\n<h2>Executive Summary</h2>\n");
    html.push_str("<div class=\"summary-grid\">\n");

    // Top 3 by warm throughput
    html.push_str("<div class=\"summary-item\">\n<h3>Highest Throughput (warm)</h3>\n<ol>\n");
    for s in summaries.iter().take(3) {
        let _ = write!(
            html,
            "<li><span class=\"lang-tag\">{}</span> <span class=\"metric\">{:.0} req/s</span></li>\n",
            escape_html(&s.language), s.warm.network.rps
        );
    }
    html.push_str("</ol>\n</div>\n");

    // Lowest p99 latency
    let mut by_p99: Vec<&LangSummary> = summaries.iter().collect();
    by_p99.sort_by(|a, b| {
        a.warm.network.latency_p99_ms
            .partial_cmp(&b.warm.network.latency_p99_ms)
            .unwrap()
    });
    html.push_str("<div class=\"summary-item\">\n<h3>Lowest p99 Latency (warm)</h3>\n<ol>\n");
    for s in by_p99.iter().take(3) {
        let _ = write!(
            html,
            "<li><span class=\"lang-tag\">{}</span> <span class=\"metric\">{}</span></li>\n",
            escape_html(&s.language),
            format_duration_ms(s.warm.network.latency_p99_ms)
        );
    }
    html.push_str("</ol>\n</div>\n");

    // Lowest memory
    let mut by_mem: Vec<&LangSummary> = summaries.iter().collect();
    by_mem.sort_by(|a, b| {
        a.warm.resources.peak_rss_bytes
            .cmp(&b.warm.resources.peak_rss_bytes)
    });
    html.push_str("<div class=\"summary-item\">\n<h3>Lowest Memory</h3>\n<ol>\n");
    for s in by_mem.iter().take(3) {
        let _ = write!(
            html,
            "<li><span class=\"lang-tag\">{}</span> <span class=\"metric\">{}</span></li>\n",
            escape_html(&s.language),
            format_bytes(s.warm.resources.peak_rss_bytes)
        );
    }
    html.push_str("</ol>\n</div>\n");

    html.push_str("</div>\n</section>\n\n");
}

fn write_leaderboard(html: &mut String, summaries: &[LangSummary]) {
    html.push_str("<section class=\"card\">\n<h2>Leaderboard</h2>\n");
    html.push_str("<table id=\"leaderboard\">\n<thead>\n<tr>");

    let headers = [
        "Rank", "Language", "Runtime", "req/s (warm)", "p99 (warm)",
        "CPU %", "Memory", "Cold Start", "Binary Size",
    ];
    for (i, h) in headers.iter().enumerate() {
        let _ = write!(
            html,
            "<th data-col=\"{}\" class=\"sortable\">{}</th>",
            i, h
        );
    }
    html.push_str("</tr>\n</thead>\n<tbody>\n");

    for (rank, s) in summaries.iter().enumerate() {
        let cpu_pct = s.warm.resources.avg_cpu_fraction * 100.0;
        let cold_start = s
            .cold
            .as_ref()
            .map(|c| format_duration_ms(c.startup.time_to_first_response_ms))
            .unwrap_or_else(|| "\u{2014}".into());

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
             <td data-sort=\"{}\">{:.0}</td>\
             <td data-sort=\"{}\">{:.2}</td>\
             <td data-sort=\"{}\">{:.1}%</td>\
             <td data-sort=\"{}\">{}</td>\
             <td>{}</td>\
             <td data-sort=\"{}\">{}</td>\
             </tr>\n",
            rank_class,
            rank + 1,
            lang_color(rank),
            escape_html(&s.language),
            escape_html(&s.runtime),
            s.warm.network.rps,
            s.warm.network.rps,
            s.warm.network.latency_p99_ms,
            s.warm.network.latency_p99_ms,
            cpu_pct,
            cpu_pct,
            s.warm.resources.peak_rss_bytes,
            format_bytes(s.warm.resources.peak_rss_bytes),
            cold_start,
            s.warm.binary.size_bytes,
            format_bytes(s.warm.binary.size_bytes),
        );
    }

    html.push_str("</tbody>\n</table>\n</section>\n\n");
}

fn write_methodology(html: &mut String, run: &BenchmarkRun) {
    html.push_str("<section class=\"card\">\n<h2>Methodology</h2>\n");
    html.push_str("<dl class=\"method-grid\">\n");

    let _ = write!(
        html,
        "<dt>Run ID</dt><dd><code>{}</code></dd>\n",
        run.id
    );
    let _ = write!(
        html,
        "<dt>Config</dt><dd><code>{}</code></dd>\n",
        escape_html(&run.config_path)
    );
    let _ = write!(
        html,
        "<dt>Run purpose</dt><dd><code>{:?}</code></dd>\n",
        run.run_purpose
    );
    let _ = write!(
        html,
        "<dt>Benchmark family</dt><dd><code>{:?}</code></dd>\n",
        run.benchmark_family
    );
    let _ = write!(
        html,
        "<dt>Benchmark intent</dt><dd><code>{:?}</code></dd>\n",
        run.benchmark_intent
    );
    let _ = write!(
        html,
        "<dt>Scenario</dt><dd>connection=<code>{:?}</code>, browser=<code>{:?}</code>, load=<code>{:?}</code>, model=<code>{:?}</code>, topology=<code>{:?}</code></dd>\n",
        run.scenario.connection_state,
        run.scenario.browser_view,
        run.scenario.load_state,
        run.scenario.load_model,
        run.scenario.topology_class
    );
    let _ = write!(
        html,
        "<dt>Workload</dt><dd>request=<code>{}</code>, response=<code>{}</code>, reuse=<code>{}</code>, arrival=<code>{}</code>, think_time_ms={}</dd>\n",
        escape_html(&run.workload_profile.request_size_bytes),
        escape_html(&run.workload_profile.response_size_bytes),
        escape_html(&run.workload_profile.connection_reuse),
        escape_html(&run.workload_profile.arrival_pattern),
        run.workload_profile.think_time_ms
    );
    if let Some(template) = &run.workload_template {
        let _ = write!(
            html,
            "<dt>Workload template</dt><dd><code>{}:{}</code></dd>\n",
            escape_html(&template.name),
            escape_html(&template.version)
        );
    }
    if let Some(offered) = &run.offered_load {
        if let Some(qps) = offered.qps {
            let _ = write!(html, "<dt>Offered load</dt><dd>{:.2} qps</dd>\n", qps);
        }
    }
    let _ = write!(
        html,
        "<dt>Validity</dt><dd>env=<code>{:?}</code>, protocol_verified={}, client_risk=<code>{:?}</code>, topology=<code>{:?}</code>, sample_size={}, method=<code>{}</code></dd>\n",
        run.validity_manifest.environment_stability,
        run.validity_manifest.protocol_verified,
        run.validity_manifest.client_saturation_risk,
        run.validity_manifest.topology_control,
        run.validity_manifest.sample_size,
        escape_html(&run.validity_manifest.measurement_method)
    );
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
    let _ = write!(
        html,
        "<dt>Data points</dt><dd>{}</dd>\n",
        run.results.len()
    );

    // Extract unique concurrency levels from results
    let mut conc_levels: Vec<u32> = run.results.iter().map(|r| r.concurrency).collect();
    conc_levels.sort();
    conc_levels.dedup();
    if !conc_levels.is_empty() {
        let _ = write!(
            html,
            "<dt>Concurrency levels</dt><dd>{}</dd>\n",
            conc_levels.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", ")
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

    html.push_str("</dl>\n</section>\n\n");

    html.push_str("<section class=\"card\">\n<h2>Methodology Caveats</h2>\n<ul>");
    html.push_str("<li>No automatic cross-family composite score is used in this report.</li>");
    html.push_str("<li>Only runs with compatible family, intent, workload, and scenario should be compared side-by-side.</li>");
    if !run.validity_manifest.protocol_verified {
        html.push_str("<li>Protocol verification is not marked complete, so protocol-specific claims should be treated cautiously.</li>");
    }
    if run.validity_manifest.sample_size < 5 {
        html.push_str("<li>Sample size is below the spec baseline for stronger comparative claims.</li>");
    }
    if run.scenario.protocol_negotiated.is_none() {
        html.push_str("<li>Negotiated protocol was not persisted by the runner for this run.</li>");
    }
    html.push_str("</ul>\n</section>\n\n");
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
}\n"
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
})();\n"
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

    fn sample_result(lang: &str, runtime: &str, rps: f64, cold: bool) -> BenchmarkResult {
        BenchmarkResult {
            language: lang.into(),
            runtime: runtime.into(),
            concurrency: 10,
            repeat_index: 0,
            phase: if cold { "cold".into() } else { "warm".into() },
            network: NetworkMetrics {
                rps,
                latency_mean_ms: 1.0 / rps * 1000.0,
                latency_p50_ms: 0.8 / rps * 1000.0,
                latency_p99_ms: 3.0 / rps * 1000.0,
                latency_p999_ms: 5.0 / rps * 1000.0,
                latency_max_ms: 10.0 / rps * 1000.0,
                bytes_transferred: 1_000_000,
                error_count: 0,
                total_requests: if cold { 50 } else { 10_000 },
            },
            resources: ResourceMetrics {
                peak_rss_bytes: 50_000_000,
                avg_cpu_fraction: 0.45,
                peak_cpu_fraction: 0.92,
                peak_open_fds: 128,
            },
            startup: StartupMetrics {
                time_to_first_response_ms: if cold { 120.0 } else { 2.0 },
                time_to_ready_ms: if cold { 200.0 } else { 5.0 },
            },
            binary: BinaryMetrics {
                size_bytes: 8_000_000,
                compressed_size_bytes: 3_000_000,
                docker_image_bytes: None,
            },
            fairness: Default::default(),
            result_validity: crate::types::ResultValidity {
                state: crate::types::ValidityState::Valid,
                is_valid: true,
                warnings: vec![],
            },
            diagnostics: crate::types::BenchmarkDiagnostics::default(),
        }
    }

    fn sample_run() -> BenchmarkRun {
        let mut run = BenchmarkRun {
            id: Uuid::new_v4(),
            started_at: Utc::now() - chrono::Duration::minutes(30),
            finished_at: Some(Utc::now()),
            config_path: "benchmarks/config.json".into(),
            run_purpose: crate::types::RunPurpose::Benchmark,
            benchmark_family: crate::types::BenchmarkFamily::RuntimeEfficiency,
            benchmark_intent: crate::types::BenchmarkIntent::RuntimeEfficiencyUnderLoad,
            scenario: crate::types::ScenarioState {
                connection_state: crate::types::ConnectionState::Pooled,
                browser_view: crate::types::BrowserView::NotApplicable,
                load_state: crate::types::LoadState::Loaded,
                load_model: crate::types::LoadModel::ClosedLoop,
                topology_class: crate::types::TopologyClass::WanLowRtt,
                protocol_expected: Some("http2".into()),
                protocol_negotiated: Some("http2".into()),
            },
            workload_profile: crate::types::WorkloadProfile {
                request_size_bytes: "small_json".into(),
                response_size_bytes: "small_json".into(),
                concurrency_limit: Some(100),
                connection_reuse: "pooled".into(),
                arrival_pattern: "completion_driven".into(),
                burstiness: "none".into(),
                think_time_ms: 0,
                object_count: None,
            },
            workload_template: Some(crate::types::WorkloadTemplateRef {
                name: "api_small_payload".into(),
                version: "v1".into(),
            }),
            offered_load: None,
            validity_manifest: crate::types::ValidityManifest {
                environment_stability: crate::types::StabilityGrade::Medium,
                protocol_verified: false,
                client_saturation_risk: crate::types::StabilityGrade::Medium,
                topology_control: crate::types::TopologyControl::SemiControlled,
                sample_size: 3,
                warm_state_certainty: crate::types::StabilityGrade::Medium,
                measurement_method: "closed_loop_basic".into(),
            },
            reproducibility: crate::types::ReproducibilityMetadata::default(),
            results: vec![
                sample_result("rust", "axum", 85000.0, true),
                sample_result("rust", "axum", 120000.0, false),
                sample_result("go", "gin", 60000.0, true),
                sample_result("go", "gin", 95000.0, false),
                sample_result("python", "fastapi", 8000.0, true),
                sample_result("python", "fastapi", 12000.0, false),
                sample_result("node", "express", 25000.0, true),
                sample_result("node", "express", 45000.0, false),
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
        assert!(content.contains("Leaderboard"));
        assert!(content.contains("req/s (warm)"));

        // Charts (SVG)
        assert!(content.contains("Cold vs Warm Throughput"));
        assert!(content.contains("Latency Distribution"));
        assert!(content.contains("Resource Usage"));
        assert!(content.contains("<svg"));

        // Methodology
        assert!(content.contains("Methodology"));
        assert!(content.contains("config.json"));

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
            run_purpose: crate::types::RunPurpose::Benchmark,
            benchmark_family: crate::types::BenchmarkFamily::RuntimeEfficiency,
            benchmark_intent: crate::types::BenchmarkIntent::RuntimeEfficiencyUnderLoad,
            scenario: crate::types::ScenarioState {
                connection_state: crate::types::ConnectionState::Pooled,
                browser_view: crate::types::BrowserView::NotApplicable,
                load_state: crate::types::LoadState::Loaded,
                load_model: crate::types::LoadModel::ClosedLoop,
                topology_class: crate::types::TopologyClass::WanLowRtt,
                protocol_expected: Some("http2".into()),
                protocol_negotiated: None,
            },
            workload_profile: crate::types::WorkloadProfile {
                request_size_bytes: "small_json".into(),
                response_size_bytes: "small_json".into(),
                concurrency_limit: None,
                connection_reuse: "pooled".into(),
                arrival_pattern: "completion_driven".into(),
                burstiness: "none".into(),
                think_time_ms: 0,
                object_count: None,
            },
            workload_template: None,
            offered_load: None,
            validity_manifest: crate::types::ValidityManifest {
                environment_stability: crate::types::StabilityGrade::Medium,
                protocol_verified: false,
                client_saturation_risk: crate::types::StabilityGrade::Medium,
                topology_control: crate::types::TopologyControl::SemiControlled,
                sample_size: 1,
                warm_state_certainty: crate::types::StabilityGrade::Medium,
                measurement_method: "closed_loop_basic".into(),
            },
            reproducibility: crate::types::ReproducibilityMetadata::default(),
            results: vec![],
        };
        let dir = std::env::temp_dir();
        let path = dir.join("alethabench-test-empty.html");
        generate_html(&run, &path).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("AletheBench Report"));
        assert!(content.contains("0 languages"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_generate_json_roundtrip() {
        let run = sample_run();
        let dir = std::env::temp_dir();
        let path = dir.join("alethabench-test-json.json");
        generate_json(&run, &path).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let loaded: BenchmarkRun = serde_json::from_str(&content).unwrap();
        assert_eq!(loaded.id, run.id);
        assert_eq!(loaded.results.len(), run.results.len());
        std::fs::remove_file(&path).ok();
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
    fn test_summarise_by_language_ordering() {
        let run = sample_run();
        let summaries = summarise_by_language(&run);
        assert_eq!(summaries.len(), 4);
        // Should be sorted by warm RPS descending
        assert_eq!(summaries[0].language, "rust");
        assert_eq!(summaries[1].language, "go");
        assert_eq!(summaries[2].language, "node");
        assert_eq!(summaries[3].language, "python");
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
