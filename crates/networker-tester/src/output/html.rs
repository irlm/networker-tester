/// Generate a self-contained HTML diagnostic report from a `TestRun`.
///
/// The report embeds a minimal inline CSS for offline viewing and optionally
/// adds a `<link rel="stylesheet">` for the external `report.css` file so
/// operators can customize the look without editing generated HTML.
use crate::metrics::{
    attempt_payload_bytes, compute_stats, primary_metric_label, primary_metric_value, HostInfo,
    NetworkType, Protocol, RequestAttempt, TestRun,
};
use chrono::DateTime;
use std::fmt::Write as FmtWrite;
use std::path::Path;

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

pub fn save(run: &TestRun, path: &Path, css_href: Option<&str>) -> anyhow::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let html = render(run, css_href);
    std::fs::write(path, html)?;
    Ok(())
}

/// Save a combined HTML report for one or more `TestRun`s.
///
/// When `runs.len() == 1` the output is identical to `save()`.
/// When `runs.len() > 1` the report includes a cross-target summary and
/// protocol-comparison table followed by per-target collapsible sections.
pub fn save_multi(runs: &[TestRun], path: &Path, css_href: Option<&str>) -> anyhow::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    std::fs::write(path, render_multi(runs, css_href))?;
    Ok(())
}

pub fn render(run: &TestRun, css_href: Option<&str>) -> String {
    let mut out = String::with_capacity(64 * 1024);
    write_html_head(&run.target_url, css_href, &mut out);
    write_run_sections(run, &mut out);
    write_html_footer(run.finished_at.unwrap_or(run.started_at), &mut out);
    out
}

