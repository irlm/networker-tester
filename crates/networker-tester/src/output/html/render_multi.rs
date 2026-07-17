//! Multi-target report assembly (`render_multi`) plus the shared page
//! structural helpers: HTML head, footer, and host-info card.

use super::*;

/// Render a combined report for multiple targets.
///
/// Single-target runs delegate to `render()` for identical output.
pub fn render_multi(
    runs: &[TestRun],
    css_href: Option<&str>,
    packet_capture: Option<&PacketCaptureSummary>,
) -> String {
    if runs.len() == 1 {
        return render(&runs[0], css_href, packet_capture);
    }

    let mut out = String::with_capacity(128 * 1024);
    let title = format!("{} targets compared", runs.len());
    write_html_head(&title, css_href, &mut out);

    // ── Multi-target page header ──────────────────────────────────────────────
    let started = runs[0].started_at.format("%Y-%m-%d %H:%M:%S UTC");
    let client_ver = &runs[0].client_version;
    let server_ver_first = runs
        .iter()
        .find_map(|r| {
            r.attempts.iter().find_map(|a| {
                a.server_timing
                    .as_ref()
                    .and_then(|st| st.server_version.as_deref())
            })
        })
        .unwrap_or("—");
    let _ = write!(
        out,
        r#"
<header class="page-header">
  <h1>Networker Tester</h1>
  <p class="subtitle">{n} targets compared &bull; {started}</p>
  <p class="subtitle"><strong>Client</strong> v{client_ver} &bull; <strong>Server</strong> v{server_ver}</p>
</header>
"#,
        n = runs.len(),
        client_ver = escape_html(client_ver),
        server_ver = escape_html(server_ver_first),
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
        let server_summary = run
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
                let version_badge = s
                    .server_version
                    .as_ref()
                    .map(|v| format!(" <code>v{v}</code>"))
                    .unwrap_or_default();
                let region = s
                    .region
                    .as_ref()
                    .map(|r| format!("<br><small>Region: {r}</small>"))
                    .unwrap_or_default();
                let display_name = derive_display_name(Some(s), "");
                format!(
                    "{display_name}{version_badge}<br><small>{os} | {} cores | {mem}</small>{region}",
                    s.cpu_cores
                )
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
            .map(|b| {
                if b.samples == 0 {
                    "—".into()
                } else {
                    format!("{:.2} ms", b.rtt_avg_ms)
                }
            })
            .unwrap_or_else(|| "—".into());
        let ep_attempts: Vec<_> = run
            .attempts
            .iter()
            .filter(|a| a.http_stack.is_none())
            .collect();
        let ep_ok = ep_attempts.iter().filter(|a| a.success).count();
        let ep_fail = ep_attempts.len() - ep_ok;
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
            attempts = ep_attempts.len(),
            ok = ep_ok,
            fail = ep_fail,
            fail_cls = if ep_fail > 0 { "err" } else { "ok" },
        );
    }
    let _ = writeln!(out, "    </tbody>\n  </table>\n</section>");

    // ── Short names for each target (used in headers, charts, observations) ──
    let short_names = build_target_short_names(runs);

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
            .filter(|a| &a.protocol == proto && a.http_stack.is_none())
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
        for (i, _run) in runs.iter().enumerate() {
            let _ = writeln!(out, "        <th>{}</th>", escape_html(&short_names[i]));
        }
        let _ = writeln!(out, "      </tr>\n    </thead>\n    <tbody>");

        // Pick the "best overall" Internet target as the diff baseline.
        // LAN/Loopback targets show raw values only (as reference).
        let is_lan = |run: &TestRun| -> bool {
            run.baseline
                .as_ref()
                .is_some_and(|b| matches!(b.network_type, NetworkType::LAN | NetworkType::Loopback))
        };

        // Compute composite score: for each protocol, rank Internet targets
        // (1 = best). Sum ranks across all protocols. Lowest total = best overall.
        let internet_indices: Vec<usize> = runs
            .iter()
            .enumerate()
            .filter(|(_, r)| !is_lan(r))
            .map(|(i, _)| i)
            .collect();

        let best_internet_idx = if internet_indices.len() <= 1 {
            internet_indices.first().copied()
        } else {
            let mut rank_sums: Vec<(usize, f64)> =
                internet_indices.iter().map(|&i| (i, 0.0)).collect();

            for proto in &active_protos {
                let is_throughput = matches!(
                    proto,
                    Protocol::Download
                        | Protocol::Upload
                        | Protocol::WebDownload
                        | Protocol::WebUpload
                        | Protocol::UdpDownload
                        | Protocol::UdpUpload
                );
                // Collect (index_in_rank_sums, value) for Internet targets with data
                let mut vals: Vec<(usize, f64)> = rank_sums
                    .iter()
                    .enumerate()
                    .filter_map(|(ri, &(run_i, _))| {
                        avg_primary(&runs[run_i], proto).map(|v| (ri, v))
                    })
                    .collect();
                if vals.is_empty() {
                    continue;
                }
                // Sort: best first (highest throughput or lowest latency)
                if is_throughput {
                    vals.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                } else {
                    vals.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
                }
                // Assign ranks (1-based)
                for (rank, &(ri, _)) in vals.iter().enumerate() {
                    rank_sums[ri].1 += (rank + 1) as f64;
                }
            }

            rank_sums
                .iter()
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|&(i, _)| i)
        };

        for proto in &active_protos {
            let baseline = best_internet_idx.and_then(|idx| avg_primary(&runs[idx], proto));
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
                        // LAN/Loopback targets and the baseline target: show raw value only
                        if is_lan(run) || Some(i) == best_internet_idx || baseline.is_none() {
                            if is_lan(run) {
                                // Dim LAN reference values
                                let _ = writeln!(
                                    out,
                                    "        <td style=\"opacity:.55\">{v:.2} <small>(ref)</small></td>",
                                );
                            } else {
                                let _ = writeln!(out, "        <td>{v:.2}</td>");
                            }
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
        let _ = writeln!(out, "    </tbody>\n  </table>");

        // ── Cross-target observations ────────────────────────────────────────
        {
            let mut observations: Vec<String> = Vec::new();

            // For each active protocol, find fastest/slowest among Internet targets only
            for proto in &active_protos {
                let avgs: Vec<(usize, f64)> = runs
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| !is_lan(r))
                    .filter_map(|(i, r)| avg_primary(r, proto).map(|v| (i, v)))
                    .collect();
                if avgs.len() >= 2 {
                    let is_throughput = matches!(
                        proto,
                        Protocol::Download
                            | Protocol::Upload
                            | Protocol::WebDownload
                            | Protocol::WebUpload
                            | Protocol::UdpDownload
                            | Protocol::UdpUpload
                    );
                    let (best_idx, best_v) = if is_throughput {
                        *avgs
                            .iter()
                            .max_by(|a, b| {
                                a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
                            })
                            .unwrap()
                    } else {
                        *avgs
                            .iter()
                            .min_by(|a, b| {
                                a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
                            })
                            .unwrap()
                    };
                    let (worst_idx, worst_v) = if is_throughput {
                        *avgs
                            .iter()
                            .min_by(|a, b| {
                                a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
                            })
                            .unwrap()
                    } else {
                        *avgs
                            .iter()
                            .max_by(|a, b| {
                                a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
                            })
                            .unwrap()
                    };
                    if (best_v - worst_v).abs() > 0.01 {
                        let diff_pct = if worst_v.abs() > 0.001 {
                            (worst_v - best_v).abs() / worst_v * 100.0
                        } else {
                            0.0
                        };
                        let label = primary_metric_label(proto);
                        let word = if is_throughput { "higher" } else { "faster" };
                        observations.push(format!(
                            "<strong>{proto}</strong> ({label}): {} is fastest ({best_v:.2}) \u{2014} \
                             {diff_pct:.1}% {word} than {} ({worst_v:.2})",
                            short_names[best_idx],
                            short_names[worst_idx],
                        ));
                    }
                }
            }

            // Network type comparison
            let net_types: Vec<(usize, &str)> = runs
                .iter()
                .enumerate()
                .filter_map(|(i, r)| {
                    r.baseline.as_ref().map(|b| {
                        (
                            i,
                            match b.network_type {
                                NetworkType::Loopback => "Loopback",
                                NetworkType::LAN => "LAN",
                                NetworkType::Internet => "Internet",
                            },
                        )
                    })
                })
                .collect();
            if net_types.len() >= 2 {
                let types_summary: Vec<String> = net_types
                    .iter()
                    .map(|(i, t)| format!("{} = {t}", short_names[*i]))
                    .collect();
                let all_same = net_types.iter().all(|(_, t)| *t == net_types[0].1);
                if all_same {
                    observations.push(format!(
                        "All targets are {} connections ({})",
                        net_types[0].1,
                        types_summary.join(", ")
                    ));
                } else {
                    observations.push(format!(
                        "Mixed network types: {} \u{2014} latency differences may reflect network distance",
                        types_summary.join(", ")
                    ));
                }
            }

            // RTT baseline comparison (exclude unreachable targets with samples=0)
            let rtts: Vec<(usize, f64)> = runs
                .iter()
                .enumerate()
                .filter_map(|(i, r)| {
                    r.baseline
                        .as_ref()
                        .filter(|b| b.samples > 0)
                        .map(|b| (i, b.rtt_avg_ms))
                })
                .collect();
            if rtts.len() >= 2 {
                let best = rtts
                    .iter()
                    .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                    .unwrap();
                let worst = rtts
                    .iter()
                    .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                    .unwrap();
                if (worst.1 - best.1) > 0.01 {
                    observations.push(format!(
                        "Baseline RTT: {} lowest ({:.2}ms avg), {} highest ({:.2}ms avg) \u{2014} {:.1}ms difference",
                        short_names[best.0],
                        best.1,
                        short_names[worst.0],
                        worst.1,
                        worst.1 - best.1
                    ));
                }
            }

            if !observations.is_empty() {
                let _ = writeln!(
                    out,
                    "  <div class=\"analysis\"><h3>Cross-Target Observations</h3><ul>"
                );
                for obs in &observations {
                    let _ = writeln!(out, "    <li>{obs}</li>");
                }
                let _ = writeln!(out, "  </ul></div>");
            }
        }

        let _ = writeln!(out, "</section>");

        // ── Per-target load time distribution + observations (2-col grid) ────
        write_multi_target_charts(runs, &short_names, &mut out);
    }

    // ── Per-target collapsible sections ───────────────────────────────────────
    for (i, run) in runs.iter().enumerate() {
        let open = if runs.len() <= 2 { " open" } else { "" };
        let _ = write!(
            out,
            "\n<details class=\"card multi-target-details\"{open}>\n  <summary><strong>{}:</strong> {}</summary>\n",
            escape_html(&short_names[i]),
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

pub(super) fn write_html_head(title: &str, css_href: Option<&str>, out: &mut String) {
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

pub(super) fn write_html_footer(timestamp: DateTime<chrono::Utc>, out: &mut String) {
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
pub(super) fn write_host_info_card(label: &str, info: &HostInfo, out: &mut String) {
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
