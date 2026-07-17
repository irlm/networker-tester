//! SVG chart builders for the HTML report.

use super::html::{escape_html, format_bytes};
use crate::types::{BenchmarkCaseSummary, ScenarioSummary};
use std::fmt::Write as FmtWrite;

// ---------------------------------------------------------------------------
// SVG chart builders
// ---------------------------------------------------------------------------

/// Color palette for languages (cycles if more than available).
const LANG_COLORS: &[&str] = &[
    "#47bfff", "#ff6b6b", "#51cf66", "#fcc419", "#cc5de8", "#ff922b", "#20c997", "#748ffc",
    "#f06595", "#ced4da",
];

pub(super) fn lang_color(index: usize) -> &'static str {
    LANG_COLORS[index % LANG_COLORS.len()]
}

pub(super) fn warm_summary(summary: &BenchmarkCaseSummary) -> Option<&ScenarioSummary> {
    summary.warm.as_ref()
}

pub(super) fn cold_summary(summary: &BenchmarkCaseSummary) -> Option<&ScenarioSummary> {
    summary.cold.as_ref()
}

/// Build a grouped bar chart comparing cold vs warm median RPS per case.
pub(super) fn svg_cold_warm_chart(summaries: &[BenchmarkCaseSummary]) -> String {
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

// Parameter count reflects the benchmark cycle's real coordination surface;
// bundling into a struct adds indirection without clarity (measurement-path code).
#[allow(clippy::too_many_arguments)]
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
pub(super) fn svg_latency_chart(summaries: &[BenchmarkCaseSummary]) -> String {
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
pub(super) fn svg_resource_chart(summaries: &[BenchmarkCaseSummary]) -> String {
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