/// Render a combined report for multiple targets.
///
/// Single-target runs delegate to `render()` for identical output.
pub fn render_multi(runs: &[TestRun], css_href: Option<&str>) -> String {
    if runs.len() == 1 {
        return render(&runs[0], css_href);
    }

    let mut out = String::with_capacity(128 * 1024);
    let title = format!("{} targets compared", runs.len());
    write_html_head(&title, css_href, &mut out);

    // ── Multi-target page header ──────────────────────────────────────────────
    let started = runs[0].started_at.format("%Y-%m-%d %H:%M:%S UTC");
    let _ = write!(
        out,
        r#"
<header class="page-header">
  <h1>Networker Tester</h1>
  <p class="subtitle">{n} targets compared &bull; {started}</p>
</header>
"#,
        n = runs.len(),
    );

    // ── Multi-Target Summary table ────────────────────────────────────────────
    let _ = write!(
        out,
        r#"
<section class="card">
  <h2>Multi-Target Summary</h2>
  <table>
    <thead>
      <tr>
        <th>#</th><th>Target</th><th>Server</th><th>Network</th><th>RTT (avg)</th>
        <th>Attempts</th><th>Succeeded</th><th>Failed</th><th>Duration</th>
      </tr>
    </thead>
    <tbody>
"#
    );
    for (i, run) in runs.iter().enumerate() {
        let dur = run
            .finished_at
            .map(|f| {
                format!(
                    "{:.2}s",
                    (f - run.started_at).num_milliseconds() as f64 / 1000.0
                )
            })
            .unwrap_or_else(|| "—".into());
        let fail = run.failure_count();
        let server_summary = run
            .server_info
            .as_ref()
            .map(|s| {
                let hostname = s.hostname.as_deref().unwrap_or("");
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
                let region = s
                    .region
                    .as_ref()
                    .map(|r| format!("<br><small>Region: {r}</small>"))
                    .unwrap_or_default();
                if hostname.is_empty() {
                    format!("{os} | {} cores | {mem}{region}", s.cpu_cores)
                } else {
                    format!(
                        "{hostname}<br><small>{os} | {} cores | {mem}</small>{region}",
                        s.cpu_cores
                    )
                }
            })
            .unwrap_or_else(|| "—".into());
        let net_type = run
            .baseline
            .as_ref()
            .map(|b| {
                let badge_cls = match b.network_type {
                    NetworkType::Loopback => "ok",
                    NetworkType::LAN => "warn",
                    NetworkType::Internet => "err",
                };
                format!(r#"<span class="{badge_cls}">{}</span>"#, b.network_type)
            })
            .unwrap_or_else(|| "—".into());
        let rtt_avg = run
            .baseline
            .as_ref()
            .map(|b| format!("{:.2} ms", b.rtt_avg_ms))
            .unwrap_or_else(|| "—".into());
        let _ = write!(
            out,
            r#"      <tr>
        <td>{idx}</td>
        <td><a href="{url}">{url}</a></td>
        <td>{server}</td>
        <td>{net_type}</td>
        <td>{rtt_avg}</td>
        <td>{attempts}</td>
        <td class="ok">{ok}</td>
        <td class="{fail_cls}">{fail}</td>
        <td>{dur}</td>
      </tr>
"#,
            idx = i + 1,
            url = escape_html(&run.target_url),
            server = server_summary,
            attempts = run.attempts.len(),
            ok = run.success_count(),
            fail_cls = if fail > 0 { "err" } else { "ok" },
        );
    }
    let _ = writeln!(out, "    </tbody>\n  </table>\n</section>");

    // ── Cross-Target Protocol Comparison table ────────────────────────────────
    // Collect all protocols present in any run (canonical order).
    let canonical_protos = [
        Protocol::Http1,
        Protocol::Http2,
        Protocol::Http3,
        Protocol::Native,
        Protocol::Curl,
        Protocol::Tcp,
        Protocol::Udp,
        Protocol::Dns,
        Protocol::Tls,
        Protocol::Download,
        Protocol::Upload,
        Protocol::WebDownload,
        Protocol::WebUpload,
        Protocol::UdpDownload,
        Protocol::UdpUpload,
        Protocol::PageLoad,
        Protocol::PageLoad2,
        Protocol::PageLoad3,
        Protocol::Browser,
        Protocol::Browser1,
        Protocol::Browser2,
        Protocol::Browser3,
    ];

    // avg_primary(run, proto) → avg of primary_metric_value across attempts for that proto.
    let avg_primary = |run: &TestRun, proto: &Protocol| -> Option<f64> {
        let vals: Vec<f64> = run
            .attempts
            .iter()
            .filter(|a| &a.protocol == proto)
            .filter_map(primary_metric_value)
            .collect();
        if vals.is_empty() {
            None
        } else {
            Some(vals.iter().sum::<f64>() / vals.len() as f64)
        }
    };

    // Only include protocols where at least one run has data.
    let active_protos: Vec<&Protocol> = canonical_protos
        .iter()
        .filter(|proto| runs.iter().any(|r| avg_primary(r, proto).is_some()))
        .collect();

    if !active_protos.is_empty() {
        // Build column headers: Protocol | Unit | Target 1 | Target 2 | …
        let _ = write!(
            out,
            r#"
<section class="card">
  <h2>Cross-Target Protocol Comparison</h2>
  <table>
    <thead>
      <tr>
        <th>Protocol</th>
        <th>Metric</th>
"#
        );
        for (i, run) in runs.iter().enumerate() {
            let _ = writeln!(
                out,
                "        <th>Target {} <small>{}</small></th>",
                i + 1,
                escape_html(&run.target_url)
            );
        }
        let _ = writeln!(out, "      </tr>\n    </thead>\n    <tbody>");

        for proto in &active_protos {
            let baseline = avg_primary(&runs[0], proto);
            let _ = write!(
                out,
                "      <tr>\n        <td><strong>{proto}</strong></td>\n        <td>{metric}</td>\n",
                metric = primary_metric_label(proto),
            );
            for (i, run) in runs.iter().enumerate() {
                match avg_primary(run, proto) {
                    None => {
                        let _ = writeln!(out, "        <td>—</td>");
                    }
                    Some(v) => {
                        if i == 0 || baseline.is_none() {
                            let _ = writeln!(out, "        <td>{v:.2}</td>");
                        } else if let Some(base) = baseline {
                            // For latency metrics (lower = better): negative diff = faster.
                            // For throughput metrics (higher = better): positive diff = faster.
                            let is_throughput = matches!(
                                proto,
                                Protocol::Download
                                    | Protocol::Upload
                                    | Protocol::WebDownload
                                    | Protocol::WebUpload
                                    | Protocol::UdpDownload
                                    | Protocol::UdpUpload
                            );
                            let diff_pct = if base > 0.0 {
                                (v - base) / base * 100.0
                            } else {
                                0.0
                            };
                            // faster = lower latency OR higher throughput
                            let is_faster = if is_throughput {
                                diff_pct > 0.0
                            } else {
                                diff_pct < 0.0
                            };
                            let diff_class = if is_faster { "diff-fast" } else { "diff-slow" };
                            let sign = if diff_pct >= 0.0 { "+" } else { "" };
                            let _ = writeln!(
                                out,
                                "        <td>{v:.2}<span class=\"{diff_class}\">{sign}{diff_pct:.1}%</span></td>",
                            );
                        }
                    }
                }
            }
            let _ = writeln!(out, "      </tr>");
        }
        let _ = writeln!(out, "    </tbody>\n  </table>\n</section>");
    }

    // ── Per-target collapsible sections ───────────────────────────────────────
    for (i, run) in runs.iter().enumerate() {
        let open = if runs.len() <= 2 { " open" } else { "" };
        let _ = write!(
            out,
            "\n<details class=\"card multi-target-details\"{open}>\n  <summary><strong>Target {}:</strong> {}</summary>\n",
            i + 1,
            escape_html(&run.target_url)
        );
        write_run_sections(run, &mut out);
        let _ = writeln!(out, "</details>");
    }

    let last = runs.last().unwrap();
    write_html_footer(last.finished_at.unwrap_or(last.started_at), &mut out);
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// Structural helpers used by render() and render_multi()
// ─────────────────────────────────────────────────────────────────────────────

fn write_html_head(title: &str, css_href: Option<&str>, out: &mut String) {
    let _ = write!(
        out,
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Networker Tester – {title}</title>
  <style>{inline_css}</style>
"#,
        title = escape_html(title),
        inline_css = INLINE_CSS,
    );
    if let Some(href) = css_href {
        let _ = writeln!(
            out,
            r#"  <link rel="stylesheet" href="{}">"#,
            escape_html(href)
        );
    }
    let _ = writeln!(out, "</head>\n<body>");
}

fn write_html_footer(timestamp: DateTime<chrono::Utc>, out: &mut String) {
    let _ = write!(
        out,
        r#"
<footer>
  Generated by <strong>networker-tester v{}</strong> &bull; {}
</footer>
</body>
</html>
"#,
        env!("CARGO_PKG_VERSION"),
        timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
    );
}

/// Write all per-run content sections (page header + all data cards).
///
/// This is used by both `render()` (directly) and `render_multi()` (wrapped
/// in a per-target `<details>` block).
fn write_host_info_card(label: &str, info: &HostInfo, out: &mut String) {
    let mem = info
        .total_memory_mb
        .map(|mb| {
            if mb >= 1024 {
                format!("{:.1} GB", mb as f64 / 1024.0)
            } else {
                format!("{mb} MB")
            }
        })
        .unwrap_or_else(|| "—".into());
    let os_ver = info.os_version.as_deref().unwrap_or("—");
    let hostname = info.hostname.as_deref().unwrap_or("—");
    let version = info.server_version.as_deref().unwrap_or("—");
    let uptime = info.uptime_secs.map(|s| {
        if s >= 86400 {
            format!("{}d {}h", s / 86400, (s % 86400) / 3600)
        } else if s >= 3600 {
            format!("{}h {}m", s / 3600, (s % 3600) / 60)
        } else {
            format!("{}m {}s", s / 60, s % 60)
        }
    });

    let _ = write!(
        out,
        r##"
<section class="card" style="flex:1;min-width:280px;margin:0">
  <h2>{label} Info</h2>
  <dl class="summary-grid">
    <dt>Hostname</dt>     <dd>{hostname}</dd>
    <dt>OS</dt>           <dd>{os_ver}</dd>
    <dt>Architecture</dt> <dd>{arch}</dd>
    <dt>CPU Cores</dt>    <dd>{cpu}</dd>
    <dt>Memory</dt>       <dd>{mem}</dd>
"##,
        hostname = escape_html(hostname),
        os_ver = escape_html(os_ver),
        arch = escape_html(&info.arch),
        cpu = info.cpu_cores,
    );
    if label == "Server" {
        let _ = writeln!(
            out,
            "    <dt>Version</dt>      <dd>{}</dd>",
            escape_html(version),
        );
        if let Some(ref up) = uptime {
            let _ = writeln!(
                out,
                "    <dt>Uptime</dt>       <dd>{}</dd>",
                escape_html(up),
            );
        }
        if let Some(ref region) = info.region {
            let _ = writeln!(
                out,
                "    <dt>Region</dt>       <dd>{}</dd>",
                escape_html(region),
            );
        }
    }
    let _ = write!(out, "  </dl>\n</section>\n");
}

fn write_run_sections(run: &TestRun, out: &mut String) {
    // ── Header ────────────────────────────────────────────────────────────────
    let _ = write!(
        out,
        r#"
<header class="page-header">
  <h1>Networker Tester</h1>
  <p class="subtitle">Run <code>{run_id}</code> &bull; {started}</p>
</header>
"#,
        run_id = run.run_id,
        started = run.started_at.format("%Y-%m-%d %H:%M:%S UTC"),
    );

    // ── Summary card ──────────────────────────────────────────────────────────
    let duration_s = run
        .finished_at
        .map(|f| {
            format!(
                "{:.2}s",
                (f - run.started_at).num_milliseconds() as f64 / 1000.0
            )
        })
        .unwrap_or_else(|| "—".into());

    let server_ver = run
        .attempts
        .iter()
        .find_map(|a| {
            a.server_timing
                .as_ref()
                .and_then(|st| st.server_version.as_deref())
        })
        .unwrap_or("—");

    let _ = write!(
        out,
        r##"
<section class="card">
  <h2>Run Summary</h2>
  <dl class="summary-grid">
    <dt>Target</dt>          <dd><a href="{url}">{url}</a></dd>
    <dt>Modes</dt>           <dd>{modes}</dd>
    <dt>Attempts</dt>        <dd>{total}</dd>
    <dt>Succeeded</dt>       <dd class="ok">{ok}</dd>
    <dt>Failed</dt>          <dd class="{fail_cls}">{fail}</dd>
    <dt>Total Duration</dt>  <dd>{dur}</dd>
    <dt>Client version</dt>  <dd>{client_ver}</dd>
    <dt>Server version</dt>  <dd>{server_ver}</dd>
  </dl>
</section>
"##,
        url = escape_html(&run.target_url),
        modes = run.modes.join(", "),
        total = run.attempts.len(),
        ok = run.success_count(),
        fail = run.failure_count(),
        fail_cls = if run.failure_count() > 0 { "err" } else { "ok" },
        dur = duration_s,
        client_ver = escape_html(&run.client_version),
        server_ver = escape_html(server_ver),
    );

    // ── Client & Server Info cards ───────────────────────────────────────────
    let _ = write!(
        out,
        r##"<div style="display:flex;flex-wrap:wrap;gap:1.5rem;margin:0 2rem">"##
    );
    if let Some(ref info) = run.client_info {
        write_host_info_card("Client", info, out);
    }
    if let Some(ref info) = run.server_info {
        write_host_info_card("Server", info, out);
    }
    if let Some(ref bl) = run.baseline {
        let net_cls = match bl.network_type {
            NetworkType::Loopback => "ok",
            NetworkType::LAN => "warn",
            NetworkType::Internet => "err",
        };
        let _ = write!(
            out,
            r##"
<section class="card" style="flex:1;min-width:280px;margin:0">
  <h2>Network Baseline</h2>
  <dl class="summary-grid">
    <dt>Network Type</dt>  <dd><span class="{net_cls}">{net_type}</span></dd>
    <dt>RTT Avg</dt>       <dd>{avg:.2} ms</dd>
    <dt>RTT Min</dt>       <dd>{min:.2} ms</dd>
    <dt>RTT Max</dt>       <dd>{max:.2} ms</dd>
    <dt>RTT p50</dt>       <dd>{p50:.2} ms</dd>
    <dt>RTT p95</dt>       <dd>{p95:.2} ms</dd>
    <dt>Samples</dt>       <dd>{samples}</dd>
  </dl>
</section>
"##,
            net_cls = net_cls,
            net_type = bl.network_type,
            avg = bl.rtt_avg_ms,
            min = bl.rtt_min_ms,
            max = bl.rtt_max_ms,
            p50 = bl.rtt_p50_ms,
            p95 = bl.rtt_p95_ms,
            samples = bl.samples,
        );
    }
    let _ = writeln!(out, "</div>");

    // ── Per-protocol timing table ─────────────────────────────────────────────
    let _ = write!(
        out,
        r#"
<section class="card">
  <h2>Timing Breakdown by Protocol</h2>
  <table>
    <thead>
      <tr>
        <th>Protocol</th>
        <th>Attempts</th>
        <th>Avg DNS (ms)</th>
        <th>Avg TCP (ms)</th>
        <th>Avg TLS (ms)</th>
        <th>Avg TTFB (ms)</th>
        <th>Avg Total (ms)</th>
        <th>Success</th>
      </tr>
    </thead>
    <tbody>
"#
    );

    for proto in &[
        Protocol::Http1,
        Protocol::Http2,
        Protocol::Http3,
        Protocol::Native,
        Protocol::Curl,
        Protocol::Tcp,
        Protocol::Udp,
        Protocol::Dns,
        Protocol::Tls,
        Protocol::Download,
        Protocol::Upload,
        Protocol::WebDownload,
        Protocol::WebUpload,
        Protocol::UdpDownload,
        Protocol::UdpUpload,
        Protocol::PageLoad,
        Protocol::PageLoad2,
        Protocol::PageLoad3,
        Protocol::Browser,
    ] {
        let rows: Vec<&RequestAttempt> = run
            .attempts
            .iter()
            .filter(|a| &a.protocol == proto)
            .collect();
        if rows.is_empty() {
            continue;
        }
        append_proto_row(out, proto, &rows);
    }
    let _ = writeln!(out, "    </tbody>\n  </table>\n</section>");

    // ── Statistics summary (grouped by proto + payload) ───────────────────────
    {
        use std::collections::BTreeSet;
        let all_protos = [
            Protocol::Http1,
            Protocol::Http2,
            Protocol::Http3,
            Protocol::Native,
            Protocol::Curl,
            Protocol::Tcp,
            Protocol::Udp,
            Protocol::Dns,
            Protocol::Tls,
            Protocol::Download,
            Protocol::Upload,
            Protocol::WebDownload,
            Protocol::WebUpload,
            Protocol::UdpDownload,
            Protocol::UdpUpload,
            Protocol::PageLoad,
            Protocol::PageLoad2,
            Protocol::PageLoad3,
            Protocol::Browser,
        ];
        // Collect (proto, Option<payload>) groups in canonical order.
        let stat_groups: Vec<(Protocol, Option<usize>)> = all_protos
            .iter()
            .flat_map(|proto| {
                let payloads: BTreeSet<Option<usize>> = run
                    .attempts
                    .iter()
                    .filter(|a| a.protocol == *proto)
                    .map(attempt_payload_bytes)
                    .collect();
                payloads.into_iter().map(move |p| ((*proto).clone(), p))
            })
            .collect();

        // Build rows: one per (proto, payload) pair that has stats data.
        let stat_rows: Vec<_> = stat_groups
            .iter()
            .filter_map(|(proto, payload)| {
                let attempts: Vec<&RequestAttempt> = run
                    .attempts
                    .iter()
                    .filter(|a| &a.protocol == proto && attempt_payload_bytes(a) == *payload)
                    .collect();
                if attempts.is_empty() {
                    return None;
                }
                let total = attempts.len();
                let success = attempts.iter().filter(|a| a.success).count();
                let success_pct = success as f64 / total as f64 * 100.0;
                let vals: Vec<f64> = attempts
                    .iter()
                    .filter_map(|a| primary_metric_value(a))
                    .collect();
                let stats = compute_stats(&vals)?;
                let label = match payload {
                    None => proto.to_string(),
                    Some(b) => format!("{proto} {}", format_bytes(*b)),
                };
                Some((label, primary_metric_label(proto), stats, success_pct))
            })
            .collect();

        if !stat_rows.is_empty() {
            let _ = write!(
                out,
                r#"
<section class="card">
  <h2>Statistics Summary</h2>
  <table>
    <thead>
      <tr>
        <th>Protocol</th><th>Metric</th><th>N</th>
        <th>Min</th><th>Mean</th><th>p50</th><th>p95</th><th>p99</th>
        <th>Max</th><th>StdDev</th><th>Success %</th>
      </tr>
    </thead>
    <tbody>
"#
            );
            for (label, metric, s, success_pct) in &stat_rows {
                let ok_cls = if *success_pct >= 100.0 {
                    "ok"
                } else if *success_pct >= 80.0 {
                    "warn"
                } else {
                    "err"
                };
                let _ = write!(
                    out,
                    r#"      <tr>
        <td>{label}</td>
        <td>{metric}</td>
        <td>{count}</td>
        <td>{min:.2}</td>
        <td>{mean:.2}</td>
        <td>{p50:.2}</td>
        <td>{p95:.2}</td>
        <td>{p99:.2}</td>
        <td>{max:.2}</td>
        <td>{stddev:.2}</td>
        <td class="{ok_cls}">{pct:.0}%</td>
      </tr>
"#,
                    count = s.count,
                    min = s.min,
                    mean = s.mean,
                    p50 = s.p50,
                    p95 = s.p95,
                    p99 = s.p99,
                    max = s.max,
                    stddev = s.stddev,
                    pct = success_pct,
                );
            }
            let _ = writeln!(out, "    </tbody>\n  </table>\n</section>");
        }
    }

    // ── Protocol Comparison (Page Load) ──────────────────────────────────────
    {
        use crate::metrics::compute_stats;
        let pl_attempts: Vec<&RequestAttempt> = run
            .attempts
            .iter()
            .filter(|a| {
                matches!(
                    a.protocol,
                    Protocol::PageLoad | Protocol::PageLoad2 | Protocol::PageLoad3
                ) && a.page_load.is_some()
            })
            .collect();
        if !pl_attempts.is_empty() {
            let _ = write!(
                out,
                r#"
<section class="card">
  <h2>Protocol Comparison – Page Load</h2>
  <table>
    <thead>
      <tr>
        <th>Protocol</th><th>N</th><th>Assets</th>
        <th>Avg Conns</th><th>TLS Setup (ms)</th><th>TLS Overhead</th>
        <th>CPU (ms)</th><th>p50 Total (ms)</th><th>Min (ms)</th><th>Max (ms)</th>
      </tr>
    </thead>
    <tbody>
"#
            );
            for proto in &[Protocol::PageLoad, Protocol::PageLoad2, Protocol::PageLoad3] {
                let rows: Vec<&RequestAttempt> = pl_attempts
                    .iter()
                    .filter(|a| &a.protocol == proto)
                    .copied()
                    .collect();
                if rows.is_empty() {
                    continue;
                }
                let n = rows.len();
                let pl_rows: Vec<&crate::metrics::PageLoadResult> =
                    rows.iter().filter_map(|a| a.page_load.as_ref()).collect();
                let avg_conns = pl_rows
                    .iter()
                    .map(|p| p.connections_opened as f64)
                    .sum::<f64>()
                    / n as f64;
                let avg_fetched =
                    pl_rows.iter().map(|p| p.assets_fetched as f64).sum::<f64>() / n as f64;
                let total_assets = pl_rows.first().map(|p| p.asset_count).unwrap_or(0);
                let avg_tls_ms: f64 =
                    pl_rows.iter().map(|p| p.tls_setup_ms).sum::<f64>() / n as f64;
                let avg_tls_pct: f64 = pl_rows
                    .iter()
                    .map(|p| p.tls_overhead_ratio * 100.0)
                    .sum::<f64>()
                    / n as f64;
                let cpu_vals: Vec<f64> = pl_rows.iter().filter_map(|p| p.cpu_time_ms).collect();
                let cpu_display = if cpu_vals.is_empty() {
                    "—".to_string()
                } else {
                    format!(
                        "{:.2}",
                        cpu_vals.iter().sum::<f64>() / cpu_vals.len() as f64
                    )
                };
                let total_ms_vals: Vec<f64> = pl_rows.iter().map(|p| p.total_ms).collect();
                if let Some(s) = compute_stats(&total_ms_vals) {
                    let _ = write!(
                        out,
                        r#"      <tr>
        <td>{proto}</td>
        <td>{n}</td>
        <td>{fetched:.0}/{total}</td>
        <td>{conns:.1}</td>
        <td>{tls_ms:.2}</td>
        <td>{tls_pct:.1}%</td>
        <td>{cpu}</td>
        <td>{p50:.2}</td>
        <td>{min:.2}</td>
        <td>{max:.2}</td>
      </tr>
"#,
                        proto = proto,
                        n = n,
                        fetched = avg_fetched,
                        total = total_assets,
                        conns = avg_conns,
                        tls_ms = avg_tls_ms,
                        tls_pct = avg_tls_pct,
                        cpu = cpu_display,
                        p50 = s.p50,
                        min = s.min,
                        max = s.max,
                    );
                }
            }
            let _ = writeln!(out, "    </tbody>\n  </table>\n</section>");
        }
    }

    // ── Protocol Comparison (Browser) ─────────────────────────────────────────
    {
        let br_cmp_attempts: Vec<&RequestAttempt> = run
            .attempts
            .iter()
            .filter(|a| {
                matches!(
                    a.protocol,
                    Protocol::Browser
                        | Protocol::Browser1
                        | Protocol::Browser2
                        | Protocol::Browser3
                ) && a.browser.is_some()
            })
            .collect();
        if !br_cmp_attempts.is_empty() {
            let _ = write!(
                out,
                r#"
<section class="card">
  <h2>Protocol Comparison – Browser</h2>
  <table>
    <thead>
      <tr>
        <th>Protocol</th><th>N</th>
        <th>Avg TTFB (ms)</th><th>Avg DCL (ms)</th><th>Avg Load (ms)</th>
        <th>p50 (ms)</th><th>Min (ms)</th><th>Max (ms)</th>
        <th>Avg Resources</th><th>Avg Bytes</th>
      </tr>
    </thead>
    <tbody>
"#
            );
            for proto in &[
                Protocol::Browser1,
                Protocol::Browser2,
                Protocol::Browser3,
                Protocol::Browser,
            ] {
                let rows: Vec<&RequestAttempt> = br_cmp_attempts
                    .iter()
                    .filter(|a| &a.protocol == proto)
                    .copied()
                    .collect();
                if rows.is_empty() {
                    continue;
                }
                let n = rows.len();
                let br_data: Vec<_> = rows.iter().filter_map(|a| a.browser.as_ref()).collect();
                let avg_ttfb = br_data.iter().map(|b| b.ttfb_ms).sum::<f64>() / n as f64;
                let avg_dcl =
                    br_data.iter().map(|b| b.dom_content_loaded_ms).sum::<f64>() / n as f64;
                let load_vals: Vec<f64> = br_data.iter().map(|b| b.load_ms).collect();
                let avg_res =
                    br_data.iter().map(|b| b.resource_count as f64).sum::<f64>() / n as f64;
                let avg_bytes = br_data
                    .iter()
                    .map(|b| b.transferred_bytes as f64)
                    .sum::<f64>()
                    / n as f64;
                if let Some(s) = compute_stats(&load_vals) {
                    let _ = write!(
                        out,
                        r#"      <tr>
        <td>{proto}</td>
        <td>{n}</td>
        <td>{ttfb:.2}</td>
        <td>{dcl:.2}</td>
        <td>{load:.2}</td>
        <td>{p50:.2}</td>
        <td>{min:.2}</td>
        <td>{max:.2}</td>
        <td>{res:.1}</td>
        <td>{bytes}</td>
      </tr>
"#,
                        proto = proto,
                        n = n,
                        ttfb = avg_ttfb,
                        dcl = avg_dcl,
                        load = s.mean,
                        p50 = s.p50,
                        min = s.min,
                        max = s.max,
                        res = avg_res,
                        bytes = format_bytes(avg_bytes as usize),
                    );
                }
            }
            let _ = writeln!(out, "    </tbody>\n  </table>\n</section>");
        }
    }

    // ── Browser Results ───────────────────────────────────────────────────────
    {
        let browser_rows: Vec<&RequestAttempt> = run
            .attempts
            .iter()
            .filter(|a| {
                matches!(
                    a.protocol,
                    Protocol::Browser
                        | Protocol::Browser1
                        | Protocol::Browser2
                        | Protocol::Browser3
                ) && a.browser.is_some()
            })
            .collect();
        if !browser_rows.is_empty() {
            let open_attr = if browser_rows.len() <= 20 {
                " open"
            } else {
                ""
            };
            let _ = write!(
                out,
                r#"
<section class="card">
  <h2>Browser Results</h2>
  <details{open}>
    <summary><span class="grp-lbl">{n} attempts</span></summary>
    <table>
      <thead>
        <tr>
          <th>#</th>
          <th>Mode</th>
          <th>Protocol (main)</th>
          <th>TTFB (ms)</th>
          <th>DCL (ms)</th>
          <th>Load (ms)</th>
          <th>Resources</th>
          <th>Bytes</th>
          <th>Protocols</th>
        </tr>
      </thead>
      <tbody>
"#,
                open = open_attr,
                n = browser_rows.len(),
            );
            for a in &browser_rows {
                let b = a.browser.as_ref().unwrap();
                let proto_summary = b
                    .resource_protocols
                    .iter()
                    .map(|(p, n)| format!("{p}×{n}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                let _ = write!(
                    out,
                    r#"        <tr>
          <td>{seq}</td>
          <td><code>{mode}</code></td>
          <td><code>{proto}</code></td>
          <td>{ttfb:.2}</td>
          <td>{dcl:.2}</td>
          <td>{load:.2}</td>
          <td>{res}</td>
          <td>{bytes}</td>
          <td>{protos}</td>
        </tr>
"#,
                    seq = a.sequence_num,
                    mode = escape_html(&a.protocol.to_string()),
                    proto = escape_html(&b.protocol),
                    ttfb = b.ttfb_ms,
                    dcl = b.dom_content_loaded_ms,
                    load = b.load_ms,
                    res = b.resource_count,
                    bytes = b.transferred_bytes,
                    protos = if proto_summary.is_empty() {
                        "—".to_string()
                    } else {
                        escape_html(&proto_summary)
                    },
                );
            }
            let _ = writeln!(
                out,
                "      </tbody>\n    </table>\n  </details>\n</section>"
            );
        }
    }

    // ── Charts + Analysis ─────────────────────────────────────────────────────
    {
        let chart_browser: Vec<&RequestAttempt> = run
            .attempts
            .iter()
            .filter(|a| {
                matches!(
                    a.protocol,
                    Protocol::Browser
                        | Protocol::Browser1
                        | Protocol::Browser2
                        | Protocol::Browser3
                ) && a.browser.is_some()
            })
            .collect();
        let chart_pl: Vec<&RequestAttempt> = run
            .attempts
            .iter()
            .filter(|a| {
                matches!(
                    a.protocol,
                    Protocol::PageLoad | Protocol::PageLoad2 | Protocol::PageLoad3
                ) && a.page_load.is_some()
            })
            .collect();
        let has_throughput = run.attempts.iter().any(|a| {
            matches!(
                a.protocol,
                Protocol::Download | Protocol::Upload | Protocol::WebDownload | Protocol::WebUpload
            ) && a
                .http
                .as_ref()
                .map(|h| h.throughput_mbps.is_some())
                .unwrap_or(false)
        });

        if !chart_browser.is_empty() || !chart_pl.is_empty() {
            let _ = writeln!(
                out,
                "<section class=\"card\">\n  <h2>Charts &amp; Analysis</h2>\n  <div class=\"charts-grid\">"
            );

            // Chart 1: Page Load Time — All Protocols
            {
                let mut bars: Vec<(String, f64, &'static str)> = Vec::new();
                for (proto, color) in [
                    (Protocol::Browser1, "#e07b39"),
                    (Protocol::Browser2, "#4e79a7"),
                    (Protocol::Browser3, "#59a14f"),
                    (Protocol::Browser, "#8c6bb1"),
                ] {
                    let data: Vec<f64> = chart_browser
                        .iter()
                        .filter(|a| a.protocol == proto)
                        .filter_map(|a| a.browser.as_ref().map(|b| b.load_ms))
                        .collect();
                    if !data.is_empty() {
                        let avg = data.iter().sum::<f64>() / data.len() as f64;
                        bars.push((proto.to_string(), avg, color));
                    }
                }
                for (proto, color) in [
                    (Protocol::PageLoad, "#e07b39"),
                    (Protocol::PageLoad2, "#4e79a7"),
                    (Protocol::PageLoad3, "#59a14f"),
                ] {
                    let data: Vec<f64> = chart_pl
                        .iter()
                        .filter(|a| a.protocol == proto)
                        .filter_map(|a| a.page_load.as_ref().map(|p| p.total_ms))
                        .collect();
                    if !data.is_empty() {
                        let avg = data.iter().sum::<f64>() / data.len() as f64;
                        bars.push((proto.to_string(), avg, color));
                    }
                }
                if !bars.is_empty() {
                    bars.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                    let bar_refs: Vec<(&str, f64)> =
                        bars.iter().map(|(l, v, _)| (l.as_str(), *v)).collect();
                    let color_refs: Vec<&str> = bars.iter().map(|(_, _, c)| *c).collect();
                    let svg = svg_hbar(
                        "Page Load Time (ms) \u{2014} All Protocols",
                        &bar_refs,
                        "ms",
                        &color_refs,
                    );
                    let _ = writeln!(out, "<div>{}</div>", svg);
                }
            }

            // Chart 2: Browser TTFB / DCL / Load Breakdown
            if !chart_browser.is_empty() {
                let mut bar_labels: Vec<String> = Vec::new();
                let mut bar_values: Vec<f64> = Vec::new();
                let mut bar_colors: Vec<&'static str> = Vec::new();
                for (proto, ttfb_c, dcl_c, load_c) in [
                    (Protocol::Browser1, "#f0b98d", "#e07b39", "#b85c1f"),
                    (Protocol::Browser2, "#91b8d4", "#4e79a7", "#2c5077"),
                    (Protocol::Browser3, "#95cf8b", "#59a14f", "#38712f"),
                    (Protocol::Browser, "#c9b3d5", "#8c6bb1", "#614080"),
                ] {
                    let data: Vec<_> = chart_browser
                        .iter()
                        .filter(|a| a.protocol == proto)
                        .filter_map(|a| a.browser.as_ref())
                        .collect();
                    if data.is_empty() {
                        continue;
                    }
                    let label = proto.to_string();
                    let n = data.len() as f64;
                    let avg_ttfb = data.iter().map(|b| b.ttfb_ms).sum::<f64>() / n;
                    let avg_dcl = data.iter().map(|b| b.dom_content_loaded_ms).sum::<f64>() / n;
                    let avg_load = data.iter().map(|b| b.load_ms).sum::<f64>() / n;
                    bar_labels.push(format!("{label} TTFB"));
                    bar_values.push(avg_ttfb);
                    bar_colors.push(ttfb_c);
                    bar_labels.push(format!("{label} DCL"));
                    bar_values.push(avg_dcl);
                    bar_colors.push(dcl_c);
                    bar_labels.push(format!("{label} Load"));
                    bar_values.push(avg_load);
                    bar_colors.push(load_c);
                }
                if !bar_values.is_empty() {
                    let bars: Vec<(&str, f64)> = bar_labels
                        .iter()
                        .zip(bar_values.iter())
                        .map(|(l, v)| (l.as_str(), *v))
                        .collect();
                    let svg = svg_hbar(
                        "Browser TTFB / DCL / Load Breakdown",
                        &bars,
                        "ms",
                        &bar_colors,
                    );
                    let _ = writeln!(out, "<div>{}</div>", svg);
                }
            }

            // Chart 3: Load Time Distribution (box-and-whisker) — All Protocols
            // Inspired by Paper 1 (GLOBECOM 2016) which uses box plots to show
            // that variance — not just average — matters when comparing protocols.
            // Includes real-browser and synthetic pageload side-by-side.
            {
                let mut groups: Vec<(String, Vec<f64>, &'static str)> = Vec::new();
                for (proto, color) in [
                    (Protocol::Browser1, "#e07b39"),
                    (Protocol::Browser2, "#4e79a7"),
                    (Protocol::Browser3, "#59a14f"),
                    (Protocol::Browser, "#8c6bb1"),
                ] {
                    let data: Vec<f64> = chart_browser
                        .iter()
                        .filter(|a| a.protocol == proto)
                        .filter_map(|a| a.browser.as_ref().map(|b| b.load_ms))
                        .collect();
                    if data.len() >= 4 {
                        groups.push((proto.to_string(), data, color));
                    }
                }
                for (proto, color) in [
                    (Protocol::PageLoad, "#e07b39"),
                    (Protocol::PageLoad2, "#4e79a7"),
                    (Protocol::PageLoad3, "#59a14f"),
                ] {
                    let data: Vec<f64> = chart_pl
                        .iter()
                        .filter(|a| a.protocol == proto)
                        .filter_map(|a| a.page_load.as_ref().map(|p| p.total_ms))
                        .collect();
                    if data.len() >= 4 {
                        groups.push((proto.to_string(), data, color));
                    }
                }
                if !groups.is_empty() {
                    let group_refs: Vec<(&str, &[f64], &str)> = groups
                        .iter()
                        .map(|(l, v, c)| (l.as_str(), v.as_slice(), *c))
                        .collect();
                    let svg = svg_boxplot(
                        "Load Time Distribution \u{2014} All Protocols (p5\u{2013}p95, IQR box)",
                        &group_refs,
                        "ms",
                    );
                    let _ = writeln!(out, "<div>{}</div>", svg);
                }
            }

            // Chart 3b: TTFB Distribution (box-and-whisker) — Browser + PageLoad
            // TTFB is the first-byte latency; shows network RTT + server processing variance.
            {
                let mut groups: Vec<(String, Vec<f64>, &'static str)> = Vec::new();
                for (proto, color) in [
                    (Protocol::Browser1, "#e07b39"),
                    (Protocol::Browser2, "#4e79a7"),
                    (Protocol::Browser3, "#59a14f"),
                    (Protocol::Browser, "#8c6bb1"),
                ] {
                    let data: Vec<f64> = chart_browser
                        .iter()
                        .filter(|a| a.protocol == proto)
                        .filter_map(|a| a.browser.as_ref().map(|b| b.ttfb_ms))
                        .collect();
                    if data.len() >= 4 {
                        groups.push((proto.to_string(), data, color));
                    }
                }
                for (proto, color) in [
                    (Protocol::PageLoad, "#e07b39"),
                    (Protocol::PageLoad2, "#4e79a7"),
                    (Protocol::PageLoad3, "#59a14f"),
                ] {
                    let data: Vec<f64> = chart_pl
                        .iter()
                        .filter(|a| a.protocol == proto)
                        .filter_map(|a| a.page_load.as_ref().map(|p| p.ttfb_ms))
                        .collect();
                    if data.len() >= 4 {
                        groups.push((proto.to_string(), data, color));
                    }
                }
                if !groups.is_empty() {
                    let group_refs: Vec<(&str, &[f64], &str)> = groups
                        .iter()
                        .map(|(l, v, c)| (l.as_str(), v.as_slice(), *c))
                        .collect();
                    let svg = svg_boxplot(
                        "TTFB Distribution \u{2014} All Protocols (p5\u{2013}p95, IQR box)",
                        &group_refs,
                        "ms",
                    );
                    let _ = writeln!(out, "<div>{}</div>", svg);
                }
            }

            // Chart 3c: DOM Content Loaded Distribution (box-and-whisker) — Browser only
            // DCL fires when the HTML is parsed; shows rendering-pipeline variance across H1/H2/H3.
            if !chart_browser.is_empty() {
                let mut groups: Vec<(String, Vec<f64>, &'static str)> = Vec::new();
                for (proto, color) in [
                    (Protocol::Browser1, "#e07b39"),
                    (Protocol::Browser2, "#4e79a7"),
                    (Protocol::Browser3, "#59a14f"),
                    (Protocol::Browser, "#8c6bb1"),
                ] {
                    let data: Vec<f64> = chart_browser
                        .iter()
                        .filter(|a| a.protocol == proto)
                        .filter_map(|a| a.browser.as_ref().map(|b| b.dom_content_loaded_ms))
                        .collect();
                    if data.len() >= 4 {
                        groups.push((proto.to_string(), data, color));
                    }
                }
                if !groups.is_empty() {
                    let group_refs: Vec<(&str, &[f64], &str)> = groups
                        .iter()
                        .map(|(l, v, c)| (l.as_str(), v.as_slice(), *c))
                        .collect();
                    let svg = svg_boxplot(
                        "DOM Content Loaded Distribution (p5\u{2013}p95, IQR box)",
                        &group_refs,
                        "ms",
                    );
                    let _ = writeln!(out, "<div>{}</div>", svg);
                }
            }

            // Chart 4: Browser Load Time CDF
            // Inspired by Paper 1 (GLOBECOM 2016) Figure 6 / Figure 7 — CDF plots
            // show whether a protocol is consistently faster or only occasionally faster.
            {
                let mut series: Vec<(String, Vec<f64>, &'static str)> = Vec::new();
                for (proto, color) in [
                    (Protocol::Browser1, "#e07b39"),
                    (Protocol::Browser2, "#4e79a7"),
                    (Protocol::Browser3, "#59a14f"),
                    (Protocol::Browser, "#8c6bb1"),
                ] {
                    let data: Vec<f64> = chart_browser
                        .iter()
                        .filter(|a| a.protocol == proto)
                        .filter_map(|a| a.browser.as_ref().map(|b| b.load_ms))
                        .collect();
                    if data.len() >= 2 {
                        series.push((proto.to_string(), data, color));
                    }
                }
                // Also add pageload series so real-browser vs synthetic comparison is visible.
                for (proto, color) in [
                    (Protocol::PageLoad, "#e07b39"),
                    (Protocol::PageLoad2, "#4e79a7"),
                    (Protocol::PageLoad3, "#59a14f"),
                ] {
                    let data: Vec<f64> = chart_pl
                        .iter()
                        .filter(|a| a.protocol == proto)
                        .filter_map(|a| a.page_load.as_ref().map(|p| p.total_ms))
                        .collect();
                    if data.len() >= 2 {
                        series.push((proto.to_string(), data, color));
                    }
                }
                if series.len() >= 2 {
                    let series_refs: Vec<(&str, &[f64], &str)> = series
                        .iter()
                        .map(|(l, v, c)| (l.as_str(), v.as_slice(), *c))
                        .collect();
                    let svg = svg_cdf("Load Time CDF \u{2014} All Protocols", &series_refs, "ms");
                    let _ = writeln!(out, "<div>{}</div>", svg);
                }
            }

            // Chart 5: Throughput by Protocol (MB/s)
            if has_throughput {
                use std::collections::BTreeSet;
                let tp_attempts: Vec<&RequestAttempt> = run
                    .attempts
                    .iter()
                    .filter(|a| {
                        matches!(
                            a.protocol,
                            Protocol::Download
                                | Protocol::Upload
                                | Protocol::WebDownload
                                | Protocol::WebUpload
                        ) && a.http.is_some()
                    })
                    .collect();
                let mut bars: Vec<(String, f64, &'static str)> = Vec::new();
                for (proto, color) in [
                    (Protocol::Download, "#4e79a7"),
                    (Protocol::Upload, "#e07b39"),
                    (Protocol::WebDownload, "#59a14f"),
                    (Protocol::WebUpload, "#8c6bb1"),
                ] {
                    let payloads: BTreeSet<usize> = tp_attempts
                        .iter()
                        .filter(|a| a.protocol == proto)
                        .filter_map(|a| a.http.as_ref().map(|h| h.payload_bytes))
                        .filter(|&b| b > 0)
                        .collect();
                    for payload_bytes in payloads {
                        let data: Vec<f64> = tp_attempts
                            .iter()
                            .filter(|a| {
                                a.protocol == proto
                                    && a.http.as_ref().map(|h| h.payload_bytes)
                                        == Some(payload_bytes)
                            })
                            .filter_map(|a| a.http.as_ref().and_then(|h| h.throughput_mbps))
                            .collect();
                        if !data.is_empty() {
                            let avg = data.iter().sum::<f64>() / data.len() as f64;
                            bars.push((
                                format!("{proto} {}", format_bytes(payload_bytes)),
                                avg,
                                color,
                            ));
                        }
                    }
                }
                if !bars.is_empty() {
                    let bar_refs: Vec<(&str, f64)> =
                        bars.iter().map(|(l, v, _)| (l.as_str(), *v)).collect();
                    let color_refs: Vec<&str> = bars.iter().map(|(_, _, c)| *c).collect();
                    let svg = svg_hbar(
                        "Throughput by Protocol (MB/s)",
                        &bar_refs,
                        "MB/s",
                        &color_refs,
                    );
                    let _ = writeln!(out, "<div>{}</div>", svg);
                }
            }

            let _ = writeln!(out, "  </div>"); // close charts-grid

            // Analysis observations
            {
                let mut observations: Vec<String> = Vec::new();

                // Fastest browser mode
                let mut fastest: Option<(String, f64)> = None;
                for proto in [
                    Protocol::Browser1,
                    Protocol::Browser2,
                    Protocol::Browser3,
                    Protocol::Browser,
                ] {
                    let data: Vec<f64> = chart_browser
                        .iter()
                        .filter(|a| a.protocol == proto)
                        .filter_map(|a| a.browser.as_ref().map(|b| b.load_ms))
                        .collect();
                    if !data.is_empty() {
                        let avg = data.iter().sum::<f64>() / data.len() as f64;
                        if fastest.as_ref().map(|(_, v)| avg < *v).unwrap_or(true) {
                            fastest = Some((proto.to_string(), avg));
                        }
                    }
                }
                if let Some((proto, ms)) = fastest {
                    observations.push(format!(
                        "Fastest browser mode: <strong>{proto}</strong> \u{2014} {ms:.1}ms avg load time"
                    ));
                }

                // H3 vs H2 comparison
                let br2_avg: Option<f64> = {
                    let data: Vec<f64> = chart_browser
                        .iter()
                        .filter(|a| a.protocol == Protocol::Browser2)
                        .filter_map(|a| a.browser.as_ref().map(|b| b.load_ms))
                        .collect();
                    if data.is_empty() {
                        None
                    } else {
                        Some(data.iter().sum::<f64>() / data.len() as f64)
                    }
                };
                let br3_avg: Option<f64> = {
                    let data: Vec<f64> = chart_browser
                        .iter()
                        .filter(|a| a.protocol == Protocol::Browser3)
                        .filter_map(|a| a.browser.as_ref().map(|b| b.load_ms))
                        .collect();
                    if data.is_empty() {
                        None
                    } else {
                        Some(data.iter().sum::<f64>() / data.len() as f64)
                    }
                };
                if let (Some(h2), Some(h3)) = (br2_avg, br3_avg) {
                    let diff = h2 - h3;
                    let pct = diff / h2 * 100.0;
                    if diff > 0.0 {
                        observations.push(format!(
                            "H3 is {:.1}ms ({:.1}%) faster than H2 ({:.1}ms vs {:.1}ms avg load)",
                            diff, pct, h3, h2
                        ));
                    } else {
                        observations.push(format!(
                            "H2 is {:.1}ms ({:.1}%) faster than H3 ({:.1}ms vs {:.1}ms avg load)",
                            -diff, -pct, h2, h3
                        ));
                    }
                }

                // TTFB leader (Paper 2 highlight: H3 has lower TTFB in most conditions)
                {
                    let mut ttfb_vals: Vec<(String, f64)> = Vec::new();
                    for proto in [
                        Protocol::Browser1,
                        Protocol::Browser2,
                        Protocol::Browser3,
                        Protocol::Browser,
                    ] {
                        let data: Vec<f64> = chart_browser
                            .iter()
                            .filter(|a| a.protocol == proto)
                            .filter_map(|a| a.browser.as_ref().map(|b| b.ttfb_ms))
                            .collect();
                        if !data.is_empty() {
                            let avg = data.iter().sum::<f64>() / data.len() as f64;
                            ttfb_vals.push((proto.to_string(), avg));
                        }
                    }
                    if ttfb_vals.len() >= 2 {
                        if let Some((best, best_v)) = ttfb_vals.iter().min_by(|a, b| {
                            a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
                        }) {
                            let worst_v = ttfb_vals.iter().map(|(_, v)| *v).fold(0.0_f64, f64::max);
                            observations.push(format!(
                                "Lowest TTFB: <strong>{best}</strong> at {best_v:.1}ms \
                                 ({:.1}ms advantage over slowest protocol)",
                                worst_v - best_v
                            ));
                        }
                    }
                }

                // Consistency: p95−p50 spread (Paper 1: variance matters as much as average)
                {
                    let mut spreads: Vec<(String, f64)> = Vec::new();
                    for proto in [
                        Protocol::Browser1,
                        Protocol::Browser2,
                        Protocol::Browser3,
                        Protocol::Browser,
                    ] {
                        let mut data: Vec<f64> = chart_browser
                            .iter()
                            .filter(|a| a.protocol == proto)
                            .filter_map(|a| a.browser.as_ref().map(|b| b.load_ms))
                            .collect();
                        if data.len() >= 4 {
                            data.sort_by(|a, b| {
                                a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                            });
                            let n = data.len();
                            let p50 = data[((n as f64 * 0.50).round() as usize).min(n - 1)];
                            let p95 = data[((n as f64 * 0.95).round() as usize).min(n - 1)];
                            spreads.push((proto.to_string(), p95 - p50));
                        }
                    }
                    if spreads.len() >= 2 {
                        spreads.sort_by(|a, b| {
                            a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
                        });
                        let (stable, stable_sp) = &spreads[0];
                        let (noisy, noisy_sp) = spreads.last().unwrap();
                        observations.push(format!(
                            "Consistency (p95\u{2212}p50): \
                             <strong>{stable}</strong> most stable ({stable_sp:.1}ms spread) vs \
                             <strong>{noisy}</strong> ({noisy_sp:.1}ms spread)"
                        ));
                    }
                }

                // Real browser vs synthetic (browser3 vs pageload3)
                let pl3_avg: Option<f64> = {
                    let data: Vec<f64> = chart_pl
                        .iter()
                        .filter(|a| a.protocol == Protocol::PageLoad3)
                        .filter_map(|a| a.page_load.as_ref().map(|p| p.total_ms))
                        .collect();
                    if data.is_empty() {
                        None
                    } else {
                        Some(data.iter().sum::<f64>() / data.len() as f64)
                    }
                };
                if let (Some(br3), Some(pl3)) = (br3_avg, pl3_avg) {
                    let overhead = br3 - pl3;
                    let pct = overhead / pl3 * 100.0;
                    observations.push(format!(
                        "Real browser (browser3) overhead vs synthetic (pageload3): {overhead:+.1}ms ({pct:+.1}%)"
                    ));
                }

                // Resource protocol breakdown from last browser run
                if let Some(last_br) = chart_browser.last() {
                    if let Some(b) = &last_br.browser {
                        if !b.resource_protocols.is_empty() {
                            let total_res: u32 = b.resource_protocols.iter().map(|(_, n)| n).sum();
                            if b.resource_protocols.len() == 1 {
                                let (p, _) = &b.resource_protocols[0];
                                observations.push(format!(
                                    "All {} resources via {} (last run)",
                                    total_res,
                                    escape_html(p)
                                ));
                            } else {
                                let proto_summary = b
                                    .resource_protocols
                                    .iter()
                                    .map(|(p, n)| format!("{}×{}", escape_html(p), n))
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                observations.push(format!(
                                    "Resource protocols (last run): {proto_summary}"
                                ));
                            }
                        }
                    }
                }

                if !observations.is_empty() {
                    let _ = writeln!(out, "  <div class=\"analysis\"><h3>Observations</h3><ul>");
                    for obs in &observations {
                        let _ = writeln!(out, "    <li>{obs}</li>");
                    }
                    let _ = writeln!(out, "  </ul></div>");
                }
            }

            let _ = writeln!(out, "</section>");
        }
    }

    // ── UDP statistics ────────────────────────────────────────────────────────
    let udp_rows: Vec<&RequestAttempt> = run
        .attempts
        .iter()
        .filter(|a| a.protocol == Protocol::Udp && a.udp.is_some())
        .collect();
    if !udp_rows.is_empty() {
        let _ = write!(
            out,
            r#"
<section class="card">
  <h2>UDP Probe Statistics</h2>
  <table>
    <thead>
      <tr><th>Run #</th><th>Target</th><th>Sent</th><th>Recv</th><th>Loss %</th>
          <th>Min RTT</th><th>Avg RTT</th><th>P95 RTT</th><th>Jitter</th></tr>
    </thead>
    <tbody>
"#
        );
        for a in &udp_rows {
            let u = a.udp.as_ref().unwrap();
            let _ = write!(
                out,
                r#"      <tr>
        <td>{seq}</td>
        <td>{addr}</td>
        <td>{sent}</td>
        <td>{recv}</td>
        <td class="{loss_cls}">{loss:.1}%</td>
        <td>{min:.2}ms</td>
        <td>{avg:.2}ms</td>
        <td>{p95:.2}ms</td>
        <td>{jitter:.2}ms</td>
      </tr>
"#,
                seq = a.sequence_num,
                addr = escape_html(&u.remote_addr),
                sent = u.probe_count,
                recv = u.success_count,
                loss = u.loss_percent,
                loss_cls = if u.loss_percent > 0.0 { "warn" } else { "ok" },
                min = u.rtt_min_ms,
                avg = u.rtt_avg_ms,
                p95 = u.rtt_p95_ms,
                jitter = u.jitter_ms,
            );
        }
        let _ = writeln!(out, "    </tbody>\n  </table>\n</section>");
    }

    // ── Throughput results (collapsible, grouped by proto+payload) ───────────
    {
        use std::collections::BTreeSet;
        let throughput_rows: Vec<&RequestAttempt> = run
            .attempts
            .iter()
            .filter(|a| {
                matches!(
                    a.protocol,
                    Protocol::Download
                        | Protocol::Upload
                        | Protocol::WebDownload
                        | Protocol::WebUpload
                ) && a.http.is_some()
            })
            .collect();
        if !throughput_rows.is_empty() {
            let _ = writeln!(
                out,
                "\n<section class=\"card\">\n  <h2>Throughput Results</h2>"
            );

            // Collect distinct (proto, payload_bytes) pairs in order.
            let groups: Vec<(Protocol, usize)> = {
                let mut seen = BTreeSet::new();
                let protos_order = [
                    Protocol::Download,
                    Protocol::Upload,
                    Protocol::WebDownload,
                    Protocol::WebUpload,
                ];
                protos_order
                    .iter()
                    .flat_map(|proto| {
                        let mut payloads: Vec<usize> = throughput_rows
                            .iter()
                            .filter(|a| &a.protocol == proto)
                            .filter_map(|a| a.http.as_ref().map(|h| h.payload_bytes))
                            .filter(|&b| b > 0)
                            .collect::<BTreeSet<_>>()
                            .into_iter()
                            .collect();
                        payloads.retain(|p| seen.insert((proto.to_string(), *p)));
                        payloads
                            .into_iter()
                            .map(move |p| (proto.clone(), p))
                            .collect::<Vec<_>>()
                    })
                    .collect()
            };

            let single_small = groups.len() == 1
                && throughput_rows
                    .iter()
                    .filter(|a| {
                        groups
                            .first()
                            .map(|(p, b)| {
                                &a.protocol == p
                                    && a.http.as_ref().map(|h| h.payload_bytes) == Some(*b)
                            })
                            .unwrap_or(false)
                    })
                    .count()
                    <= 20;

            for (proto, payload_bytes) in &groups {
                let group_rows: Vec<&&RequestAttempt> = throughput_rows
                    .iter()
                    .filter(|a| {
                        &a.protocol == proto
                            && a.http.as_ref().map(|h| h.payload_bytes) == Some(*payload_bytes)
                    })
                    .collect();
                let n = group_rows.len();
                let mbps_vals: Vec<f64> = group_rows
                    .iter()
                    .filter_map(|a| a.http.as_ref().and_then(|h| h.throughput_mbps))
                    .collect();
                let stats = compute_stats(&mbps_vals);
                let summary_meta = if let Some(ref s) = stats {
                    format!(
                        "{n} runs · avg {avg:.1} MB/s · ±{sd:.1} · min {min:.1} · max {max:.1}",
                        avg = s.mean,
                        sd = s.stddev,
                        min = s.min,
                        max = s.max,
                    )
                } else {
                    format!("{n} runs")
                };
                let grp_label = format!("{proto} {}", format_bytes(*payload_bytes));
                let open_attr = if single_small { " open" } else { "" };
                let _ = write!(
                    out,
                    r#"  <details{open}>
    <summary><span class="grp-lbl">{lbl}</span><span class="grp-meta">{meta}</span></summary>
    <table>
      <thead>
        <tr><th>Run #</th><th>Mode</th><th>Payload</th><th>Throughput (MB/s)</th>
            <th>Goodput (MB/s)</th><th>TTFB (ms)</th><th>Total (ms)</th>
            <th>CPU (ms)</th><th>Client CSW (v/i)</th><th>Server CSW (v/i)</th>
            <th>Status</th></tr>
      </thead>
      <tbody>
"#,
                    open = open_attr,
                    lbl = escape_html(&grp_label),
                    meta = escape_html(&summary_meta),
                );
                for a in &group_rows {
                    let h = a.http.as_ref().unwrap();
                    let throughput = h
                        .throughput_mbps
                        .map(|m| format!("{m:.2}"))
                        .unwrap_or_else(|| "—".into());
                    let goodput = h
                        .goodput_mbps
                        .map(|g| format!("{g:.2}"))
                        .unwrap_or_else(|| "—".into());
                    let cpu = h
                        .cpu_time_ms
                        .map(|c| format!("{c:.1}"))
                        .unwrap_or_else(|| "—".into());
                    let client_csw = match (h.csw_voluntary, h.csw_involuntary) {
                        (Some(v), Some(i)) => format!("{v}/{i}"),
                        _ => "—".into(),
                    };
                    let server_csw = match a.server_timing.as_ref() {
                        Some(st) => match (st.srv_csw_voluntary, st.srv_csw_involuntary) {
                            (Some(v), Some(i)) => format!("{v}/{i}"),
                            _ => "—".into(),
                        },
                        None => "—".into(),
                    };
                    let status_cell = {
                        let cls = if h.status_code < 400 { "ok" } else { "err" };
                        format!(r#"<span class="{cls}">{}</span>"#, h.status_code)
                    };
                    let _ = write!(
                        out,
                        r#"        <tr>
          <td>{seq}</td>
          <td>{proto}</td>
          <td>{payload}</td>
          <td class="{thr_cls}">{thr}</td>
          <td>{goodput}</td>
          <td>{ttfb:.2}</td>
          <td>{total:.2}</td>
          <td>{cpu}</td>
          <td>{client_csw}</td>
          <td>{server_csw}</td>
          <td>{status}</td>
        </tr>
"#,
                        seq = a.sequence_num,
                        proto = a.protocol,
                        payload = format_bytes(h.payload_bytes),
                        thr_cls = if h.throughput_mbps.is_some() {
                            "ok"
                        } else {
                            "warn"
                        },
                        thr = throughput,
                        goodput = goodput,
                        ttfb = h.ttfb_ms,
                        total = h.total_duration_ms,
                        cpu = cpu,
                        client_csw = client_csw,
                        server_csw = server_csw,
                        status = status_cell,
                    );
                }
                let _ = writeln!(out, "      </tbody>\n    </table>\n  </details>");
            }
            let _ = writeln!(out, "</section>");
        }
    }

    // ── UDP Throughput results (collapsible, grouped by proto+payload) ───────
    {
        use std::collections::BTreeSet;
        let udp_tp_rows: Vec<&RequestAttempt> = run
            .attempts
            .iter()
            .filter(|a| {
                matches!(a.protocol, Protocol::UdpDownload | Protocol::UdpUpload)
                    && a.udp_throughput.is_some()
            })
            .collect();
        if !udp_tp_rows.is_empty() {
            let _ = writeln!(
                out,
                "\n<section class=\"card\">\n  <h2>UDP Throughput Results</h2>"
            );

            let groups: Vec<(Protocol, usize)> = {
                let mut seen = BTreeSet::new();
                [Protocol::UdpDownload, Protocol::UdpUpload]
                    .iter()
                    .flat_map(|proto| {
                        let mut payloads: Vec<usize> = udp_tp_rows
                            .iter()
                            .filter(|a| &a.protocol == proto)
                            .filter_map(|a| a.udp_throughput.as_ref().map(|u| u.payload_bytes))
                            .filter(|&b| b > 0)
                            .collect::<BTreeSet<_>>()
                            .into_iter()
                            .collect();
                        payloads.retain(|p| seen.insert((proto.to_string(), *p)));
                        payloads
                            .into_iter()
                            .map(move |p| (proto.clone(), p))
                            .collect::<Vec<_>>()
                    })
                    .collect()
            };

            let single_small = groups.len() == 1
                && udp_tp_rows
                    .iter()
                    .filter(|a| {
                        groups
                            .first()
                            .map(|(p, b)| {
                                &a.protocol == p
                                    && a.udp_throughput.as_ref().map(|u| u.payload_bytes)
                                        == Some(*b)
                            })
                            .unwrap_or(false)
                    })
                    .count()
                    <= 20;

            for (proto, payload_bytes) in &groups {
                let group_rows: Vec<&&RequestAttempt> = udp_tp_rows
                    .iter()
                    .filter(|a| {
                        &a.protocol == proto
                            && a.udp_throughput.as_ref().map(|u| u.payload_bytes)
                                == Some(*payload_bytes)
                    })
                    .collect();
                let n = group_rows.len();
                let mbps_vals: Vec<f64> = group_rows
                    .iter()
                    .filter_map(|a| a.udp_throughput.as_ref().and_then(|u| u.throughput_mbps))
                    .collect();
                let avg_loss: f64 = group_rows
                    .iter()
                    .filter_map(|a| a.udp_throughput.as_ref().map(|u| u.loss_percent))
                    .sum::<f64>()
                    / n.max(1) as f64;
                let stats = compute_stats(&mbps_vals);
                let summary_meta = if let Some(ref s) = stats {
                    format!(
                        "{n} runs · avg {avg:.1} MB/s · ±{sd:.1} · loss {loss:.1}%",
                        avg = s.mean,
                        sd = s.stddev,
                        loss = avg_loss,
                    )
                } else {
                    format!("{n} runs · loss {avg_loss:.1}%")
                };
                let grp_label = format!("{proto} {}", format_bytes(*payload_bytes));
                let open_attr = if single_small { " open" } else { "" };
                let _ = write!(
                    out,
                    r#"  <details{open}>
    <summary><span class="grp-lbl">{lbl}</span><span class="grp-meta">{meta}</span></summary>
    <table>
      <thead>
        <tr><th>Run #</th><th>Mode</th><th>Payload</th><th>Sent</th><th>Recv</th>
            <th>Loss %</th><th>Throughput (MB/s)</th><th>Transfer (ms)</th><th>Bytes Acked</th></tr>
      </thead>
      <tbody>
"#,
                    open = open_attr,
                    lbl = escape_html(&grp_label),
                    meta = escape_html(&summary_meta),
                );
                for a in &group_rows {
                    let u = a.udp_throughput.as_ref().unwrap();
                    let throughput = u
                        .throughput_mbps
                        .map(|m| format!("{m:.2}"))
                        .unwrap_or_else(|| "—".into());
                    let bytes_acked = u
                        .bytes_acked
                        .map(format_bytes)
                        .unwrap_or_else(|| "—".into());
                    let _ = write!(
                        out,
                        r#"        <tr>
          <td>{seq}</td>
          <td>{proto}</td>
          <td>{payload}</td>
          <td>{sent}</td>
          <td>{recv}</td>
          <td class="{loss_cls}">{loss:.1}%</td>
          <td class="{thr_cls}">{thr}</td>
          <td>{xfer:.2}</td>
          <td>{acked}</td>
        </tr>
"#,
                        seq = a.sequence_num,
                        proto = a.protocol,
                        payload = format_bytes(u.payload_bytes),
                        sent = u.datagrams_sent,
                        recv = u.datagrams_received,
                        loss = u.loss_percent,
                        loss_cls = if u.loss_percent > 5.0 {
                            "warn"
                        } else if u.loss_percent == 0.0 {
                            "ok"
                        } else {
                            ""
                        },
                        thr_cls = if u.throughput_mbps.is_some() {
                            "ok"
                        } else {
                            "warn"
                        },
                        thr = throughput,
                        xfer = u.transfer_ms,
                        acked = bytes_acked,
                    );
                }
                let _ = writeln!(out, "      </tbody>\n    </table>\n  </details>");
            }
            let _ = writeln!(out, "</section>");
        }
    }

    // ── Individual attempts ───────────────────────────────────────────────────
    {
        let total_attempts = run.attempts.len();
        let succeeded = run.success_count();
        let failed = run.failure_count();
        let open_attr = if total_attempts <= 20 { " open" } else { "" };
        let summary_meta = format!("{succeeded} succeeded · {failed} failed");
        let _ = write!(
            out,
            r#"
<section class="card">
  <h2>All Attempts</h2>
  <details{open}>
    <summary>
      <span class="grp-lbl">{n} attempts</span>
      <span class="grp-meta">{meta}</span>
    </summary>
    <table>
      <thead>
        <tr>
          <th>#</th><th>Protocol</th><th>Status</th>
          <th>DNS (ms)</th><th>TCP (ms)</th><th>TLS (ms)</th>
          <th>TTFB (ms)</th><th>Total (ms)</th>
          <th>HTTP ver / UDP stats</th><th>Error</th>
        </tr>
      </thead>
      <tbody>
"#,
            open = open_attr,
            n = total_attempts,
            meta = escape_html(&summary_meta),
        );

        for a in &run.attempts {
            append_attempt_row(out, a);
        }
        let _ = writeln!(
            out,
            "      </tbody>\n    </table>\n  </details>\n</section>"
        );
    }

    // ── TCP kernel stats ─────────────────────────────────────────────────────
    let tcp_rows: Vec<&RequestAttempt> = run.attempts.iter().filter(|a| a.tcp.is_some()).collect();
    if !tcp_rows.is_empty() {
        let open_attr = if tcp_rows.len() <= 20 { " open" } else { "" };
        let _ = write!(
            out,
            r#"
<section class="card">
  <h2>TCP Stats</h2>
  <details{open}>
    <summary><span class="grp-lbl">{n} connections</span></summary>
    <table>
      <thead>
        <tr>
          <th>#</th><th>Protocol</th><th>Local → Remote</th>
          <th>Connect (ms)</th><th>MSS (B)</th>
          <th>RTT (ms)</th><th>RTT Var (ms)</th><th>Min RTT (ms)</th>
          <th>Cwnd (seg)</th><th>Ssthresh</th>
          <th>Retrans</th><th>Total Retrans</th>
          <th>Rcv Win (B)</th><th>Segs Out</th><th>Segs In</th>
          <th>Delivery (MB/s)</th><th>Congestion</th>
        </tr>
      </thead>
      <tbody>
"#,
            open = open_attr,
            n = tcp_rows.len(),
        );
        for a in &tcp_rows {
            let t = a.tcp.as_ref().unwrap();
            let local_remote = format!(
                "{} → {}",
                t.local_addr.as_deref().unwrap_or("?"),
                t.remote_addr
            );
            let delivery_mbps = t
                .delivery_rate_bps
                .map(|b| format!("{:.1}", b as f64 / 1_000_000.0))
                .unwrap_or_else(|| "—".into());
            let _ = write!(
                out,
                r#"        <tr>
          <td>{seq}</td>
          <td>{proto}</td>
          <td><code>{addrs}</code></td>
          <td>{conn:.3}</td>
          <td>{mss}</td>
          <td>{rtt}</td>
          <td>{rttvar}</td>
          <td>{minrtt}</td>
          <td>{cwnd}</td>
          <td>{ssthresh}</td>
          <td>{retrans}</td>
          <td>{total_retrans}</td>
          <td>{rcvwin}</td>
          <td>{segsout}</td>
          <td>{segsin}</td>
          <td>{delivery}</td>
          <td>{cong}</td>
        </tr>
"#,
                seq = a.sequence_num,
                proto = a.protocol,
                addrs = local_remote,
                conn = t.connect_duration_ms,
                mss = t
                    .mss_bytes
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into()),
                rtt = t
                    .rtt_estimate_ms
                    .map(|v| format!("{v:.3}"))
                    .unwrap_or_else(|| "—".into()),
                rttvar = t
                    .rtt_variance_ms
                    .map(|v| format!("{v:.3}"))
                    .unwrap_or_else(|| "—".into()),
                minrtt = t
                    .min_rtt_ms
                    .map(|v| format!("{v:.3}"))
                    .unwrap_or_else(|| "—".into()),
                cwnd = t
                    .snd_cwnd
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into()),
                ssthresh = t
                    .snd_ssthresh
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "∞".into()),
                retrans = t
                    .retransmits
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "0".into()),
                total_retrans = t
                    .total_retrans
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "0".into()),
                rcvwin = t
                    .rcv_space
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into()),
                segsout = t
                    .segs_out
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into()),
                segsin = t
                    .segs_in
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into()),
                delivery = delivery_mbps,
                cong = t.congestion_algorithm.as_deref().unwrap_or("—"),
            );
        }
        let _ = writeln!(
            out,
            "      </tbody>\n    </table>\n  </details>\n</section>"
        );
    }

    // ── TLS info ─────────────────────────────────────────────────────────────
    let tls_rows: Vec<&RequestAttempt> = run.attempts.iter().filter(|a| a.tls.is_some()).collect();
    if !tls_rows.is_empty() {
        let _ = write!(
            out,
            r#"
<section class="card">
  <h2>TLS Details</h2>
  <table>
    <thead>
      <tr><th>#</th><th>Version</th><th>Cipher</th><th>ALPN</th>
          <th>Cert Subject</th><th>Cert Expiry</th><th>Handshake (ms)</th></tr>
    </thead>
    <tbody>
"#
        );
        for a in &tls_rows {
            let t = a.tls.as_ref().unwrap();
            let _ = write!(
                out,
                r#"      <tr>
        <td>{seq}</td>
        <td>{ver}</td>
        <td><code>{cipher}</code></td>
        <td>{alpn}</td>
        <td>{subj}</td>
        <td>{expiry}</td>
        <td>{hs:.2}</td>
      </tr>
"#,
                seq = a.sequence_num,
                ver = escape_html(&t.protocol_version),
                cipher = escape_html(&t.cipher_suite),
                alpn = t.alpn_negotiated.as_deref().unwrap_or("—"),
                subj = t
                    .cert_subject
                    .as_deref()
                    .map(escape_html)
                    .unwrap_or_else(|| "—".into()),
                expiry = t
                    .cert_expiry
                    .map(|e| e.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "—".into()),
                hs = t.handshake_duration_ms,
            );
        }
        let _ = writeln!(out, "    </tbody>\n  </table>\n</section>");
    }

    // ── Errors ────────────────────────────────────────────────────────────────
    let error_rows: Vec<&RequestAttempt> =
        run.attempts.iter().filter(|a| a.error.is_some()).collect();
    if !error_rows.is_empty() {
        let _ = writeln!(
            out,
            r#"<section class="card error-section"><h2>Errors</h2><table>
    <thead><tr><th>#</th><th>Protocol</th><th>Category</th><th>Message</th><th>Detail</th></tr></thead>
    <tbody>"#
        );
        for a in &error_rows {
            let e = a.error.as_ref().unwrap();
            let _ = write!(
                out,
                r#"      <tr>
        <td>{seq}</td>
        <td>{proto}</td>
        <td class="err">{cat}</td>
        <td>{msg}</td>
        <td>{detail}</td>
      </tr>
"#,
                seq = a.sequence_num,
                proto = a.protocol,
                cat = e.category,
                msg = escape_html(&e.message),
                detail = e
                    .detail
                    .as_deref()
                    .map(escape_html)
                    .unwrap_or_else(|| "—".into()),
            );
        }
        let _ = writeln!(out, "    </tbody>\n  </table>\n</section>");
    }
} // end write_run_sections

// ─────────────────────────────────────────────────────────────────────────────
// Row renderers
// ─────────────────────────────────────────────────────────────────────────────

fn append_proto_row(out: &mut String, proto: &Protocol, rows: &[&RequestAttempt]) {
    let successes = rows.iter().filter(|a| a.success).count();

    let avg = |f: fn(&RequestAttempt) -> Option<f64>| -> String {
        let vals: Vec<f64> = rows.iter().filter_map(|a| f(a)).collect();
        if vals.is_empty() {
            "—".into()
        } else {
            format!("{:.2}", vals.iter().sum::<f64>() / vals.len() as f64)
        }
    };

    let dns_avg = avg(|a| a.dns.as_ref().map(|d| d.duration_ms));
    let tcp_avg = avg(|a| a.tcp.as_ref().map(|t| t.connect_duration_ms));
    let tls_avg = avg(|a| a.tls.as_ref().map(|t| t.handshake_duration_ms));
    let ttfb_avg = avg(|a| a.http.as_ref().map(|h| h.ttfb_ms));
    let total_avg = avg(|a| {
        a.http
            .as_ref()
            .map(|h| h.total_duration_ms)
            .or_else(|| a.udp.as_ref().map(|u| u.rtt_avg_ms))
    });

    let ok_cls = if successes == rows.len() {
        "ok"
    } else {
        "warn"
    };

    let _ = write!(
        out,
        r#"      <tr>
        <td><strong>{proto}</strong></td>
        <td>{n}</td>
        <td>{dns}</td>
        <td>{tcp}</td>
        <td>{tls}</td>
        <td>{ttfb}</td>
        <td>{total}</td>
        <td class="{ok_cls}">{suc}/{n}</td>
      </tr>
"#,
        proto = proto,
        n = rows.len(),
        dns = dns_avg,
        tcp = tcp_avg,
        tls = tls_avg,
        ttfb = ttfb_avg,
        total = total_avg,
        suc = successes,
        ok_cls = ok_cls,
    );
}

fn append_attempt_row(out: &mut String, a: &RequestAttempt) {
    let dns_ms = a
        .dns
        .as_ref()
        .map(|d| format!("{:.2}", d.duration_ms))
        .unwrap_or_else(|| "—".into());
    let tcp_ms = a
        .tcp
        .as_ref()
        .map(|t| format!("{:.2}", t.connect_duration_ms))
        .unwrap_or_else(|| "—".into());
    let tls_ms = a
        .tls
        .as_ref()
        .map(|t| format!("{:.2}", t.handshake_duration_ms))
        .unwrap_or_else(|| "—".into());
    let (ttfb_ms, total_ms, version) = if let Some(h) = &a.http {
        let ver = match &a.protocol {
            Protocol::Download | Protocol::Upload | Protocol::WebDownload | Protocol::WebUpload => {
                if let Some(mbps) = h.throughput_mbps {
                    format!("{:.2} MB/s ({})", mbps, format_bytes(h.payload_bytes))
                } else {
                    h.negotiated_version.clone()
                }
            }
            _ => h.negotiated_version.clone(),
        };
        (
            format!("{:.2}", h.ttfb_ms),
            format!("{:.2}", h.total_duration_ms),
            ver,
        )
    } else if let Some(ut) = &a.udp_throughput {
        let thr = ut
            .throughput_mbps
            .map(|m| format!("{m:.2} MB/s ({})", format_bytes(ut.payload_bytes)))
            .unwrap_or_else(|| format!("loss={:.1}%", ut.loss_percent));
        ("—".into(), format!("{:.2}", ut.transfer_ms), thr)
    } else if let Some(u) = &a.udp {
        (
            "—".into(),
            format!("{:.2}", u.rtt_avg_ms),
            format!("loss={:.1}%", u.loss_percent),
        )
    } else {
        ("—".into(), "—".into(), "—".into())
    };

    let status_cell = if let Some(h) = &a.http {
        let cls = if h.status_code < 400 { "ok" } else { "err" };
        format!(r#"<span class="{cls}">{}</span>"#, h.status_code)
    } else if a.success {
        r#"<span class="ok">OK</span>"#.into()
    } else {
        r#"<span class="err">FAIL</span>"#.into()
    };

    let err_cell = a
        .error
        .as_ref()
        .map(|e| {
            format!(
                r#"<span class="err" title="{}">{}</span>"#,
                escape_html(e.detail.as_deref().unwrap_or("")),
                escape_html(&e.message)
            )
        })
        .unwrap_or_else(|| "—".into());

    let _ = write!(
        out,
        r#"      <tr class="{row_cls}">
        <td>{seq}</td><td>{proto}</td><td>{status}</td>
        <td>{dns}</td><td>{tcp}</td><td>{tls}</td>
        <td>{ttfb}</td><td>{total}</td>
        <td>{ver}</td><td>{err}</td>
      </tr>
"#,
        row_cls = if a.success { "" } else { "row-err" },
        seq = a.sequence_num,
        proto = a.protocol,
        status = status_cell,
        dns = dns_ms,
        tcp = tcp_ms,
        tls = tls_ms,
        ttfb = ttfb_ms,
        total = total_ms,
        ver = escape_html(&version),
        err = err_cell,
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Formatting helpers
// ─────────────────────────────────────────────────────────────────────────────

fn format_bytes(n: usize) -> String {
    if n >= 1 << 30 {
        format!("{:.1} GiB", n as f64 / (1u64 << 30) as f64)
    } else if n >= 1 << 20 {
        format!("{:.1} MiB", n as f64 / (1u64 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{:.1} KiB", n as f64 / (1u64 << 10) as f64)
    } else {
        format!("{n} B")
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HTML escaping
// ─────────────────────────────────────────────────────────────────────────────

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Render a self-contained box-and-whisker SVG chart (horizontal layout).
///
/// `groups`: `(label, values, fill_color)` — values need at least 4 points.
/// Draws: p5 whisker ← Q1 box → median line → Q3 box → p95 whisker.
/// `unit`: appended to the per-row annotation label.
fn svg_boxplot(title: &str, groups: &[(&str, &[f64], &str)], unit: &str) -> String {
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
        .filter(|(_, v, _)| v.len() >= 4)
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
fn svg_cdf(title: &str, series: &[(&str, &[f64], &str)], unit: &str) -> String {
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
fn svg_hbar(title: &str, bars: &[(&str, f64)], unit: &str, colors: &[&str]) -> String {
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
"#;

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{
        ErrorCategory, ErrorRecord, HttpResult, Protocol, RequestAttempt, TcpResult, TestRun,
        UdpResult, UdpThroughputResult,
    };
    use chrono::Utc;
    use uuid::Uuid;

    fn make_run() -> TestRun {
        let run_id = Uuid::new_v4();
        TestRun {
            run_id,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            target_url: "http://localhost/health".into(),
            target_host: "localhost".into(),
            modes: vec!["http1".into()],
            total_runs: 1,
            concurrency: 1,
            timeout_ms: 5000,
            client_os: "test".into(),
            client_version: "0.1.0".into(),
            server_info: None,
            client_info: None,
            baseline: None,
            attempts: vec![RequestAttempt {
                attempt_id: Uuid::new_v4(),
                run_id,
                protocol: Protocol::Http1,
                sequence_num: 0,
                started_at: Utc::now(),
                finished_at: Some(Utc::now()),
                success: true,
                dns: None,
                tcp: Some(TcpResult {
                    local_addr: Some("127.0.0.1:12345".into()),
                    remote_addr: "127.0.0.1:80".into(),
                    connect_duration_ms: 1.5,
                    attempt_count: 1,
                    started_at: Utc::now(),
                    success: true,
                    mss_bytes: None,
                    rtt_estimate_ms: None,
                    retransmits: None,
                    total_retrans: None,
                    snd_cwnd: None,
                    snd_ssthresh: None,
                    rtt_variance_ms: None,
                    rcv_space: None,
                    segs_out: None,
                    segs_in: None,
                    congestion_algorithm: None,
                    delivery_rate_bps: None,
                    min_rtt_ms: None,
                }),
                tls: None,
                http: Some(HttpResult {
                    negotiated_version: "HTTP/1.1".into(),
                    status_code: 200,
                    headers_size_bytes: 120,
                    body_size_bytes: 42,
                    ttfb_ms: 5.0,
                    total_duration_ms: 10.0,
                    redirect_count: 0,
                    started_at: Utc::now(),
                    response_headers: vec![],
                    payload_bytes: 0,
                    throughput_mbps: None,
                    goodput_mbps: None,
                    cpu_time_ms: None,
                    csw_voluntary: None,
                    csw_involuntary: None,
                }),
                udp: None,
                error: None,
                retry_count: 0,
                server_timing: None,
                udp_throughput: None,
                page_load: None,
                browser: None,
            }],
        }
    }

    #[test]
    fn html_contains_target() {
        let run = make_run();
        let html = render(&run, None);
        assert!(html.contains("localhost/health"));
    }

    #[test]
    fn html_contains_http11() {
        let run = make_run();
        let html = render(&run, None);
        assert!(html.contains("HTTP/1.1"));
    }

    #[test]
    fn html_escapes_special_chars() {
        assert_eq!(
            escape_html("<script>alert(1)</script>"),
            "&lt;script&gt;alert(1)&lt;/script&gt;"
        );
    }

    #[test]
    fn escape_html_ampersand_and_quotes() {
        assert_eq!(escape_html("a&b"), "a&amp;b");
        assert_eq!(escape_html("\"quoted\""), "&quot;quoted&quot;");
        assert_eq!(escape_html("it's"), "it&#39;s");
    }

    #[test]
    fn escape_html_empty_string() {
        assert_eq!(escape_html(""), "");
    }

    #[test]
    fn escape_html_ampersand_escaped_first_to_avoid_double_escaping() {
        // "&lt;" should become "&amp;lt;" not "&lt;" again
        assert_eq!(escape_html("&lt;"), "&amp;lt;");
    }

    // ── format_bytes ─────────────────────────────────────────────────────────

    #[test]
    fn format_bytes_zero() {
        assert_eq!(format_bytes(0), "0 B");
    }

    #[test]
    fn format_bytes_one() {
        assert_eq!(format_bytes(1), "1 B");
    }

    #[test]
    fn format_bytes_just_below_kib() {
        assert_eq!(format_bytes(1023), "1023 B");
    }

    #[test]
    fn format_bytes_exactly_kib() {
        assert_eq!(format_bytes(1024), "1.0 KiB");
    }

    #[test]
    fn format_bytes_just_below_mib() {
        assert_eq!(format_bytes(1024 * 1024 - 1), "1024.0 KiB");
    }

    #[test]
    fn format_bytes_exactly_mib() {
        assert_eq!(format_bytes(1024 * 1024), "1.0 MiB");
    }

    #[test]
    fn format_bytes_just_below_gib() {
        assert_eq!(format_bytes(1024 * 1024 * 1024 - 1), "1024.0 MiB");
    }

    #[test]
    fn format_bytes_exactly_gib() {
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GiB");
    }

    #[test]
    fn format_bytes_multiple_gibs() {
        assert_eq!(format_bytes(4 * 1024 * 1024 * 1024), "4.0 GiB");
    }

    #[test]
    fn save_writes_html_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let run = make_run();
        save(&run, tmp.path(), None).unwrap();
        let content = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(content.starts_with("<!DOCTYPE html>"));
    }

    #[test]
    fn html_includes_css_link_when_href_provided() {
        let run = make_run();
        let html = render(&run, Some("report.css"));
        assert!(html.contains(r#"<link rel="stylesheet""#));
        assert!(html.contains("report.css"));
    }

    #[test]
    fn html_no_css_link_without_href() {
        let run = make_run();
        let html = render(&run, None);
        // Without an external CSS href, should embed inline styles but no <link>
        assert!(html.contains("<style>"));
    }

    #[test]
    fn html_contains_error_section_for_failed_attempt() {
        use crate::metrics::{ErrorCategory, ErrorRecord};

        let run_id = Uuid::new_v4();
        let run = TestRun {
            run_id,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            target_url: "http://localhost/health".into(),
            target_host: "localhost".into(),
            modes: vec!["http1".into()],
            total_runs: 1,
            concurrency: 1,
            timeout_ms: 5000,
            client_os: "test".into(),
            client_version: "0.1.0".into(),
            server_info: None,
            client_info: None,
            baseline: None,
            attempts: vec![RequestAttempt {
                attempt_id: Uuid::new_v4(),
                run_id,
                protocol: Protocol::Http1,
                sequence_num: 0,
                started_at: Utc::now(),
                finished_at: Some(Utc::now()),
                success: false,
                dns: None,
                tcp: None,
                tls: None,
                http: None,
                udp: None,
                error: Some(ErrorRecord {
                    category: ErrorCategory::Tcp,
                    message: "Connection refused".into(),
                    detail: Some("os error 111".into()),
                    occurred_at: Utc::now(),
                }),
                retry_count: 0,
                server_timing: None,
                udp_throughput: None,
                page_load: None,
                browser: None,
            }],
        };
        let html = render(&run, None);
        assert!(html.contains("Errors"), "should have an Errors section");
        assert!(html.contains("Connection refused"));
    }

    #[test]
    fn html_contains_throughput_section_for_download_attempt() {
        let run_id = Uuid::new_v4();
        let now = Utc::now();
        let run = TestRun {
            run_id,
            started_at: now,
            finished_at: Some(now),
            target_url: "http://localhost/health".into(),
            target_host: "localhost".into(),
            modes: vec!["download".into()],
            total_runs: 1,
            concurrency: 1,
            timeout_ms: 5000,
            client_os: "test".into(),
            client_version: "0.1.0".into(),
            server_info: None,
            client_info: None,
            baseline: None,
            attempts: vec![RequestAttempt {
                attempt_id: Uuid::new_v4(),
                run_id,
                protocol: Protocol::Download,
                sequence_num: 0,
                started_at: now,
                finished_at: Some(now),
                success: true,
                dns: None,
                tcp: None,
                tls: None,
                http: Some(HttpResult {
                    negotiated_version: "HTTP/1.1".into(),
                    status_code: 200,
                    headers_size_bytes: 0,
                    body_size_bytes: 1_048_576,
                    ttfb_ms: 5.0,
                    total_duration_ms: 95.0,
                    redirect_count: 0,
                    started_at: now,
                    response_headers: vec![],
                    payload_bytes: 1_048_576,
                    throughput_mbps: Some(105.5),
                    goodput_mbps: Some(98.0),
                    cpu_time_ms: Some(12.0),
                    csw_voluntary: None,
                    csw_involuntary: None,
                }),
                udp: None,
                error: None,
                retry_count: 0,
                server_timing: None,
                udp_throughput: None,
                page_load: None,
                browser: None,
            }],
        };
        let html = render(&run, None);
        assert!(
            html.contains("Throughput Results"),
            "should have a Throughput Results section"
        );
        assert!(html.contains("105"), "should show throughput value");
    }

    #[test]
    fn html_contains_tls_section_for_tls_attempt() {
        let run_id = Uuid::new_v4();
        let now = Utc::now();
        let run = TestRun {
            run_id,
            started_at: now,
            finished_at: Some(now),
            target_url: "https://localhost/health".into(),
            target_host: "localhost".into(),
            modes: vec!["tls".into()],
            total_runs: 1,
            concurrency: 1,
            timeout_ms: 5000,
            client_os: "test".into(),
            client_version: "0.1.0".into(),
            server_info: None,
            client_info: None,
            baseline: None,
            attempts: vec![RequestAttempt {
                attempt_id: Uuid::new_v4(),
                run_id,
                protocol: crate::metrics::Protocol::Tls,
                sequence_num: 0,
                started_at: now,
                finished_at: Some(now),
                success: true,
                dns: None,
                tcp: None,
                tls: Some(crate::metrics::TlsResult {
                    protocol_version: "TLSv1.3".into(),
                    cipher_suite: "TLS_AES_256_GCM_SHA384".into(),
                    alpn_negotiated: Some("h2".into()),
                    cert_subject: Some("CN=localhost".into()),
                    cert_issuer: Some("CN=Test CA".into()),
                    cert_expiry: Some(now),
                    handshake_duration_ms: 7.5,
                    started_at: now,
                    success: true,
                    cert_chain: vec![],
                    tls_backend: Some("rustls".into()),
                }),
                http: None,
                udp: None,
                error: None,
                retry_count: 0,
                server_timing: None,
                udp_throughput: None,
                page_load: None,
                browser: None,
            }],
        };
        let html = render(&run, None);
        assert!(
            html.contains("TLS Details"),
            "should have TLS Details section"
        );
        assert!(html.contains("TLSv1.3"));
    }

    #[test]
    fn html_contains_page_load_section() {
        let run_id = Uuid::new_v4();
        let now = Utc::now();
        let run = TestRun {
            run_id,
            started_at: now,
            finished_at: Some(now),
            target_url: "https://localhost/health".into(),
            target_host: "localhost".into(),
            modes: vec!["pageload".into()],
            total_runs: 1,
            concurrency: 1,
            timeout_ms: 5000,
            client_os: "test".into(),
            client_version: "0.1.0".into(),
            server_info: None,
            client_info: None,
            baseline: None,
            attempts: vec![RequestAttempt {
                attempt_id: Uuid::new_v4(),
                run_id,
                protocol: crate::metrics::Protocol::PageLoad,
                sequence_num: 0,
                started_at: now,
                finished_at: Some(now),
                success: true,
                dns: None,
                tcp: None,
                tls: None,
                http: None,
                udp: None,
                error: None,
                retry_count: 0,
                server_timing: None,
                udp_throughput: None,
                page_load: Some(crate::metrics::PageLoadResult {
                    asset_count: 20,
                    assets_fetched: 20,
                    total_bytes: 204_800,
                    total_ms: 120.5,
                    ttfb_ms: 5.2,
                    connections_opened: 6,
                    asset_timings_ms: vec![10.0; 20],
                    started_at: now,
                    tls_setup_ms: 24.0,
                    tls_overhead_ratio: 0.19,
                    per_connection_tls_ms: vec![4.0; 6],
                    cpu_time_ms: Some(8.3),
                }),
                browser: None,
            }],
        };
        let html = render(&run, None);
        // Page load data should appear in the Protocol Comparison section
        assert!(
            html.contains("pageload") || html.contains("PageLoad") || html.contains("Page Load"),
            "should reference page load mode"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // append_proto_row — protocol summary row rendering
    // ─────────────────────────────────────────────────────────────────────────

    fn make_http_attempt(success: bool, ttfb: f64, total: f64) -> RequestAttempt {
        RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id: Uuid::new_v4(),
            protocol: Protocol::Http1,
            sequence_num: 0,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            success,
            dns: None,
            tcp: None,
            tls: None,
            http: Some(HttpResult {
                negotiated_version: "HTTP/1.1".into(),
                status_code: if success { 200 } else { 500 },
                headers_size_bytes: 100,
                body_size_bytes: 42,
                ttfb_ms: ttfb,
                total_duration_ms: total,
                redirect_count: 0,
                started_at: Utc::now(),
                response_headers: vec![],
                payload_bytes: 0,
                throughput_mbps: None,
                goodput_mbps: None,
                cpu_time_ms: None,
                csw_voluntary: None,
                csw_involuntary: None,
            }),
            udp: None,
            error: None,
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
        }
    }

    #[test]
    fn append_proto_row_all_success_uses_ok_class() {
        let a1 = make_http_attempt(true, 5.0, 10.0);
        let a2 = make_http_attempt(true, 7.0, 14.0);
        let rows: Vec<&RequestAttempt> = vec![&a1, &a2];
        let mut out = String::new();
        append_proto_row(&mut out, &Protocol::Http1, &rows);
        assert!(
            out.contains(r#"class="ok""#),
            "all-success rows should use 'ok' class"
        );
        assert!(out.contains("2/2"), "should show 2/2 successes");
    }

    #[test]
    fn append_proto_row_partial_success_uses_warn_class() {
        let ok = make_http_attempt(true, 5.0, 10.0);
        let fail = make_http_attempt(false, 0.0, 0.0);
        let rows: Vec<&RequestAttempt> = vec![&ok, &fail];
        let mut out = String::new();
        append_proto_row(&mut out, &Protocol::Http1, &rows);
        assert!(
            out.contains(r#"class="warn""#),
            "partial failures should use 'warn' class"
        );
        assert!(out.contains("1/2"), "should show 1/2 successes");
    }

    #[test]
    fn append_proto_row_averages_ttfb_correctly() {
        let a1 = make_http_attempt(true, 10.0, 20.0);
        let a2 = make_http_attempt(true, 20.0, 40.0);
        let rows: Vec<&RequestAttempt> = vec![&a1, &a2];
        let mut out = String::new();
        append_proto_row(&mut out, &Protocol::Http1, &rows);
        // avg TTFB = (10 + 20) / 2 = 15.00
        assert!(out.contains("15.00"), "average TTFB should be 15.00");
        // avg total = (20 + 40) / 2 = 30.00
        assert!(out.contains("30.00"), "average total should be 30.00");
    }

    #[test]
    fn append_proto_row_no_http_shows_dashes() {
        let a = RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id: Uuid::new_v4(),
            protocol: Protocol::Tcp,
            sequence_num: 0,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            success: true,
            dns: None,
            tcp: None,
            tls: None,
            http: None,
            udp: None,
            error: None,
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
        };
        let rows: Vec<&RequestAttempt> = vec![&a];
        let mut out = String::new();
        append_proto_row(&mut out, &Protocol::Tcp, &rows);
        assert!(out.contains("—"), "no timing data should show em dash");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // append_attempt_row — individual attempt row rendering
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn append_attempt_row_success_http_shows_status_code() {
        let a = make_http_attempt(true, 5.0, 10.0);
        let mut out = String::new();
        append_attempt_row(&mut out, &a);
        assert!(out.contains("200"), "should show HTTP status 200");
        assert!(
            out.contains(r#"class="ok""#),
            "status cell should use 'ok' class"
        );
        // No error → em dash in error column
        assert!(out.contains("—"), "no error should show em dash");
    }

    #[test]
    fn append_attempt_row_failed_shows_err_class() {
        let mut a = make_http_attempt(false, 0.0, 0.0);
        a.error = Some(ErrorRecord {
            category: ErrorCategory::Tcp,
            message: "connection refused".to_string(),
            detail: Some("detail info".to_string()),
            occurred_at: Utc::now(),
        });
        let mut out = String::new();
        append_attempt_row(&mut out, &a);
        assert!(
            out.contains("row-err"),
            "failed attempt should have row-err class"
        );
        assert!(
            out.contains("connection refused"),
            "error message should appear"
        );
        assert!(
            out.contains("detail info"),
            "detail should appear in title attr"
        );
    }

    #[test]
    fn append_attempt_row_udp_echo_shows_rtt() {
        let a = RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id: Uuid::new_v4(),
            protocol: Protocol::Udp,
            sequence_num: 1,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            success: true,
            dns: None,
            tcp: None,
            tls: None,
            http: None,
            udp: Some(UdpResult {
                remote_addr: "127.0.0.1:9000".into(),
                probe_count: 5,
                success_count: 5,
                loss_percent: 0.0,
                rtt_min_ms: 1.0,
                rtt_avg_ms: 2.5,
                rtt_p95_ms: 3.0,
                jitter_ms: 0.5,
                started_at: Utc::now(),
                probe_rtts_ms: vec![Some(2.5); 5],
            }),
            error: None,
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
        };
        let mut out = String::new();
        append_attempt_row(&mut out, &a);
        assert!(out.contains("2.50"), "rtt_avg_ms should appear");
        assert!(out.contains("loss=0.0%"), "loss percent should appear");
    }

    #[test]
    fn append_attempt_row_udp_throughput_shows_transfer_ms() {
        let a = RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id: Uuid::new_v4(),
            protocol: Protocol::UdpDownload,
            sequence_num: 2,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            success: true,
            dns: None,
            tcp: None,
            tls: None,
            http: None,
            udp: None,
            error: None,
            retry_count: 0,
            server_timing: None,
            udp_throughput: Some(UdpThroughputResult {
                remote_addr: "127.0.0.1:9998".into(),
                payload_bytes: 65_536,
                datagrams_sent: 50,
                datagrams_received: 50,
                bytes_acked: None,
                loss_percent: 0.0,
                transfer_ms: 125.0,
                throughput_mbps: Some(4.5),
                started_at: Utc::now(),
            }),
            page_load: None,
            browser: None,
        };
        let mut out = String::new();
        append_attempt_row(&mut out, &a);
        assert!(out.contains("125.00"), "transfer_ms should appear");
        assert!(out.contains("4.50 MB/s"), "throughput should appear");
    }

    #[test]
    fn append_attempt_row_no_results_shows_dashes() {
        let a = RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id: Uuid::new_v4(),
            protocol: Protocol::Tcp,
            sequence_num: 0,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            success: true,
            dns: None,
            tcp: None,
            tls: None,
            http: None,
            udp: None,
            error: None,
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
        };
        let mut out = String::new();
        append_attempt_row(&mut out, &a);
        let dash_count = out.matches("—").count();
        assert!(
            dash_count >= 4,
            "no-data attempt should have multiple em dashes, got {dash_count}"
        );
        assert!(
            out.contains(r#"class="ok">OK<"#),
            "success with no HTTP should show OK"
        );
    }

    #[test]
    fn append_attempt_row_http_throughput_shows_mbps() {
        let mut a = make_http_attempt(true, 5.0, 100.0);
        a.protocol = Protocol::Download;
        if let Some(ref mut h) = a.http {
            h.throughput_mbps = Some(12.34);
            h.payload_bytes = 1_048_576; // 1 MiB
        }
        let mut out = String::new();
        append_attempt_row(&mut out, &a);
        assert!(out.contains("12.34 MB/s"), "should show throughput");
        assert!(out.contains("1.0 MiB"), "should show payload size");
    }

    #[test]
    fn html_contains_browser_section() {
        let run_id = Uuid::new_v4();
        let now = Utc::now();
        let run = TestRun {
            run_id,
            started_at: now,
            finished_at: Some(now),
            target_url: "https://localhost/health".into(),
            target_host: "localhost".into(),
            modes: vec!["browser".into()],
            total_runs: 1,
            concurrency: 1,
            timeout_ms: 5000,
            client_os: "test".into(),
            client_version: "0.1.0".into(),
            server_info: None,
            client_info: None,
            baseline: None,
            attempts: vec![RequestAttempt {
                attempt_id: Uuid::new_v4(),
                run_id,
                protocol: crate::metrics::Protocol::Browser,
                sequence_num: 0,
                started_at: now,
                finished_at: Some(now),
                success: true,
                dns: None,
                tcp: None,
                tls: None,
                http: None,
                udp: None,
                error: None,
                retry_count: 0,
                server_timing: None,
                udp_throughput: None,
                page_load: None,
                browser: Some(crate::metrics::BrowserResult {
                    load_ms: 350.0,
                    dom_content_loaded_ms: 200.0,
                    ttfb_ms: 50.0,
                    resource_count: 21,
                    transferred_bytes: 204_800,
                    protocol: "h2".into(),
                    resource_protocols: vec![("h2".into(), 21)],
                    started_at: now,
                }),
            }],
        };
        let html = render(&run, None);
        assert!(
            html.contains("Browser Results"),
            "should have Browser Results section"
        );
    }
}
