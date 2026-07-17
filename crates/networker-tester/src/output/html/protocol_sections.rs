//! Per-protocol report sections (timing tables, throughput, TLS, page-load,
//! browser, UDP, statistics) rendered by `write_protocol_sections`.

use super::*;

pub(super) fn write_protocol_sections(run: &TestRun, out: &mut String, stack_filter: Option<&str>) {
    use crate::metrics::compute_stats;

    // Label suffix for chart data points (e.g. " endpoint", " iis", " nginx")
    let label_suffix = match stack_filter {
        None => " endpoint".to_string(),
        Some(s) => format!(" {s}"),
    };

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
            .filter(|a| &a.protocol == proto && a.http_stack.as_deref() == stack_filter)
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
                    .filter(|a| a.protocol == *proto && a.http_stack.as_deref() == stack_filter)
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
                    .filter(|a| {
                        &a.protocol == proto
                            && attempt_payload_bytes(a) == *payload
                            && a.http_stack.as_deref() == stack_filter
                    })
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
                // p95/p99 suppressed below the sample-size guard (n≥20 / n≥100).
                let fmt_pctl =
                    |v: Option<f64>| v.map_or_else(|| "—".to_string(), |x| format!("{x:.2}"));
                let _ = write!(
                    out,
                    r#"      <tr>
        <td>{label}</td>
        <td>{metric}</td>
        <td>{count}</td>
        <td>{min:.2}</td>
        <td>{mean:.2}</td>
        <td>{p50:.2}</td>
        <td>{p95}</td>
        <td>{p99}</td>
        <td>{max:.2}</td>
        <td>{stddev:.2}</td>
        <td class="{ok_cls}">{pct:.0}%</td>
      </tr>
"#,
                    count = s.count,
                    min = s.min,
                    mean = s.mean,
                    p50 = s.p50,
                    p95 = fmt_pctl(s.p95),
                    p99 = fmt_pctl(s.p99),
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
                    && a.http_stack.as_deref() == stack_filter
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
                // Check if any rows use connection reuse
                let has_cold = rows
                    .iter()
                    .any(|a| a.page_load.as_ref().is_some_and(|p| !p.connection_reused));
                let has_warm = rows
                    .iter()
                    .any(|a| a.page_load.as_ref().is_some_and(|p| p.connection_reused));
                // If both cold and warm exist, show separate rows
                let subsets: Vec<(&str, Vec<&RequestAttempt>)> = if has_cold && has_warm {
                    vec![
                        (
                            "cold",
                            rows.iter()
                                .filter(|a| {
                                    a.page_load.as_ref().is_some_and(|p| !p.connection_reused)
                                })
                                .copied()
                                .collect(),
                        ),
                        (
                            "warm",
                            rows.iter()
                                .filter(|a| {
                                    a.page_load.as_ref().is_some_and(|p| p.connection_reused)
                                })
                                .copied()
                                .collect(),
                        ),
                    ]
                } else {
                    vec![("", rows)]
                };
                for (suffix, subset) in subsets {
                    let n = subset.len();
                    if n == 0 {
                        continue;
                    }
                    let pl_rows: Vec<&crate::metrics::PageLoadResult> =
                        subset.iter().filter_map(|a| a.page_load.as_ref()).collect();
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
                    let label = if suffix.is_empty() {
                        proto.to_string()
                    } else {
                        format!("{proto} <small>({suffix})</small>")
                    };
                    let total_ms_vals: Vec<f64> = pl_rows.iter().map(|p| p.total_ms).collect();
                    if let Some(s) = compute_stats(&total_ms_vals) {
                        let _ = write!(
                            out,
                            r#"      <tr>
        <td>{label}</td>
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
                            label = label,
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
                    && a.http_stack.as_deref() == stack_filter
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
                    && a.http_stack.as_deref() == stack_filter
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
                    && a.http_stack.as_deref() == stack_filter
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
                    && a.http_stack.as_deref() == stack_filter
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
                        bars.push((format!("{proto}{}", label_suffix), avg, color));
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
                        bars.push((format!("{proto}{}", label_suffix), avg, color));
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
                    let label = format!("{proto}{}", label_suffix);
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
                        groups.push((format!("{proto}{}", label_suffix), data, color));
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
                        groups.push((format!("{proto}{}", label_suffix), data, color));
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
                        groups.push((format!("{proto}{}", label_suffix), data, color));
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
                        groups.push((format!("{proto}{}", label_suffix), data, color));
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
                        groups.push((format!("{proto}{}", label_suffix), data, color));
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
                        series.push((format!("{proto}{}", label_suffix), data, color));
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
                        series.push((format!("{proto}{}", label_suffix), data, color));
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

                // Connection-reuse analysis: cold (warmup) vs warm probes
                for proto in &[Protocol::PageLoad2, Protocol::PageLoad3] {
                    let cold: Vec<f64> = chart_pl
                        .iter()
                        .filter(|a| &a.protocol == proto)
                        .filter_map(|a| a.page_load.as_ref())
                        .filter(|p| !p.connection_reused)
                        .map(|p| p.total_ms)
                        .collect();
                    let warm: Vec<f64> = chart_pl
                        .iter()
                        .filter(|a| &a.protocol == proto)
                        .filter_map(|a| a.page_load.as_ref())
                        .filter(|p| p.connection_reused)
                        .map(|p| p.total_ms)
                        .collect();
                    if !cold.is_empty() && !warm.is_empty() {
                        let cold_avg = cold.iter().sum::<f64>() / cold.len() as f64;
                        let warm_avg = warm.iter().sum::<f64>() / warm.len() as f64;
                        let saved = cold_avg - warm_avg;
                        let pct = if cold_avg > 0.0 {
                            saved / cold_avg * 100.0
                        } else {
                            0.0
                        };
                        let label = proto.to_string();
                        observations.push(format!(
                            "Connection reuse ({label}): cold {cold_avg:.1}ms → warm {warm_avg:.1}ms \
                             ({saved:+.1}ms, {pct:.0}% faster)"
                        ));
                        // Show TLS savings
                        let cold_tls: f64 = chart_pl
                            .iter()
                            .filter(|a| &a.protocol == proto)
                            .filter_map(|a| a.page_load.as_ref())
                            .filter(|p| !p.connection_reused)
                            .map(|p| p.tls_setup_ms)
                            .sum::<f64>()
                            / cold.len() as f64;
                        if cold_tls > 0.5 {
                            observations.push(format!(
                                "  TLS handshake saved per warm run ({label}): {cold_tls:.1}ms"
                            ));
                        }
                    }
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
}
