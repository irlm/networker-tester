//! Chart rendering: the multi-target comparison charts plus the
//! self-contained SVG primitives (boxplot, CDF, horizontal bar).

use super::*;

/// Per-target load time distribution charts + observations in a 2-column grid.
/// Only rendered when there are multiple targets and at least one has pageload/browser data.
pub(super) fn write_multi_target_charts(
    runs: &[TestRun],
    short_names: &[String],
    out: &mut String,
) {
    struct TargetChartData {
        idx: usize,
        url: String,
        server_info: String,
        network_type: NetworkType,
        rtt_avg_ms: f64,
        boxplot_groups: Vec<(String, Vec<f64>, &'static str)>,
        observations: Vec<String>,
        best_avg_ms: f64, // best protocol avg for cross-group comparison
    }

    let proto_colors: &[(Protocol, &str)] = &[
        (Protocol::Browser1, "#e07b39"),
        (Protocol::Browser2, "#4e79a7"),
        (Protocol::Browser3, "#59a14f"),
        (Protocol::Browser, "#8c6bb1"),
        (Protocol::PageLoad, "#e07b39"),
        (Protocol::PageLoad2, "#4e79a7"),
        (Protocol::PageLoad3, "#59a14f"),
    ];

    let mut targets: Vec<TargetChartData> = Vec::new();

    for (i, run) in runs.iter().enumerate() {
        let mut groups: Vec<(String, Vec<f64>, &'static str)> = Vec::new();
        let mut observations: Vec<String> = Vec::new();

        for (proto, color) in proto_colors {
            let data: Vec<f64> = run
                .attempts
                .iter()
                .filter(|a| &a.protocol == proto)
                .filter_map(|a| {
                    if matches!(
                        proto,
                        Protocol::Browser
                            | Protocol::Browser1
                            | Protocol::Browser2
                            | Protocol::Browser3
                    ) {
                        a.browser.as_ref().map(|b| b.load_ms)
                    } else {
                        a.page_load.as_ref().map(|p| p.total_ms)
                    }
                })
                .collect();
            if data.len() >= 2 {
                groups.push((proto.to_string(), data, color));
            }
        }

        if groups.is_empty() {
            continue;
        }

        // Best avg across all protocols for this target
        let mut best_avg = f64::MAX;
        let mut fastest: Option<(&str, f64)> = None;
        for (label, data, _) in &groups {
            let avg = data.iter().sum::<f64>() / data.len() as f64;
            if avg < best_avg {
                best_avg = avg;
                fastest = Some((label.as_str(), avg));
            }
        }
        if let Some((proto, ms)) = fastest {
            observations.push(format!("Fastest: <strong>{proto}</strong> ({ms:.1}ms avg)"));
        }

        let h2_avg = groups
            .iter()
            .find(|(l, _, _)| l == "browser2" || l == "pageload2")
            .map(|(_, d, _)| d.iter().sum::<f64>() / d.len() as f64);
        let h3_avg = groups
            .iter()
            .find(|(l, _, _)| l == "browser3" || l == "pageload3")
            .map(|(_, d, _)| d.iter().sum::<f64>() / d.len() as f64);
        if let (Some(h2), Some(h3)) = (h2_avg, h3_avg) {
            let diff = h2 - h3;
            let pct = diff / h2 * 100.0;
            if diff > 0.0 {
                observations.push(format!("H3 is {:.1}ms ({:.1}%) faster than H2", diff, pct));
            } else {
                observations.push(format!(
                    "H2 is {:.1}ms ({:.1}%) faster than H3",
                    -diff, -pct
                ));
            }
        }

        let mut spreads: Vec<(&str, f64)> = Vec::new();
        for (label, data, _) in &groups {
            if data.len() >= 4 {
                let mut sorted = data.clone();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let n = sorted.len();
                let p50 = sorted[((n as f64 * 0.50).round() as usize).min(n - 1)];
                let p95 = sorted[((n as f64 * 0.95).round() as usize).min(n - 1)];
                spreads.push((label.as_str(), p95 - p50));
            }
        }
        if spreads.len() >= 2 {
            spreads.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            let (stable, sp) = spreads[0];
            observations.push(format!(
                "Most consistent: <strong>{stable}</strong> (p95\u{2212}p50 = {sp:.1}ms)"
            ));
        }

        // Server info summary
        let server_info = run
            .server_info
            .as_ref()
            .map(|s| {
                let os = s.os_version.as_deref().unwrap_or(&s.os);
                let mem = s
                    .total_memory_mb
                    .map(|mb| {
                        if mb >= 1024 {
                            format!("{:.0} GB", mb as f64 / 1024.0)
                        } else {
                            format!("{mb} MB")
                        }
                    })
                    .unwrap_or_default();
                let ver = s
                    .server_version
                    .as_ref()
                    .map(|v| format!(" (v{v})"))
                    .unwrap_or_default();
                let region = s
                    .region
                    .as_ref()
                    .map(|r| format!(" | {r}"))
                    .unwrap_or_default();
                let display_name = derive_display_name(Some(s), "");
                format!(
                    "{display_name}{ver} | {os} | {} cores | {mem}{region}",
                    s.cpu_cores
                )
            })
            .unwrap_or_default();

        let net_type = run
            .baseline
            .as_ref()
            .map(|b| b.network_type)
            .unwrap_or(NetworkType::Internet);
        let rtt = run
            .baseline
            .as_ref()
            .filter(|b| b.samples > 0)
            .map(|b| b.rtt_avg_ms)
            .unwrap_or(0.0);

        targets.push(TargetChartData {
            idx: i,
            url: run.target_url.clone(),
            server_info,
            network_type: net_type,
            rtt_avg_ms: rtt,
            boxplot_groups: groups,
            observations,
            best_avg_ms: best_avg,
        });
    }

    if targets.is_empty() {
        return;
    }

    // Split targets by network type
    let internet_targets: Vec<&TargetChartData> = targets
        .iter()
        .filter(|t| t.network_type == NetworkType::Internet)
        .collect();
    let lan_targets: Vec<&TargetChartData> = targets
        .iter()
        .filter(|t| t.network_type == NetworkType::LAN || t.network_type == NetworkType::Loopback)
        .collect();

    // Helper closure to write a group of target cells
    let write_target_group = |group: &[&TargetChartData], group_label: &str, out: &mut String| {
        if group.is_empty() {
            return;
        }
        let _ = writeln!(
            out,
            "    <div class=\"target-group-header\">{group_label}</div>"
        );
        for t in group {
            let group_refs: Vec<(&str, &[f64], &str)> = t
                .boxplot_groups
                .iter()
                .map(|(l, v, c)| (l.as_str(), v.as_slice(), *c))
                .collect();
            let title = format!("{} \u{2014} {}", short_names[t.idx], t.url);
            let svg = svg_boxplot(&title, &group_refs, "ms");

            let _ = writeln!(out, "    <div class=\"target-chart-cell\">");
            // Server info bar
            let net_badge = match t.network_type {
                NetworkType::Loopback => r#"<span class="ok">Loopback</span>"#,
                NetworkType::LAN => r#"<span class="warn">LAN</span>"#,
                NetworkType::Internet => r#"<span class="err">Internet</span>"#,
            };
            let _ = writeln!(
                    out,
                    "      <div class=\"target-server-info\">{net_badge} RTT {:.2}ms &bull; <small>{}</small></div>",
                    t.rtt_avg_ms,
                    escape_html(&t.server_info)
                );
            let _ = writeln!(out, "      {svg}");

            if !t.observations.is_empty() {
                let _ = writeln!(
                    out,
                    "      <div class=\"analysis\"><h3>Observations</h3><ul>"
                );
                for obs in &t.observations {
                    let _ = writeln!(out, "        <li>{obs}</li>");
                }
                let _ = writeln!(out, "      </ul></div>");
            }

            let _ = writeln!(out, "    </div>");
        }

        // Intra-group comparison (when 2+ targets in same network type)
        if group.len() >= 2 {
            let best = group
                .iter()
                .min_by(|a, b| {
                    a.best_avg_ms
                        .partial_cmp(&b.best_avg_ms)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap();
            let worst = group
                .iter()
                .max_by(|a, b| {
                    a.best_avg_ms
                        .partial_cmp(&b.best_avg_ms)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap();
            if (worst.best_avg_ms - best.best_avg_ms).abs() > 0.01 {
                let pct = (worst.best_avg_ms - best.best_avg_ms) / worst.best_avg_ms * 100.0;
                let _ = writeln!(
                    out,
                    "    <div class=\"target-group-comparison analysis\">\
                         <strong>{} is {:.1}% faster</strong> than {} \
                         ({:.1}ms vs {:.1}ms best protocol avg) \u{2014} \
                         RTT difference: {:.1}ms</div>",
                    short_names[best.idx],
                    pct,
                    short_names[worst.idx],
                    best.best_avg_ms,
                    worst.best_avg_ms,
                    (worst.rtt_avg_ms - best.rtt_avg_ms).abs()
                );
            }
        }
    };

    let _ = writeln!(
        out,
        r##"
<section class="card">
  <h2>Per-Target Load Time Distribution</h2>
  <div class="target-charts-grid">"##
    );

    // Render Internet targets first, then LAN targets
    if !internet_targets.is_empty() && !lan_targets.is_empty() {
        write_target_group(&internet_targets, "\u{1f310} Internet Targets", out);
        write_target_group(&lan_targets, "\u{1f3e0} LAN / Local Targets", out);

        // Cross-group comparison: best Internet vs best LAN
        let best_internet = internet_targets
            .iter()
            .min_by(|a, b| {
                a.best_avg_ms
                    .partial_cmp(&b.best_avg_ms)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap();
        let best_lan = lan_targets
            .iter()
            .min_by(|a, b| {
                a.best_avg_ms
                    .partial_cmp(&b.best_avg_ms)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap();

        let overhead = best_internet.best_avg_ms - best_lan.best_avg_ms;
        let factor = if best_lan.best_avg_ms > 0.01 {
            best_internet.best_avg_ms / best_lan.best_avg_ms
        } else {
            0.0
        };
        let _ = writeln!(
            out,
            r##"    <div class="target-group-header">LAN vs Internet &mdash; Best of Each</div>
    <div class="target-group-comparison analysis" style="grid-column:1/-1">
      <h3>LAN Baseline vs Internet Reality</h3>
      <ul>
        <li>Best LAN: <strong>{lan_name}</strong> &mdash; {lan_ms:.1}ms (RTT {lan_rtt:.2}ms)</li>
        <li>Best Internet: <strong>{inet_name}</strong> &mdash; {inet_ms:.1}ms (RTT {inet_rtt:.2}ms)</li>
        <li>Internet overhead: <strong>+{overhead:.1}ms</strong> ({factor:.1}x slower than LAN baseline)</li>
        <li>The LAN result represents the achievable performance without network latency &mdash;
           the Internet result shows the real-world impact of {rtt_diff:.0}ms RTT + jitter + congestion</li>
      </ul>
    </div>"##,
            lan_name = short_names[best_lan.idx],
            lan_ms = best_lan.best_avg_ms,
            lan_rtt = best_lan.rtt_avg_ms,
            inet_name = short_names[best_internet.idx],
            inet_ms = best_internet.best_avg_ms,
            inet_rtt = best_internet.rtt_avg_ms,
            rtt_diff = best_internet.rtt_avg_ms - best_lan.rtt_avg_ms,
        );
    } else {
        // All same network type — just render them all
        let label = if !internet_targets.is_empty() {
            "\u{1f310} Internet Targets"
        } else {
            "\u{1f3e0} LAN / Local Targets"
        };
        write_target_group(&targets.iter().collect::<Vec<_>>(), label, out);
    }

    let _ = writeln!(out, "  </div>\n</section>");
}

/// Render a self-contained box-and-whisker SVG chart (horizontal layout).
///
/// `groups`: `(label, values, fill_color)` — values need at least 4 points.
/// Draws: p5 whisker ← Q1 box → median line → Q3 box → p95 whisker.
/// `unit`: appended to the per-row annotation label.
pub(super) fn svg_boxplot(title: &str, groups: &[(&str, &[f64], &str)], unit: &str) -> String {
    const LBL_W: usize = 130;
    const BOX_AREA: usize = 280;
    const VAL_W: usize = 130;
    const BOX_H: usize = 18;
    const ROW_H: usize = 32;
    const PAD_TOP: usize = 30;
    const PAD_BOT: usize = 12;

    struct BoxRow {
        label: String,
        color: String,
        p5: f64,
        q1: f64,
        median: f64,
        q3: f64,
        p95: f64,
    }

    let rows: Vec<BoxRow> = groups
        .iter()
        .filter(|(_, v, _)| v.len() >= 2)
        .map(|(label, vals, color)| {
            let mut sorted = vals.to_vec();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let n = sorted.len();
            let pct = |p: f64| -> f64 {
                let idx = (p * (n - 1) as f64).round() as usize;
                sorted[idx.min(n - 1)]
            };
            BoxRow {
                label: (*label).to_string(),
                color: (*color).to_string(),
                p5: pct(0.05),
                q1: pct(0.25),
                median: pct(0.50),
                q3: pct(0.75),
                p95: pct(0.95),
            }
        })
        .collect();

    if rows.is_empty() {
        return String::new();
    }

    // Scale based on p5–p95 range across all rows, with 5% padding.
    let global_min = rows.iter().map(|r| r.p5).fold(f64::MAX, f64::min);
    let global_max = rows.iter().map(|r| r.p95).fold(0.0_f64, f64::max);
    let range = (global_max - global_min).max(1.0);
    let pad = range * 0.05;
    let x_lo = (global_min - pad).max(0.0);
    let x_hi = global_max + pad;
    let span = x_hi - x_lo;
    let scale = |v: f64| -> usize { ((v - x_lo) / span * BOX_AREA as f64).round() as usize };

    let total_h = PAD_TOP + rows.len() * ROW_H + PAD_BOT;
    let total_w = LBL_W + BOX_AREA + VAL_W;

    let mut s = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{total_w}" height="{total_h}" style="font-family:system-ui,sans-serif;font-size:12px">"#
    );
    s.push_str(&format!(
        r##"<text x="{}" y="18" font-weight="bold" font-size="13" fill="#1a1a2e">{}</text>"##,
        LBL_W + 5,
        escape_html(title)
    ));
    for (i, row) in rows.iter().enumerate() {
        let y0 = PAD_TOP + i * ROW_H;
        let cy = y0 + ROW_H / 2;
        let bx = LBL_W;
        s.push_str(&format!(
            r##"<text x="{}" y="{cy}" text-anchor="end" dominant-baseline="middle" fill="#555">{}</text>"##,
            bx - 5,
            escape_html(&row.label)
        ));
        let p5x = bx + scale(row.p5);
        let q1x = bx + scale(row.q1);
        let medx = bx + scale(row.median);
        let q3x = bx + scale(row.q3);
        let p95x = bx + scale(row.p95);
        let box_top = cy - BOX_H / 2;
        // Dashed whisker p5–p95
        s.push_str(&format!(
            r#"<line x1="{p5x}" y1="{cy}" x2="{p95x}" y2="{cy}" stroke="{}" stroke-width="1.5" stroke-dasharray="3,2" opacity="0.5"/>"#,
            row.color
        ));
        // p5 tick
        s.push_str(&format!(
            r#"<line x1="{p5x}" y1="{}" x2="{p5x}" y2="{}" stroke="{}" stroke-width="1.5" opacity="0.6"/>"#,
            cy - 6, cy + 6, row.color
        ));
        // p95 tick
        s.push_str(&format!(
            r#"<line x1="{p95x}" y1="{}" x2="{p95x}" y2="{}" stroke="{}" stroke-width="1.5" opacity="0.6"/>"#,
            cy - 6, cy + 6, row.color
        ));
        // IQR box Q1–Q3
        let box_w = q3x.saturating_sub(q1x).max(2);
        s.push_str(&format!(
            r#"<rect x="{q1x}" y="{box_top}" width="{box_w}" height="{BOX_H}" rx="2" fill="{}" opacity="0.75"/>"#,
            row.color
        ));
        // Median line
        s.push_str(&format!(
            r#"<line x1="{medx}" y1="{box_top}" x2="{medx}" y2="{}" stroke="white" stroke-width="2.5"/>"#,
            box_top + BOX_H
        ));
        // Annotation
        s.push_str(&format!(
            r##"<text x="{}" y="{cy}" dominant-baseline="middle" fill="#555" font-size="11">p50={:.1}  p95={:.1} {unit}</text>"##,
            bx + BOX_AREA + 6,
            row.median,
            row.p95
        ));
    }
    s.push_str("</svg>");
    s
}

/// Render a self-contained empirical CDF (step-function) SVG chart.
///
/// `series`: `(label, values, stroke_color)` — values need not be pre-sorted.
/// Series with fewer than 2 values are skipped.
/// `unit`: appended on x-axis tick labels.
pub(super) fn svg_cdf(title: &str, series: &[(&str, &[f64], &str)], unit: &str) -> String {
    const W: usize = 490;
    const PAD_L: usize = 46;
    const PAD_R: usize = 10;
    const PAD_T: usize = 30;
    const PAD_B: usize = 36;
    const PLOT_H: usize = 150;
    const LEG_ROW: usize = 18;

    let sorted_series: Vec<(&str, Vec<f64>, &str)> = series
        .iter()
        .filter(|(_, v, _)| v.len() >= 2)
        .map(|(lbl, vals, col)| {
            let mut sv = vals.to_vec();
            sv.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            (*lbl, sv, *col)
        })
        .collect();

    if sorted_series.is_empty() {
        return String::new();
    }

    // x range: 0 to 99th-percentile max (prevents outlier stretch).
    let p99_val = |v: &[f64]| {
        let i = ((v.len() - 1) as f64 * 0.99).round() as usize;
        v[i.min(v.len() - 1)]
    };
    let x_max = sorted_series
        .iter()
        .map(|(_, v, _)| p99_val(v))
        .fold(0.0_f64, f64::max)
        * 1.05;

    let plot_w = W - PAD_L - PAD_R;
    let leg_rows = sorted_series.len().div_ceil(3);
    let total_h = PAD_T + PLOT_H + PAD_B + leg_rows * LEG_ROW;

    let sx = |v: f64| PAD_L + (v.min(x_max) / x_max * plot_w as f64).round() as usize;
    let sy = |p: f64| PAD_T + ((1.0 - p) * PLOT_H as f64).round() as usize;

    let mut s = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{total_h}" style="font-family:system-ui,sans-serif;font-size:11px">"#
    );
    s.push_str(&format!(
        r##"<text x="{}" y="18" text-anchor="middle" font-weight="bold" font-size="13" fill="#1a1a2e">{}</text>"##,
        W / 2,
        escape_html(title)
    ));
    // Plot background
    s.push_str(&format!(
        r##"<rect x="{PAD_L}" y="{PAD_T}" width="{plot_w}" height="{PLOT_H}" fill="#f8f9fa" stroke="#ddd" stroke-width="1"/>"##
    ));
    // Y-axis grid + labels
    for &pct in &[0u32, 25, 50, 75, 100] {
        let p = pct as f64 / 100.0;
        let y = sy(p);
        let stroke = if pct == 0 || pct == 100 {
            "#ccc"
        } else {
            "#e8e8e8"
        };
        s.push_str(&format!(
            r#"<line x1="{PAD_L}" y1="{y}" x2="{}" y2="{y}" stroke="{stroke}" stroke-width="1"/>"#,
            PAD_L + plot_w
        ));
        s.push_str(&format!(
            r##"<text x="{}" y="{y}" text-anchor="end" dominant-baseline="middle" fill="#999">{pct}%</text>"##,
            PAD_L - 4
        ));
    }
    // X-axis ticks (5 evenly spaced)
    let y_bot = PAD_T + PLOT_H;
    for step in 0..=4usize {
        let v = x_max * step as f64 / 4.0;
        let x = sx(v);
        if step > 0 {
            s.push_str(&format!(
                r##"<line x1="{x}" y1="{PAD_T}" x2="{x}" y2="{y_bot}" stroke="#e8e8e8" stroke-width="1"/>"##
            ));
        }
        s.push_str(&format!(
            r##"<line x1="{x}" y1="{y_bot}" x2="{x}" y2="{}" stroke="#aaa" stroke-width="1"/>"##,
            y_bot + 4
        ));
        s.push_str(&format!(
            r##"<text x="{x}" y="{}" text-anchor="middle" fill="#999">{:.0} {unit}</text>"##,
            y_bot + 16,
            v
        ));
    }
    // CDF step-function polylines
    for (_, vals, color) in &sorted_series {
        let n = vals.len();
        let mut pts = format!("{},{}", sx(0.0), sy(0.0));
        for (i, &v) in vals.iter().enumerate() {
            let x = sx(v);
            pts.push_str(&format!(
                " {},{} {},{}",
                x,
                sy(i as f64 / n as f64),
                x,
                sy((i + 1) as f64 / n as f64)
            ));
        }
        s.push_str(&format!(
            r#"<polyline points="{pts}" fill="none" stroke="{color}" stroke-width="2" stroke-linejoin="miter"/>"#
        ));
    }
    // Legend
    let leg_y0 = y_bot + PAD_B - 6;
    for (i, (lbl, _, color)) in sorted_series.iter().enumerate() {
        let col = i % 3;
        let row = i / 3;
        let lx = PAD_L + col * (plot_w / 3);
        let ly = leg_y0 + row * LEG_ROW;
        s.push_str(&format!(
            r#"<rect x="{lx}" y="{}" width="12" height="10" fill="{color}" rx="2"/>"#,
            ly - 5
        ));
        s.push_str(&format!(
            r##"<text x="{}" y="{ly}" dominant-baseline="middle" fill="#555">{}</text>"##,
            lx + 16,
            escape_html(lbl)
        ));
    }
    s.push_str("</svg>");
    s
}

/// Render a self-contained horizontal SVG bar chart.
/// `bars`: (label, value) pairs sorted as desired.
/// `unit`: appended after the value in the value label (e.g. "ms", "MB/s").
/// `colors`: per-bar fill color hex strings; cycles if shorter than `bars`.
pub(super) fn svg_hbar(title: &str, bars: &[(&str, f64)], unit: &str, colors: &[&str]) -> String {
    const LBL_W: usize = 130;
    const BAR_AREA: usize = 280;
    const VAL_W: usize = 80;
    const BAR_H: usize = 26;
    const GAP: usize = 6;
    const PAD_TOP: usize = 30;
    const PAD_BOT: usize = 12;

    if bars.is_empty() {
        return String::new();
    }
    let max_val = bars.iter().map(|(_, v)| *v).fold(0.0_f64, f64::max);
    let total_h = PAD_TOP + bars.len() * (BAR_H + GAP) + PAD_BOT;
    let total_w = LBL_W + BAR_AREA + VAL_W;
    let def_color = "#4e79a7";

    let mut s = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{total_w}" height="{total_h}" style="font-family:system-ui,sans-serif;font-size:12px">"#
    );
    s.push_str(&format!(
        r##"<text x="{}" y="18" font-weight="bold" font-size="13" fill="#1a1a2e">{}</text>"##,
        LBL_W + 5,
        escape_html(title)
    ));
    for (i, (label, value)) in bars.iter().enumerate() {
        let y = PAD_TOP + i * (BAR_H + GAP);
        let bx = LBL_W;
        let bar_w = if max_val > 0.0 {
            (*value / max_val * BAR_AREA as f64) as usize
        } else {
            0
        };
        let color = colors
            .get(i % colors.len().max(1))
            .copied()
            .unwrap_or(def_color);
        s.push_str(&format!(
            r##"<text x="{}" y="{}" text-anchor="end" dominant-baseline="middle" fill="#555">{}</text>"##,
            bx - 5,
            y + BAR_H / 2,
            escape_html(label)
        ));
        if bar_w > 0 {
            s.push_str(&format!(
                r#"<rect x="{bx}" y="{y}" width="{bar_w}" height="{BAR_H}" rx="3" fill="{color}"/>"#
            ));
        }
        s.push_str(&format!(
            r##"<text x="{}" y="{}" dominant-baseline="middle" fill="#333">{:.1} {}</text>"##,
            bx + bar_w + 5,
            y + BAR_H / 2,
            value,
            unit
        ));
    }
    s.push_str("</svg>");
    s
}
