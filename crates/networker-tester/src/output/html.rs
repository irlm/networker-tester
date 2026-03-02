/// Generate a self-contained HTML diagnostic report from a `TestRun`.
///
/// The report embeds a minimal inline CSS for offline viewing and optionally
/// adds a `<link rel="stylesheet">` for the external `report.css` file so
/// operators can customize the look without editing generated HTML.
use crate::metrics::{
    attempt_payload_bytes, compute_stats, primary_metric_label, primary_metric_value, Protocol,
    RequestAttempt, TestRun,
};
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

pub fn render(run: &TestRun, css_href: Option<&str>) -> String {
    let mut out = String::with_capacity(64 * 1024);

    // ── Head ─────────────────────────────────────────────────────────────────
    let _ = write!(
        out,
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Networker Tester – {target}</title>
  <style>{inline_css}</style>
"#,
        target = escape_html(&run.target_url),
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
        r#"
<section class="card">
  <h2>Run Summary</h2>
  <dl class="summary-grid">
    <dt>Target</dt>          <dd><a href="{url}">{url}</a></dd>
    <dt>Modes</dt>           <dd>{modes}</dd>
    <dt>Attempts</dt>        <dd>{total}</dd>
    <dt>Succeeded</dt>       <dd class="ok">{ok}</dd>
    <dt>Failed</dt>          <dd class="{fail_cls}">{fail}</dd>
    <dt>Total Duration</dt>  <dd>{dur}</dd>
    <dt>OS</dt>              <dd>{os}</dd>
    <dt>Client version</dt>  <dd>{client_ver}</dd>
    <dt>Server version</dt>  <dd>{server_ver}</dd>
  </dl>
</section>
"#,
        url = escape_html(&run.target_url),
        modes = run.modes.join(", "),
        total = run.attempts.len(),
        ok = run.success_count(),
        fail = run.failure_count(),
        fail_cls = if run.failure_count() > 0 { "err" } else { "ok" },
        dur = duration_s,
        os = escape_html(&run.client_os),
        client_ver = escape_html(&run.client_version),
        server_ver = escape_html(server_ver),
    );

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
        append_proto_row(&mut out, proto, &rows);
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

    // ── Browser Results ───────────────────────────────────────────────────────
    {
        let browser_rows: Vec<&RequestAttempt> = run
            .attempts
            .iter()
            .filter(|a| a.protocol == Protocol::Browser && a.browser.is_some())
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
            append_attempt_row(&mut out, a);
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

    // ── Footer ────────────────────────────────────────────────────────────────
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
        run.finished_at
            .unwrap_or(run.started_at)
            .format("%Y-%m-%d %H:%M:%S UTC"),
    );

    out
}

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
