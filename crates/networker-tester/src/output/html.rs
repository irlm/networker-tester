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

/// Per-target load time distribution charts + observations in a 2-column grid.
/// Only rendered when there are multiple targets and at least one has pageload/browser data.
fn write_multi_target_charts(runs: &[TestRun], short_names: &[String], out: &mut String) {
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

fn write_protocol_sections(run: &TestRun, out: &mut String, stack_filter: Option<&str>) {
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

fn write_run_sections(run: &TestRun, out: &mut String) {
    // ── Header ────────────────────────────────────────────────────────────────
    let server_ver_header = run
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
<header class="page-header">
  <h1>Networker Tester</h1>
  <p class="subtitle">Run <code>{run_id}</code> &bull; {started}</p>
  <p class="subtitle"><strong>Client</strong> v{client_ver} &bull; <strong>Server</strong> v{server_ver}</p>
</header>
"#,
        run_id = run.run_id,
        started = run.started_at.format("%Y-%m-%d %H:%M:%S UTC"),
        client_ver = escape_html(&run.client_version),
        server_ver = escape_html(server_ver_header),
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

    let ep_attempts: Vec<_> = run
        .attempts
        .iter()
        .filter(|a| a.http_stack.is_none())
        .collect();
    let ep_ok = ep_attempts.iter().filter(|a| a.success).count();
    let ep_fail = ep_attempts.len() - ep_ok;
    let stack_count = run.attempts.len() - ep_attempts.len();
    let stack_note = if stack_count > 0 {
        format!(" <small>(+ {stack_count} stack probes)</small>")
    } else {
        String::new()
    };
    let _ = write!(
        out,
        r##"
<section class="card">
  <h2>Run Summary</h2>
  <dl class="summary-grid">
    <dt>Target</dt>          <dd><a href="{url}">{url}</a></dd>
    <dt>Modes</dt>           <dd>{modes}</dd>
    <dt>Attempts</dt>        <dd>{total}{stack_note}</dd>
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
        total = ep_attempts.len(),
        stack_note = stack_note,
        ok = ep_ok,
        fail = ep_fail,
        fail_cls = if ep_fail > 0 { "err" } else { "ok" },
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

    // ── Protocol sections for endpoint (default) ─────────────────────────────
    write_protocol_sections(run, out, None);

    // ── Protocol sections for each HTTP stack ────────────────────────────────
    {
        let stack_names: Vec<String> = {
            let names: std::collections::BTreeSet<String> = run
                .attempts
                .iter()
                .filter_map(|a| a.http_stack.clone())
                .collect();
            names.into_iter().collect()
        };
        for stack_name in &stack_names {
            let stack_total = run
                .attempts
                .iter()
                .filter(|a| a.http_stack.as_deref() == Some(stack_name.as_str()))
                .count();
            let stack_ok = run
                .attempts
                .iter()
                .filter(|a| a.http_stack.as_deref() == Some(stack_name.as_str()) && a.success)
                .count();
            let stack_fail = stack_total - stack_ok;
            let fail_cls = if stack_fail > 0 { "err" } else { "ok" };
            let _ = write!(
                out,
                r#"
<hr style="border:none;border-top:3px solid #1a1a2e;margin:2.5rem 2rem 0">
<section class="card" style="border-top:3px solid #4e79a7">
  <h2 style="font-size:1.3rem">{name} Stack Results</h2>
  <dl class="summary-grid">
    <dt>Attempts</dt>   <dd>{total}</dd>
    <dt>Succeeded</dt>  <dd class="ok">{ok}</dd>
    <dt>Failed</dt>     <dd class="{fail_cls}">{fail}</dd>
  </dl>
</section>
"#,
                name = escape_html(&stack_name.to_uppercase()),
                total = stack_total,
                ok = stack_ok,
                fail = stack_fail,
                fail_cls = fail_cls,
            );
            write_protocol_sections(run, out, Some(stack_name.as_str()));
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
        let succeeded = run.attempts.iter().filter(|a| a.success).count();
        let failed = total_attempts - succeeded;
        let open_attr = if total_attempts <= 20 { " open" } else { "" };
        let stack_count = run
            .attempts
            .iter()
            .filter(|a| a.http_stack.is_some())
            .count();
        let summary_meta = if stack_count > 0 {
            format!("{succeeded} succeeded · {failed} failed · {stack_count} stack probes")
        } else {
            format!("{succeeded} succeeded · {failed} failed")
        };
        let has_stacks = stack_count > 0;
        let stack_th = if has_stacks { "<th>Stack</th>" } else { "" };
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
          <th>#</th><th>Protocol</th>{stack_th}<th>Status</th>
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
            stack_th = stack_th,
        );

        for a in &run.attempts {
            append_attempt_row(out, a, has_stacks);
        }
        let _ = writeln!(
            out,
            "      </tbody>\n    </table>\n  </details>\n</section>"
        );
    }

    // ── TCP kernel stats ─────────────────────────────────────────────────────
    let has_stacks_for_tcp = run.attempts.iter().any(|a| a.http_stack.is_some());
    let tcp_rows: Vec<&RequestAttempt> = run.attempts.iter().filter(|a| a.tcp.is_some()).collect();
    if !tcp_rows.is_empty() {
        let open_attr = if tcp_rows.len() <= 20 { " open" } else { "" };
        let tcp_stack_th = if has_stacks_for_tcp {
            "<th>Stack</th>"
        } else {
            ""
        };
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
          <th>#</th><th>Protocol</th>{tcp_stack_th}<th>Local → Remote</th>
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
            tcp_stack_th = tcp_stack_th,
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
            let tcp_stack_td = if has_stacks_for_tcp {
                let label = match a.http_stack.as_deref() {
                    Some(s) => escape_html(s),
                    None => "endpoint".into(),
                };
                format!("\n          <td>{label}</td>")
            } else {
                String::new()
            };
            let _ = write!(
                out,
                r#"        <tr>
          <td>{seq}</td>
          <td>{proto}</td>{tcp_stack_td}
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
                tcp_stack_td = tcp_stack_td,
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
        // Note explaining why browser probes are absent from TCP Stats.
        let has_browser = run.attempts.iter().any(|a| {
            matches!(
                a.protocol,
                crate::metrics::Protocol::Browser
                    | crate::metrics::Protocol::Browser1
                    | crate::metrics::Protocol::Browser2
                    | crate::metrics::Protocol::Browser3
            )
        });
        if has_browser {
            let _ = writeln!(
                out,
                r##"  <p class="note">Browser probes (browser1/browser2/browser3) are not shown here &mdash; Chrome owns the TCP connections internally, so kernel-level socket stats (MSS, cwnd, retransmits, congestion algorithm, etc.) are not accessible from our process.</p>"##
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
        let open_attr = if tls_rows.len() <= 20 { " open" } else { "" };
        let _ = write!(
            out,
            r#"
<section class="card">
  <h2>TLS Details</h2>
  <details{open}>
    <summary><span class="grp-lbl">{n} handshakes</span></summary>
    <table>
      <thead>
        <tr><th>#</th><th>Version</th><th>Cipher</th><th>ALPN</th>
            <th>Cert Subject</th><th>Cert Expiry</th><th>Handshake (ms)</th></tr>
      </thead>
      <tbody>
"#,
            open = open_attr,
            n = tls_rows.len(),
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
        let _ = writeln!(
            out,
            "      </tbody>\n    </table>\n  </details>\n</section>"
        );
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

fn append_attempt_row(out: &mut String, a: &RequestAttempt, show_stack: bool) {
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

    let stack_td = if show_stack {
        let label = match a.http_stack.as_deref() {
            Some(s) => escape_html(s),
            None => "endpoint".into(),
        };
        format!("<td>{label}</td>")
    } else {
        String::new()
    };
    let _ = write!(
        out,
        r#"      <tr class="{row_cls}">
        <td>{seq}</td><td>{proto}</td>{stack_td}<td>{status}</td>
        <td>{dns}</td><td>{tcp}</td><td>{tls}</td>
        <td>{ttfb}</td><td>{total}</td>
        <td>{ver}</td><td>{err}</td>
      </tr>
"#,
        row_cls = if a.success { "" } else { "row-err" },
        seq = a.sequence_num,
        proto = a.protocol,
        stack_td = stack_td,
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

fn write_packet_capture_section(packet_capture: Option<&PacketCaptureSummary>, out: &mut String) {
    let Some(summary) = packet_capture else {
        return;
    };
    let _ = write!(
        out,
        r#"
<section class="card">
  <h2>Packet Capture Summary</h2>
  <p><strong>Status:</strong> {status} &bull; <strong>Interface:</strong> {iface} &bull; <strong>Total packets:</strong> {total}</p>
  <p><strong>Observed transport:</strong> QUIC={oq} &bull; TCP-only={ot} &bull; Mixed transport={om} &bull; Ambiguous={amb}</p>
  <table>
    <thead><tr><th>Protocol</th><th>Packets</th><th>% of total</th></tr></thead>
    <tbody>
"#,
        status = escape_html(&summary.capture_status),
        iface = escape_html(&summary.interface),
        total = summary.total_packets,
        oq = summary.observed_quic,
        ot = summary.observed_tcp_only,
        om = summary.observed_mixed_transport,
        amb = summary.capture_may_be_ambiguous,
    );
    for row in &summary.transport_shares {
        let _ = write!(
            out,
            "<tr><td>{}</td><td>{}</td><td>{:.1}%</td></tr>",
            escape_html(&row.protocol),
            row.packets,
            row.pct_of_total
        );
    }
    let _ = write!(out, "</tbody></table>");
    if !summary.likely_target_endpoints.is_empty() {
        let _ = write!(out, "<p><strong>Likely target endpoints:</strong> ");
        for (i, endpoint) in summary.likely_target_endpoints.iter().enumerate() {
            if i > 0 {
                let _ = write!(out, ", ");
            }
            let _ = write!(out, "<code>{}</code>", escape_html(endpoint));
        }
        let _ = write!(
            out,
            " &bull; <strong>Likely target packets:</strong> {} ({:.1}%) &bull; <strong>Confidence:</strong> {}",
            summary.likely_target_packets,
            summary.likely_target_pct_of_total,
            escape_html(&summary.capture_confidence)
        );
        if let Some(port) = summary.dominant_trace_port {
            let _ = write!(
                out,
                " &bull; <strong>Dominant trace port:</strong> <code>{}</code>",
                port
            );
        }
        let _ = write!(out, "</p>");
    }
    if !summary.top_endpoints.is_empty() {
        let _ = write!(out, "<h3>Top Endpoints</h3><ul>");
        for row in &summary.top_endpoints {
            let _ = write!(
                out,
                "<li><code>{}</code> — {} packets</li>",
                escape_html(&row.endpoint),
                row.packets
            );
        }
        let _ = write!(out, "</ul>");
    }
    if !summary.top_ports.is_empty() {
        let _ = write!(out, "<h3>Top Ports</h3><ul>");
        for row in &summary.top_ports {
            let _ = write!(
                out,
                "<li><code>{}</code> — {} packets</li>",
                row.port, row.packets
            );
        }
        let _ = write!(out, "</ul>");
    }
    if let Some(note) = &summary.note {
        let _ = write!(out, "<p><strong>Note:</strong> {}</p>", escape_html(note));
    }
    if !summary.warnings.is_empty() {
        let _ = write!(out, "<h3>Warnings</h3><ul>");
        for warning in &summary.warnings {
            let _ = write!(out, "<li>{}</li>", escape_html(warning));
        }
        let _ = write!(out, "</ul>");
    }
    let _ = write!(out, "</section>");
}

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
            packet_capture_summary: None,
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
                http_stack: None,
            }],
        }
    }

    #[test]
    fn html_contains_target() {
        let run = make_run();
        let html = render(&run, None, None);
        assert!(html.contains("localhost/health"));
    }

    #[test]
    fn html_contains_http11() {
        let run = make_run();
        let html = render(&run, None, None);
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

    fn sample_packet_capture_summary() -> crate::capture::PacketCaptureSummary {
        crate::capture::PacketCaptureSummary {
            mode: "tester".into(),
            interface: "lo0".into(),
            capture_path: "packet-capture-tester.pcapng".into(),
            tshark_path: "tshark".into(),
            total_packets: 42,
            capture_status: "captured".into(),
            note: Some("Capture note".into()),
            warnings: vec!["Ambiguous trace".into()],
            likely_target_endpoints: vec!["127.0.0.1".into()],
            likely_target_packets: 20,
            likely_target_pct_of_total: 47.6,
            dominant_trace_port: Some(443),
            capture_confidence: "medium".into(),
            tcp_packets: 10,
            udp_packets: 20,
            quic_packets: 15,
            http_packets: 5,
            dns_packets: 2,
            retransmissions: 1,
            duplicate_acks: 0,
            resets: 0,
            transport_shares: vec![crate::capture::PacketShare {
                protocol: "udp".into(),
                packets: 20,
                pct_of_total: 47.6,
            }],
            top_endpoints: vec![crate::capture::EndpointPacketCount {
                endpoint: "127.0.0.1".into(),
                packets: 20,
            }],
            top_ports: vec![crate::capture::PortPacketCount {
                port: 443,
                packets: 18,
            }],
            observed_quic: true,
            observed_tcp_only: false,
            observed_mixed_transport: true,
            capture_may_be_ambiguous: true,
        }
    }

    #[test]
    fn render_includes_packet_capture_section_when_present() {
        let run = make_run();
        let html = render(&run, None, Some(&sample_packet_capture_summary()));
        assert!(html.contains("Packet Capture Summary"));
        assert!(html.contains("Likely target endpoints"));
        assert!(html.contains("127.0.0.1"));
        assert!(html.contains("Confidence"));
        assert!(html.contains("Dominant trace port"));
        assert!(html.contains("Ambiguous trace"));
    }

    #[test]
    fn save_writes_html_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let run = make_run();
        save(&run, tmp.path(), None, None).unwrap();
        let content = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(content.starts_with("<!DOCTYPE html>"));
    }

    #[test]
    fn html_includes_css_link_when_href_provided() {
        let run = make_run();
        let html = render(&run, Some("report.css"), None);
        assert!(html.contains(r#"<link rel="stylesheet""#));
        assert!(html.contains("report.css"));
    }

    #[test]
    fn html_no_css_link_without_href() {
        let run = make_run();
        let html = render(&run, None, None);
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
            packet_capture_summary: None,
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
                http_stack: None,
            }],
        };
        let html = render(&run, None, None);
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
            packet_capture_summary: None,
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
                http_stack: None,
            }],
        };
        let html = render(&run, None, None);
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
            packet_capture_summary: None,
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
                    resumed: None,
                    handshake_kind: None,
                    tls13_tickets_received: None,
                    previous_handshake_duration_ms: None,
                    previous_handshake_kind: None,
                    previous_http_status_code: None,
                    http_status_code: None,
                }),
                http: None,
                udp: None,
                error: None,
                retry_count: 0,
                server_timing: None,
                udp_throughput: None,
                page_load: None,
                browser: None,
                http_stack: None,
            }],
        };
        let html = render(&run, None, None);
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
            packet_capture_summary: None,
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
                    connection_reused: false,
                }),
                browser: None,
                http_stack: None,
            }],
        };
        let html = render(&run, None, None);
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
            http_stack: None,
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
            http_stack: None,
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
        append_attempt_row(&mut out, &a, false);
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
        append_attempt_row(&mut out, &a, false);
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
            http_stack: None,
        };
        let mut out = String::new();
        append_attempt_row(&mut out, &a, false);
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
            http_stack: None,
        };
        let mut out = String::new();
        append_attempt_row(&mut out, &a, false);
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
            http_stack: None,
        };
        let mut out = String::new();
        append_attempt_row(&mut out, &a, false);
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
        append_attempt_row(&mut out, &a, false);
        assert!(out.contains("12.34 MB/s"), "should show throughput");
        assert!(out.contains("1.0 MiB"), "should show payload size");
    }

    #[test]
    fn append_attempt_row_with_stack_shows_stack_column() {
        let mut a = make_http_attempt(true, 5.0, 100.0);
        a.http_stack = Some("nginx".into());
        let mut out = String::new();
        append_attempt_row(&mut out, &a, true);
        assert!(out.contains("<td>nginx</td>"), "should show stack name");
    }

    #[test]
    fn append_attempt_row_endpoint_shows_endpoint_label() {
        let a = make_http_attempt(true, 5.0, 100.0);
        let mut out = String::new();
        append_attempt_row(&mut out, &a, true);
        assert!(
            out.contains("<td>endpoint</td>"),
            "should show 'endpoint' for non-stack"
        );
    }

    #[test]
    fn append_attempt_row_no_stack_column_when_disabled() {
        let a = make_http_attempt(true, 5.0, 100.0);
        let mut out = String::new();
        append_attempt_row(&mut out, &a, false);
        assert!(
            !out.contains("<td>endpoint</td>"),
            "should not show stack column"
        );
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
            packet_capture_summary: None,
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
                http_stack: None,
            }],
        };
        let html = render(&run, None, None);
        assert!(
            html.contains("Browser Results"),
            "should have Browser Results section"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Helpers / fixture builders shared by new tests
    // ─────────────────────────────────────────────────────────────────────────

    fn make_run_with_url(url: &str) -> TestRun {
        let run_id = Uuid::new_v4();
        TestRun {
            run_id,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            target_url: url.to_string(),
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
            packet_capture_summary: None,
            attempts: vec![],
        }
    }

    /// Build a minimal successful HTTP/1.1 attempt.
    fn make_attempt(proto: Protocol, success: bool) -> RequestAttempt {
        let run_id = Uuid::new_v4();
        RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id,
            protocol: proto.clone(),
            sequence_num: 0,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            success,
            dns: None,
            tcp: None,
            tls: None,
            http: if matches!(
                proto,
                Protocol::Http1
                    | Protocol::Http2
                    | Protocol::Http3
                    | Protocol::Native
                    | Protocol::Curl
            ) {
                Some(HttpResult {
                    negotiated_version: "HTTP/1.1".into(),
                    status_code: if success { 200 } else { 500 },
                    headers_size_bytes: 100,
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
                })
            } else {
                None
            },
            udp: None,
            error: None,
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
            http_stack: None,
        }
    }

    fn make_page_load_attempt(
        proto: Protocol,
        total_ms: f64,
        connection_reused: bool,
    ) -> RequestAttempt {
        let mut a = make_attempt(proto, true);
        a.http = None;
        a.page_load = Some(crate::metrics::PageLoadResult {
            asset_count: 10,
            assets_fetched: 10,
            total_bytes: 102_400,
            total_ms,
            ttfb_ms: 20.0,
            connections_opened: 1,
            asset_timings_ms: vec![10.0; 10],
            started_at: Utc::now(),
            tls_setup_ms: 5.0,
            tls_overhead_ratio: 0.05,
            per_connection_tls_ms: vec![5.0],
            cpu_time_ms: None,
            connection_reused,
        });
        a
    }

    fn make_browser_attempt(proto: Protocol, load_ms: f64, ttfb_ms: f64) -> RequestAttempt {
        let mut a = make_attempt(proto, true);
        a.http = None;
        a.browser = Some(crate::metrics::BrowserResult {
            load_ms,
            dom_content_loaded_ms: load_ms * 0.6,
            ttfb_ms,
            resource_count: 15,
            transferred_bytes: 150_000,
            protocol: "h2".into(),
            resource_protocols: vec![("h2".into(), 15)],
            started_at: Utc::now(),
        });
        a
    }

    fn make_baseline(net: NetworkType, rtt: f64) -> crate::metrics::NetworkBaseline {
        crate::metrics::NetworkBaseline {
            samples: 10,
            rtt_min_ms: rtt * 0.9,
            rtt_avg_ms: rtt,
            rtt_max_ms: rtt * 1.1,
            rtt_p50_ms: rtt,
            rtt_p95_ms: rtt * 1.05,
            network_type: net,
        }
    }

    fn make_host_info(
        hostname: Option<&str>,
        os: &str,
        region: Option<&str>,
        server_version: Option<&str>,
    ) -> crate::metrics::HostInfo {
        crate::metrics::HostInfo {
            os: os.to_string(),
            arch: "x86_64".into(),
            cpu_cores: 4,
            total_memory_mb: Some(8192),
            os_version: Some(os.to_string()),
            hostname: hostname.map(|h| h.to_string()),
            server_version: server_version.map(|v| v.to_string()),
            uptime_secs: None,
            region: region.map(|r| r.to_string()),
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // render() — single-target output structure
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn render_single_target_has_html_structure() {
        let run = make_run();
        let html = render(&run, None, None);
        assert!(
            html.starts_with("<!DOCTYPE html>"),
            "must start with DOCTYPE"
        );
        assert!(html.contains("</html>"), "must close html tag");
        assert!(html.contains("<head>"), "must have head element");
        assert!(html.contains("<body>"), "must have body element");
        assert!(html.contains("</body>"), "must close body element");
    }

    #[test]
    fn render_single_target_includes_run_summary() {
        let run = make_run();
        let html = render(&run, None, None);
        assert!(
            html.contains("Run Summary"),
            "should have Run Summary section"
        );
        assert!(
            html.contains("http://localhost/health"),
            "should show target URL"
        );
        assert!(html.contains("Succeeded"), "should show success field");
    }

    #[test]
    fn render_with_no_finished_at_shows_dash_for_duration() {
        let mut run = make_run();
        run.finished_at = None;
        let html = render(&run, None, None);
        // The duration cell shows "—" when finished_at is None
        assert!(
            html.contains("—"),
            "missing finished_at should produce em dash for duration"
        );
    }

    #[test]
    fn render_css_href_produces_link_element() {
        let run = make_run();
        let html = render(&run, Some("/static/report.css"), None);
        assert!(html.contains(r#"<link rel="stylesheet""#));
        assert!(html.contains("/static/report.css"));
    }

    #[test]
    fn render_css_href_escapes_special_chars_in_path() {
        let run = make_run();
        let html = render(&run, Some("path/with&special<chars>"), None);
        // The href value is HTML-escaped
        assert!(html.contains("&amp;"), "& must be escaped in href");
    }

    #[test]
    fn render_shows_network_baseline_when_present() {
        let mut run = make_run();
        run.baseline = Some(make_baseline(NetworkType::Internet, 42.5));
        let html = render(&run, None, None);
        assert!(
            html.contains("Network Baseline"),
            "should have Network Baseline card"
        );
        assert!(html.contains("42.50"), "should show RTT avg value");
        assert!(html.contains("Internet"), "should show network type");
    }

    #[test]
    fn render_network_baseline_loopback_uses_ok_class() {
        let mut run = make_run();
        run.baseline = Some(make_baseline(NetworkType::Loopback, 0.1));
        let html = render(&run, None, None);
        // Loopback maps to "ok" CSS class
        assert!(html.contains("Loopback"), "should show Loopback label");
        // The net_cls for Loopback is "ok"
        assert!(
            html.contains(r#"<span class="ok">Loopback</span>"#),
            "loopback should use ok class"
        );
    }

    #[test]
    fn render_network_baseline_lan_uses_warn_class() {
        let mut run = make_run();
        run.baseline = Some(make_baseline(NetworkType::LAN, 2.0));
        let html = render(&run, None, None);
        assert!(
            html.contains(r#"<span class="warn">LAN</span>"#),
            "LAN should use warn class"
        );
    }

    #[test]
    fn render_network_baseline_internet_uses_err_class() {
        let mut run = make_run();
        run.baseline = Some(make_baseline(NetworkType::Internet, 50.0));
        let html = render(&run, None, None);
        assert!(
            html.contains(r#"<span class="err">Internet</span>"#),
            "Internet should use err class"
        );
    }

    #[test]
    fn render_shows_client_info_card_when_present() {
        let mut run = make_run();
        run.client_info = Some(make_host_info(
            Some("client-host"),
            "Ubuntu 22.04",
            None,
            None,
        ));
        let html = render(&run, None, None);
        assert!(html.contains("Client Info"), "should have Client Info card");
        assert!(html.contains("client-host"), "should show client hostname");
    }

    #[test]
    fn render_shows_server_info_card_when_present() {
        let mut run = make_run();
        run.server_info = Some(make_host_info(
            Some("server-host"),
            "Ubuntu 22.04",
            None,
            Some("0.13.2"),
        ));
        let html = render(&run, None, None);
        assert!(html.contains("Server Info"), "should have Server Info card");
        assert!(html.contains("server-host"), "should show server hostname");
        assert!(
            html.contains("Version"),
            "server card should show Version row"
        );
        assert!(html.contains("0.13.2"), "should show server version");
    }

    #[test]
    fn render_server_info_shows_region_when_present() {
        let mut run = make_run();
        run.server_info = Some(make_host_info(
            Some("vm1"),
            "Ubuntu 22.04",
            Some("azure/eastus"),
            None,
        ));
        let html = render(&run, None, None);
        assert!(html.contains("Region"), "should have Region row");
        assert!(
            html.contains("azure/eastus"),
            "should show full region string"
        );
    }

    #[test]
    fn render_server_info_no_region_row_when_absent() {
        let mut run = make_run();
        run.server_info = Some(make_host_info(Some("vm1"), "Ubuntu 22.04", None, None));
        let html = render(&run, None, None);
        // Region row only appears for the server card when region is set
        assert!(
            !html.contains("<dt>Region</dt>"),
            "no region row when region is absent"
        );
    }

    #[test]
    fn render_server_info_uptime_appears_when_set() {
        let mut run = make_run();
        let mut info = make_host_info(Some("srv"), "Linux", None, None);
        info.uptime_secs = Some(3661); // 1h 1m 1s
        run.server_info = Some(info);
        let html = render(&run, None, None);
        assert!(html.contains("Uptime"), "should show Uptime row");
        assert!(
            html.contains("1h 1m"),
            "should format uptime as hours/minutes"
        );
    }

    #[test]
    fn render_server_info_uptime_days_format() {
        let mut run = make_run();
        let mut info = make_host_info(Some("srv"), "Linux", None, None);
        info.uptime_secs = Some(86400 + 7200); // 1d 2h
        run.server_info = Some(info);
        let html = render(&run, None, None);
        assert!(
            html.contains("1d 2h"),
            "uptime >= 1 day should use day format"
        );
    }

    #[test]
    fn render_server_info_uptime_minutes_format() {
        let mut run = make_run();
        let mut info = make_host_info(Some("srv"), "Linux", None, None);
        info.uptime_secs = Some(130); // 2m 10s
        run.server_info = Some(info);
        let html = render(&run, None, None);
        assert!(
            html.contains("2m 10s"),
            "uptime < 1h should use minute format"
        );
    }

    #[test]
    fn render_shows_failed_attempts_in_err_class() {
        let mut run = make_run();
        run.attempts[0].success = false;
        run.attempts[0].http.as_mut().unwrap().status_code = 500;
        let html = render(&run, None, None);
        // Failed count should be > 0
        assert!(
            html.contains("row-err"),
            "failed attempt should have row-err class"
        );
    }

    #[test]
    fn render_zero_failures_shows_ok_class_for_failed_count() {
        let run = make_run();
        let html = render(&run, None, None);
        // With 0 failures the fail_cls should be "ok"
        assert!(
            html.contains(r#"class="ok">0<"#) || html.contains("0</dd>"),
            "zero failures should display with ok class or zero value"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // write_host_info_card — memory display
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn host_info_card_memory_mb_display() {
        let mut run = make_run();
        let mut info = make_host_info(Some("srv"), "Linux", None, None);
        info.total_memory_mb = Some(512);
        run.server_info = Some(info);
        let html = render(&run, None, None);
        assert!(html.contains("512 MB"), "small memory should show in MB");
    }

    #[test]
    fn host_info_card_memory_gb_display() {
        let mut run = make_run();
        let mut info = make_host_info(Some("srv"), "Linux", None, None);
        info.total_memory_mb = Some(8192); // 8 GiB
        run.server_info = Some(info);
        let html = render(&run, None, None);
        assert!(html.contains("8.0 GB"), "large memory should show in GB");
    }

    #[test]
    fn host_info_card_no_memory_shows_dash() {
        let mut run = make_run();
        let mut info = make_host_info(Some("srv"), "Linux", None, None);
        info.total_memory_mb = None;
        run.server_info = Some(info);
        let html = render(&run, None, None);
        // The memory line shows "—" when total_memory_mb is None
        assert!(
            html.contains("<dd>—</dd>"),
            "absent memory should show em dash"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // render_multi() — multi-target output structure
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn render_multi_single_run_delegates_to_render() {
        let run = make_run();
        let multi = render_multi(std::slice::from_ref(&run), None, None);
        let single = render(&run, None, None);
        // Single-run multi should produce identical output to render()
        assert_eq!(multi, single, "single-run render_multi must equal render");
    }

    #[test]
    fn render_multi_two_targets_shows_summary_table() {
        let r1 = make_run_with_url("https://target1.example.com/");
        let r2 = make_run_with_url("https://target2.example.com/");
        let html = render_multi(&[r1, r2], None, None);
        assert!(
            html.contains("Multi-Target Summary"),
            "must have summary table"
        );
        assert!(
            html.contains("target1.example.com"),
            "must show first target"
        );
        assert!(
            html.contains("target2.example.com"),
            "must show second target"
        );
    }

    #[test]
    fn render_multi_two_targets_shows_comparison_section() {
        let mut r1 = make_run_with_url("https://target1.example.com/");
        let mut r2 = make_run_with_url("https://target2.example.com/");
        // Add HTTP/1 attempts so the protocol comparison table appears
        r1.attempts.push(make_attempt(Protocol::Http1, true));
        r2.attempts.push(make_attempt(Protocol::Http1, true));
        let html = render_multi(&[r1, r2], None, None);
        assert!(
            html.contains("Cross-Target Protocol Comparison"),
            "must have comparison table"
        );
    }

    #[test]
    fn render_multi_shows_target_count_in_title() {
        let r1 = make_run_with_url("https://a.example.com/");
        let r2 = make_run_with_url("https://b.example.com/");
        let r3 = make_run_with_url("https://c.example.com/");
        let html = render_multi(&[r1, r2, r3], None, None);
        assert!(html.contains("3 targets compared"), "title must show count");
    }

    #[test]
    fn render_multi_per_target_details_have_open_attr_for_two_targets() {
        let r1 = make_run_with_url("https://a.example.com/");
        let r2 = make_run_with_url("https://b.example.com/");
        let html = render_multi(&[r1, r2], None, None);
        // For <= 2 runs each details element should be open
        assert!(
            html.contains("<details class=\"card multi-target-details\" open>"),
            "2-target runs should have open details"
        );
    }

    #[test]
    fn render_multi_three_targets_details_closed_by_default() {
        let r1 = make_run_with_url("https://a.example.com/");
        let r2 = make_run_with_url("https://b.example.com/");
        let r3 = make_run_with_url("https://c.example.com/");
        let html = render_multi(&[r1, r2, r3], None, None);
        // With 3 targets the details should NOT have the open attribute
        assert!(
            html.contains("<details class=\"card multi-target-details\">"),
            "3-target runs should have closed details by default"
        );
        assert!(
            !html.contains("<details class=\"card multi-target-details\" open>"),
            "3-target runs must not have open attribute"
        );
    }

    #[test]
    fn render_multi_target_baseline_rtt_shown_in_summary() {
        let mut r1 = make_run_with_url("https://a.example.com/");
        r1.baseline = Some(make_baseline(NetworkType::Internet, 30.5));
        let mut r2 = make_run_with_url("https://b.example.com/");
        r2.baseline = Some(make_baseline(NetworkType::Internet, 80.2));
        let html = render_multi(&[r1, r2], None, None);
        assert!(html.contains("30.50"), "should show r1 RTT avg");
        assert!(html.contains("80.20"), "should show r2 RTT avg");
    }

    #[test]
    fn render_multi_target_duration_shown_when_finished_at_set() {
        let mut r1 = make_run_with_url("https://a.example.com/");
        let now = Utc::now();
        r1.started_at = now;
        r1.finished_at = Some(now + chrono::Duration::milliseconds(2500));
        let r2 = make_run_with_url("https://b.example.com/");
        let html = render_multi(&[r1, r2], None, None);
        assert!(html.contains("2.50s"), "should show formatted duration");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Server name display logic in render_multi summary table
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn server_name_uses_hostname_when_present() {
        let mut run = make_run_with_url("https://target.example.com/");
        run.server_info = Some(make_host_info(Some("my-vm-01"), "Ubuntu 22.04", None, None));
        let html = render_multi(&[run, make_run_with_url("https://b.com/")], None, None);
        assert!(
            html.contains("my-vm-01"),
            "hostname should be used as display name"
        );
    }

    #[test]
    fn server_name_unknown_hostname_falls_back_to_provider_os() {
        let mut run = make_run_with_url("https://target.example.com/");
        run.server_info = Some(make_host_info(
            Some("unknown"),
            "Ubuntu 22.04 LTS",
            Some("azure/eastus"),
            None,
        ));
        let html = render_multi(&[run, make_run_with_url("https://b.com/")], None, None);
        // Should show "Azure Ubuntu" derived from region prefix and OS
        assert!(
            html.contains("Azure Ubuntu"),
            "unknown hostname should yield provider+OS name"
        );
    }

    #[test]
    fn server_name_empty_hostname_falls_back_to_provider_os() {
        let mut run = make_run_with_url("https://target.example.com/");
        run.server_info = Some(make_host_info(
            Some(""),
            "Windows Server 2022",
            Some("aws/us-east-1"),
            None,
        ));
        let html = render_multi(&[run, make_run_with_url("https://b.com/")], None, None);
        assert!(
            html.contains("AWS Windows"),
            "empty hostname should yield provider+OS name"
        );
    }

    #[test]
    fn server_name_no_server_info_shows_dash() {
        let run1 = make_run_with_url("https://target.example.com/");
        let run2 = make_run_with_url("https://b.com/");
        let html = render_multi(&[run1, run2], None, None);
        // No server_info → "—" in Server column
        assert!(
            html.contains("<td>—</td>"),
            "no server info should show em dash"
        );
    }

    #[test]
    fn server_name_gcp_region_detected() {
        let mut run = make_run_with_url("https://target.example.com/");
        run.server_info = Some(make_host_info(
            Some("unknown"),
            "Ubuntu 22.04",
            Some("gcp/us-central1"),
            None,
        ));
        let html = render_multi(&[run, make_run_with_url("https://b.com/")], None, None);
        assert!(
            html.contains("GCP Ubuntu"),
            "gcp/ prefix should map to GCP provider"
        );
    }

    #[test]
    fn server_name_no_provider_region_falls_back_to_os_type() {
        let mut run = make_run_with_url("https://target.example.com/");
        run.server_info = Some(make_host_info(Some(""), "Ubuntu 20.04", None, None));
        let html = render_multi(&[run, make_run_with_url("https://b.com/")], None, None);
        // No region → no provider → just "Ubuntu"
        assert!(
            html.contains(">Ubuntu<") || html.contains(">Ubuntu "),
            "no provider gives just OS type"
        );
    }

    #[test]
    fn server_name_windows_os_type_detected() {
        let mut run = make_run_with_url("https://target.example.com/");
        run.server_info = Some(make_host_info(Some(""), "Windows Server 2022", None, None));
        let html = render_multi(&[run, make_run_with_url("https://b.com/")], None, None);
        assert!(html.contains("Windows"), "Windows OS should be detected");
    }

    #[test]
    fn server_name_generic_linux_os_type() {
        let mut run = make_run_with_url("https://target.example.com/");
        run.server_info = Some(make_host_info(Some(""), "Debian GNU/Linux 11", None, None));
        let html = render_multi(&[run, make_run_with_url("https://b.com/")], None, None);
        // Not Windows, not Ubuntu → falls back to "Linux"
        assert!(
            html.contains("Linux"),
            "unknown distro should fall back to Linux"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Version badge rendering (server_version field)
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn version_badge_appears_in_multi_summary_when_set() {
        let mut run = make_run_with_url("https://target.example.com/");
        run.server_info = Some(make_host_info(
            Some("my-vm"),
            "Ubuntu 22.04",
            None,
            Some("0.13.2"),
        ));
        let html = render_multi(&[run, make_run_with_url("https://b.com/")], None, None);
        assert!(
            html.contains("<code>v0.13.2</code>"),
            "version badge must appear in summary"
        );
    }

    #[test]
    fn version_badge_absent_when_server_version_none() {
        let mut run = make_run_with_url("https://target.example.com/");
        run.server_info = Some(make_host_info(Some("my-vm"), "Ubuntu 22.04", None, None));
        let html = render_multi(&[run, make_run_with_url("https://b.com/")], None, None);
        // No server_version → no <code>v...
        assert!(
            !html.contains("<code>v"),
            "no version badge when server_version is None"
        );
    }

    #[test]
    fn version_badge_in_host_info_card_shows_version() {
        let mut run = make_run();
        run.server_info = Some(make_host_info(Some("srv"), "Linux", None, Some("1.2.3")));
        let html = render(&run, None, None);
        // In the Server Info card the version row shows the version string
        assert!(
            html.contains("1.2.3"),
            "server version must appear in single-target render"
        );
    }

    #[test]
    fn version_badge_dash_when_no_server_version_in_host_card() {
        let mut run = make_run();
        run.server_info = Some(make_host_info(Some("srv"), "Linux", None, None));
        let html = render(&run, None, None);
        // The version row shows "—" when server_version is None
        assert!(
            html.contains("—"),
            "absent server_version should show em dash in card"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Region display in server summary
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn region_shown_in_multi_summary_table_when_set() {
        let mut run = make_run_with_url("https://target.example.com/");
        run.server_info = Some(make_host_info(
            Some("vm"),
            "Ubuntu 22.04",
            Some("azure/westeurope"),
            None,
        ));
        let html = render_multi(&[run, make_run_with_url("https://b.com/")], None, None);
        assert!(
            html.contains("azure/westeurope"),
            "region should appear in summary table"
        );
    }

    #[test]
    fn region_absent_no_region_row_in_summary() {
        let mut run = make_run_with_url("https://target.example.com/");
        run.server_info = Some(make_host_info(Some("vm"), "Ubuntu 22.04", None, None));
        let html = render_multi(&[run, make_run_with_url("https://b.com/")], None, None);
        // The region small element only appears when region is Some
        assert!(
            !html.contains("Region: "),
            "no region marker when region is absent"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // LAN/Loopback detection and dimmed reference rendering
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn lan_target_shows_warn_badge_in_summary() {
        let mut r1 = make_run_with_url("https://a.example.com/");
        r1.baseline = Some(make_baseline(NetworkType::LAN, 1.0));
        let r2 = make_run_with_url("https://b.example.com/");
        let html = render_multi(&[r1, r2], None, None);
        assert!(
            html.contains(r#"<span class="warn">LAN</span>"#),
            "LAN network type should use warn class in summary"
        );
    }

    #[test]
    fn loopback_target_shows_ok_badge_in_summary() {
        let mut r1 = make_run_with_url("http://localhost/");
        r1.baseline = Some(make_baseline(NetworkType::Loopback, 0.05));
        let r2 = make_run_with_url("https://b.example.com/");
        let html = render_multi(&[r1, r2], None, None);
        assert!(
            html.contains(r#"<span class="ok">Loopback</span>"#),
            "Loopback network type should use ok class in summary"
        );
    }

    #[test]
    fn lan_target_shows_dimmed_ref_in_protocol_comparison() {
        let mut r_lan = make_run_with_url("http://192.168.1.100/");
        r_lan.baseline = Some(make_baseline(NetworkType::LAN, 0.5));
        let mut r_inet = make_run_with_url("https://remote.example.com/");
        r_inet.baseline = Some(make_baseline(NetworkType::Internet, 50.0));
        // Add Http1 attempts so the protocol comparison table shows data
        r_lan.attempts.push({
            let mut a = make_attempt(Protocol::Http1, true);
            a.http.as_mut().unwrap().total_duration_ms = 5.0;
            a
        });
        r_inet.attempts.push({
            let mut a = make_attempt(Protocol::Http1, true);
            a.http.as_mut().unwrap().total_duration_ms = 60.0;
            a
        });
        let html = render_multi(&[r_lan, r_inet], None, None);
        // LAN values should appear with opacity:.55 and (ref) label
        assert!(
            html.contains("opacity:.55"),
            "LAN target values should be dimmed"
        );
        assert!(html.contains("(ref)"), "LAN target should show (ref) label");
    }

    #[test]
    fn internet_targets_show_diff_percentage_vs_baseline() {
        let mut r1 = make_run_with_url("https://fast.example.com/");
        r1.baseline = Some(make_baseline(NetworkType::Internet, 20.0));
        let mut r2 = make_run_with_url("https://slow.example.com/");
        r2.baseline = Some(make_baseline(NetworkType::Internet, 80.0));
        r1.attempts.push({
            let mut a = make_attempt(Protocol::Http1, true);
            a.http.as_mut().unwrap().total_duration_ms = 50.0;
            a
        });
        r2.attempts.push({
            let mut a = make_attempt(Protocol::Http1, true);
            a.http.as_mut().unwrap().total_duration_ms = 200.0;
            a
        });
        let html = render_multi(&[r1, r2], None, None);
        // One target is baseline (no diff), the other gets a <span class="diff-..."> element.
        // Check for span element usage specifically (CSS also defines these classes as plain names).
        let diff_span_count = html.matches(r#"class="diff-fast""#).count()
            + html.matches(r#"class="diff-slow""#).count();
        assert!(
            diff_span_count > 0,
            "comparison table must show diff percentage spans"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Baseline rank-sum selection — best Internet target becomes reference
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn rank_sum_baseline_selects_fastest_internet_target() {
        // Two Internet targets. Target 1 is consistently faster.
        // The comparison table should show Target 1 as baseline (raw values only),
        // and Target 2 with diff percentages.
        let mut r1 = make_run_with_url("https://fast.example.com/");
        r1.baseline = Some(make_baseline(NetworkType::Internet, 10.0));
        let mut r2 = make_run_with_url("https://slow.example.com/");
        r2.baseline = Some(make_baseline(NetworkType::Internet, 100.0));
        // Give both Http1 and Tcp attempts to build a real rank sum
        r1.attempts.push({
            let mut a = make_attempt(Protocol::Http1, true);
            a.http.as_mut().unwrap().total_duration_ms = 20.0;
            a
        });
        r2.attempts.push({
            let mut a = make_attempt(Protocol::Http1, true);
            a.http.as_mut().unwrap().total_duration_ms = 200.0;
            a
        });
        let html = render_multi(&[r1, r2], None, None);
        // When two Internet targets exist, actual <span class="diff-..."> elements appear.
        // The CSS defines these classes once each; actual usage in table cells adds more occurrences.
        // Count span elements specifically to distinguish CSS definitions from actual use.
        let diff_span_count = html.matches(r#"class="diff-fast""#).count()
            + html.matches(r#"class="diff-slow""#).count();
        assert!(
            diff_span_count > 0,
            "at least one diff span element must appear when two Internet targets exist"
        );
    }

    #[test]
    fn rank_sum_with_single_internet_target_shows_no_diff() {
        // One Internet + one LAN. The LAN is reference; the Internet target has no Internet peer to diff against.
        let mut r_inet = make_run_with_url("https://remote.example.com/");
        r_inet.baseline = Some(make_baseline(NetworkType::Internet, 50.0));
        let mut r_lan = make_run_with_url("http://192.168.1.100/");
        r_lan.baseline = Some(make_baseline(NetworkType::LAN, 1.0));
        r_inet.attempts.push({
            let mut a = make_attempt(Protocol::Http1, true);
            a.http.as_mut().unwrap().total_duration_ms = 100.0;
            a
        });
        r_lan.attempts.push({
            let mut a = make_attempt(Protocol::Http1, true);
            a.http.as_mut().unwrap().total_duration_ms = 5.0;
            a
        });
        let html = render_multi(&[r_inet, r_lan], None, None);
        // With only one Internet target it is its own baseline — no <span class="diff-..."> elements
        // (CSS still defines .diff-fast and .diff-slow as plain class names, so we look for span elements)
        let diff_span_count = html.matches(r#"class="diff-fast""#).count()
            + html.matches(r#"class="diff-slow""#).count();
        assert!(
            diff_span_count == 0,
            "single internet target with one LAN should not produce diff span elements, got {diff_span_count}"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Cross-target observations text
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn cross_target_observations_mentions_fastest_protocol() {
        let mut r1 = make_run_with_url("https://fast.example.com/");
        r1.baseline = Some(make_baseline(NetworkType::Internet, 20.0));
        let mut r2 = make_run_with_url("https://slow.example.com/");
        r2.baseline = Some(make_baseline(NetworkType::Internet, 80.0));
        r1.attempts.push({
            let mut a = make_attempt(Protocol::Http1, true);
            a.http.as_mut().unwrap().total_duration_ms = 25.0;
            a
        });
        r2.attempts.push({
            let mut a = make_attempt(Protocol::Http1, true);
            a.http.as_mut().unwrap().total_duration_ms = 95.0;
            a
        });
        let html = render_multi(&[r1, r2], None, None);
        assert!(
            html.contains("fastest") || html.contains("faster"),
            "cross-target observations should mention fastest target"
        );
    }

    #[test]
    fn cross_target_rtt_observation_appears_when_rtts_differ() {
        let mut r1 = make_run_with_url("https://a.example.com/");
        r1.baseline = Some(make_baseline(NetworkType::Internet, 5.0));
        let mut r2 = make_run_with_url("https://b.example.com/");
        r2.baseline = Some(make_baseline(NetworkType::Internet, 100.0));
        // Need at least one protocol row to trigger the observations block
        r1.attempts.push({
            let mut a = make_attempt(Protocol::Http1, true);
            a.http.as_mut().unwrap().total_duration_ms = 20.0;
            a
        });
        r2.attempts.push({
            let mut a = make_attempt(Protocol::Http1, true);
            a.http.as_mut().unwrap().total_duration_ms = 200.0;
            a
        });
        let html = render_multi(&[r1, r2], None, None);
        assert!(
            html.contains("Baseline RTT"),
            "RTT observation should appear when RTTs differ"
        );
    }

    #[test]
    fn cross_target_mixed_network_observation_appears() {
        let mut r1 = make_run_with_url("https://a.example.com/");
        r1.baseline = Some(make_baseline(NetworkType::Internet, 50.0));
        let mut r2 = make_run_with_url("http://192.168.1.1/");
        r2.baseline = Some(make_baseline(NetworkType::LAN, 1.0));
        r1.attempts.push({
            let mut a = make_attempt(Protocol::Http1, true);
            a.http.as_mut().unwrap().total_duration_ms = 100.0;
            a
        });
        r2.attempts.push({
            let mut a = make_attempt(Protocol::Http1, true);
            a.http.as_mut().unwrap().total_duration_ms = 10.0;
            a
        });
        let html = render_multi(&[r1, r2], None, None);
        assert!(
            html.contains("Mixed network types"),
            "should note mixed network types"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Network type badge rendering in summary table
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn summary_table_dash_when_no_baseline() {
        let r1 = make_run_with_url("https://a.example.com/");
        let r2 = make_run_with_url("https://b.example.com/");
        let html = render_multi(&[r1, r2], None, None);
        // Without a baseline the Network column shows "—"
        assert!(
            html.contains("<td>—</td>"),
            "no baseline should show em dash for Network type"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Throughput protocol comparison in multi-target table
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn throughput_protocol_comparison_higher_is_better() {
        // For throughput protocols, the target with higher MB/s is "faster".
        let mut r1 = make_run_with_url("https://a.example.com/");
        r1.baseline = Some(make_baseline(NetworkType::Internet, 20.0));
        let mut r2 = make_run_with_url("https://b.example.com/");
        r2.baseline = Some(make_baseline(NetworkType::Internet, 20.0));

        let make_dl = |mbps: f64| -> RequestAttempt {
            let run_id = Uuid::new_v4();
            RequestAttempt {
                attempt_id: Uuid::new_v4(),
                run_id,
                protocol: Protocol::Download,
                sequence_num: 0,
                started_at: Utc::now(),
                finished_at: Some(Utc::now()),
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
                    total_duration_ms: 100.0,
                    redirect_count: 0,
                    started_at: Utc::now(),
                    response_headers: vec![],
                    payload_bytes: 1_048_576,
                    throughput_mbps: Some(mbps),
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
                http_stack: None,
            }
        };
        // r1 has 200 MB/s (better), r2 has 100 MB/s (worse)
        r1.attempts.push(make_dl(200.0));
        r2.attempts.push(make_dl(100.0));
        let html = render_multi(&[r1, r2], None, None);
        assert!(
            html.contains("Throughput MB/s"),
            "Download metric label must appear"
        );
        // r1 (200 MB/s) is the baseline — raw value shown.
        // r2 (100 MB/s) is worse → gets class="diff-slow" (negative throughput delta).
        // Look for span elements specifically (not just the CSS class definition).
        assert!(
            html.contains(r#"class="diff-slow""#),
            "lower-throughput target should produce a diff-slow span element"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Statistics summary section
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn statistics_summary_appears_with_multiple_attempts() {
        let mut run = make_run();
        run.attempts.clear();
        for i in 0..3 {
            let mut a = make_attempt(Protocol::Http1, true);
            a.sequence_num = i;
            a.http.as_mut().unwrap().total_duration_ms = 10.0 * (i as f64 + 1.0);
            run.attempts.push(a);
        }
        let html = render(&run, None, None);
        assert!(
            html.contains("Statistics Summary"),
            "should have Statistics Summary section"
        );
    }

    #[test]
    fn statistics_summary_shows_percentiles() {
        let mut run = make_run();
        run.attempts.clear();
        for i in 0..5 {
            let mut a = make_attempt(Protocol::Http1, true);
            a.sequence_num = i;
            a.http.as_mut().unwrap().total_duration_ms = 10.0 * (i as f64 + 1.0);
            run.attempts.push(a);
        }
        let html = render(&run, None, None);
        assert!(html.contains("p50"), "should show p50 column");
        assert!(html.contains("p95"), "should show p95 column");
        assert!(html.contains("p99"), "should show p99 column");
        assert!(html.contains("StdDev"), "should show StdDev column");
    }

    #[test]
    fn statistics_success_pct_100_uses_ok_class() {
        let mut run = make_run();
        run.attempts.clear();
        for i in 0..3 {
            let mut a = make_attempt(Protocol::Http1, true);
            a.sequence_num = i;
            run.attempts.push(a);
        }
        let html = render(&run, None, None);
        assert!(html.contains("100%"), "all succeeded → 100% should appear");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Page load section and protocol comparison
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn page_load_comparison_section_appears_with_multiple_protos() {
        let mut run = make_run();
        run.attempts.clear();
        for _ in 0..2 {
            run.attempts
                .push(make_page_load_attempt(Protocol::PageLoad, 120.0, false));
            run.attempts
                .push(make_page_load_attempt(Protocol::PageLoad2, 95.0, false));
        }
        let html = render(&run, None, None);
        assert!(
            html.contains("Protocol Comparison") && html.contains("Page Load"),
            "page load comparison section should appear"
        );
    }

    #[test]
    fn page_load_cold_warm_split_shown_when_both_present() {
        let mut run = make_run();
        run.attempts.clear();
        // Add cold and warm pageload2 attempts
        for _ in 0..2 {
            run.attempts
                .push(make_page_load_attempt(Protocol::PageLoad2, 150.0, false));
            run.attempts
                .push(make_page_load_attempt(Protocol::PageLoad2, 90.0, true));
        }
        let html = render(&run, None, None);
        assert!(
            html.contains("cold"),
            "cold subset should appear in page load table"
        );
        assert!(
            html.contains("warm"),
            "warm subset should appear in page load table"
        );
    }

    #[test]
    fn page_load_connection_reuse_observation_appears() {
        let mut run = make_run();
        run.attempts.clear();
        // Need cold and warm pageload2
        for _ in 0..2 {
            run.attempts
                .push(make_page_load_attempt(Protocol::PageLoad2, 200.0, false));
            run.attempts
                .push(make_page_load_attempt(Protocol::PageLoad2, 80.0, true));
        }
        let html = render(&run, None, None);
        // The analysis section should mention connection reuse savings
        // Chart sections are only rendered when both chart_browser and chart_pl are non-empty
        // but the comparison table always shows both subsets
        assert!(
            html.contains("cold") && html.contains("warm"),
            "both cold and warm labels must appear"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Browser protocol comparison and observations
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn browser_protocol_comparison_appears_with_multiple_browser_modes() {
        let mut run = make_run();
        run.attempts.clear();
        run.attempts
            .push(make_browser_attempt(Protocol::Browser1, 300.0, 50.0));
        run.attempts
            .push(make_browser_attempt(Protocol::Browser2, 250.0, 40.0));
        run.attempts
            .push(make_browser_attempt(Protocol::Browser3, 200.0, 35.0));
        let html = render(&run, None, None);
        assert!(
            html.contains("Protocol Comparison") && html.contains("Browser"),
            "browser comparison section should appear"
        );
        assert!(
            html.contains("browser1") || html.contains("Browser1"),
            "browser1 must appear"
        );
        assert!(
            html.contains("browser2") || html.contains("Browser2"),
            "browser2 must appear"
        );
        assert!(
            html.contains("browser3") || html.contains("Browser3"),
            "browser3 must appear"
        );
    }

    #[test]
    fn browser_results_section_shows_protocol_and_timings() {
        let mut run = make_run();
        run.attempts.clear();
        run.attempts
            .push(make_browser_attempt(Protocol::Browser, 355.5, 48.2));
        let html = render(&run, None, None);
        assert!(
            html.contains("Browser Results"),
            "must have Browser Results section"
        );
        assert!(
            html.contains("355.50") || html.contains("355.5"),
            "load_ms should appear"
        );
        assert!(
            html.contains("48.20") || html.contains("48.2"),
            "ttfb_ms should appear"
        );
    }

    #[test]
    fn charts_analysis_section_appears_with_pageload_data() {
        let mut run = make_run();
        run.attempts.clear();
        // We need >= 2 pageload attempts of the same protocol for charts
        for _ in 0..4 {
            run.attempts
                .push(make_page_load_attempt(Protocol::PageLoad2, 100.0, false));
        }
        for _ in 0..4 {
            run.attempts
                .push(make_browser_attempt(Protocol::Browser2, 120.0, 30.0));
        }
        let html = render(&run, None, None);
        assert!(
            html.contains("Charts"),
            "Charts &amp; Analysis section should appear"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // UDP statistics section
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn udp_statistics_section_appears_when_udp_attempts_present() {
        let run_id = Uuid::new_v4();
        let mut run = make_run();
        run.attempts.clear();
        run.attempts.push(RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id,
            protocol: Protocol::Udp,
            sequence_num: 0,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            success: true,
            dns: None,
            tcp: None,
            tls: None,
            http: None,
            udp: Some(UdpResult {
                remote_addr: "10.0.0.1:9000".into(),
                probe_count: 10,
                success_count: 10,
                loss_percent: 0.0,
                rtt_min_ms: 1.0,
                rtt_avg_ms: 1.5,
                rtt_p95_ms: 2.0,
                jitter_ms: 0.2,
                started_at: Utc::now(),
                probe_rtts_ms: vec![Some(1.5); 10],
            }),
            error: None,
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
            http_stack: None,
        });
        let html = render(&run, None, None);
        assert!(
            html.contains("UDP Probe Statistics"),
            "UDP section must appear"
        );
        assert!(html.contains("10.0.0.1:9000"), "remote addr should appear");
        assert!(html.contains("1.50"), "avg RTT should appear");
    }

    #[test]
    fn udp_loss_shows_warn_class_when_nonzero() {
        let run_id = Uuid::new_v4();
        let mut run = make_run();
        run.attempts.clear();
        run.attempts.push(RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id,
            protocol: Protocol::Udp,
            sequence_num: 0,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            success: true,
            dns: None,
            tcp: None,
            tls: None,
            http: None,
            udp: Some(UdpResult {
                remote_addr: "10.0.0.1:9000".into(),
                probe_count: 10,
                success_count: 8,
                loss_percent: 20.0,
                rtt_min_ms: 1.0,
                rtt_avg_ms: 1.5,
                rtt_p95_ms: 2.0,
                jitter_ms: 0.5,
                started_at: Utc::now(),
                probe_rtts_ms: vec![Some(1.5); 8],
            }),
            error: None,
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
            http_stack: None,
        });
        let html = render(&run, None, None);
        assert!(html.contains("20.0%"), "loss percent should appear");
        // nonzero loss uses "warn" class in the loss cell
        assert!(
            html.contains(r#"class="warn">20.0%"#),
            "loss cell should use warn class"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // TCP stats section
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn tcp_stats_section_appears_when_tcp_attempt_present() {
        let run = make_run();
        // The existing make_run() has a TCP result already
        let html = render(&run, None, None);
        assert!(
            html.contains("TCP Stats"),
            "TCP Stats section should appear"
        );
        assert!(html.contains("127.0.0.1:12345"), "local addr should appear");
        assert!(html.contains("127.0.0.1:80"), "remote addr should appear");
    }

    #[test]
    fn tcp_stats_ssthresh_shows_infinity_symbol_when_none() {
        let run = make_run(); // snd_ssthresh = None → "∞"
        let html = render(&run, None, None);
        // When snd_ssthresh is None the cell shows ∞
        assert!(html.contains("∞"), "None ssthresh should display ∞");
    }

    #[test]
    fn tcp_stats_congestion_algorithm_shown_when_set() {
        let mut run = make_run();
        if let Some(ref mut tcp) = run.attempts[0].tcp {
            tcp.congestion_algorithm = Some("cubic".into());
        }
        let html = render(&run, None, None);
        assert!(
            html.contains("cubic"),
            "congestion algorithm should appear in TCP stats"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Error section
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn error_section_shows_detail_in_title_attribute() {
        let mut run = make_run();
        run.attempts[0].success = false;
        run.attempts[0].error = Some(ErrorRecord {
            category: ErrorCategory::Tls,
            message: "TLS handshake failed".into(),
            detail: Some("certificate expired".into()),
            occurred_at: Utc::now(),
        });
        let html = render(&run, None, None);
        assert!(
            html.contains("TLS handshake failed"),
            "error message must appear"
        );
        assert!(
            html.contains("certificate expired"),
            "error detail must appear"
        );
        assert!(
            html.contains("class=\"err\""),
            "error category cell should use err class"
        );
    }

    #[test]
    fn error_section_no_detail_shows_dash() {
        let mut run = make_run();
        run.attempts[0].success = false;
        run.attempts[0].error = Some(ErrorRecord {
            category: ErrorCategory::Timeout,
            message: "deadline exceeded".into(),
            detail: None,
            occurred_at: Utc::now(),
        });
        let html = render(&run, None, None);
        assert!(html.contains("deadline exceeded"), "message must appear");
        assert!(html.contains("—"), "absent detail should show em dash");
    }

    #[test]
    fn error_section_html_escapes_message() {
        let mut run = make_run();
        run.attempts[0].success = false;
        run.attempts[0].error = Some(ErrorRecord {
            category: ErrorCategory::Other,
            message: "<script>evil()</script>".into(),
            detail: None,
            occurred_at: Utc::now(),
        });
        let html = render(&run, None, None);
        assert!(
            html.contains("&lt;script&gt;"),
            "error message must be HTML-escaped"
        );
        assert!(
            !html.contains("<script>evil"),
            "raw script tag must not appear in output"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Timing breakdown table
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn timing_table_shows_all_protocols_with_attempts() {
        let mut run = make_run();
        run.attempts.clear();
        run.attempts.push(make_attempt(Protocol::Http1, true));
        run.attempts.push(make_attempt(Protocol::Http2, true));
        let html = render(&run, None, None);
        assert!(
            html.contains("Timing Breakdown by Protocol"),
            "timing table must appear"
        );
        assert!(
            html.contains("<strong>http1</strong>"),
            "http1 row must appear"
        );
        assert!(
            html.contains("<strong>http2</strong>"),
            "http2 row must appear"
        );
    }

    #[test]
    fn timing_table_skips_protocols_with_no_attempts() {
        let run = make_run(); // only http1 attempts
        let html = render(&run, None, None);
        // http3 has no attempts — its row should not appear in the table
        assert!(
            !html.contains("<strong>http3</strong>"),
            "http3 row should not appear when no http3 attempts"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // render_multi with throughput metric labels
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn protocol_comparison_metric_label_correct_for_tcp() {
        let mut r1 = make_run_with_url("https://a.example.com/");
        let mut r2 = make_run_with_url("https://b.example.com/");
        let make_tcp = |ms: f64| -> RequestAttempt {
            let run_id = Uuid::new_v4();
            RequestAttempt {
                attempt_id: Uuid::new_v4(),
                run_id,
                protocol: Protocol::Tcp,
                sequence_num: 0,
                started_at: Utc::now(),
                finished_at: Some(Utc::now()),
                success: true,
                dns: None,
                tcp: Some(TcpResult {
                    local_addr: None,
                    remote_addr: "1.2.3.4:80".into(),
                    connect_duration_ms: ms,
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
                http: None,
                udp: None,
                error: None,
                retry_count: 0,
                server_timing: None,
                udp_throughput: None,
                page_load: None,
                browser: None,
                http_stack: None,
            }
        };
        r1.attempts.push(make_tcp(5.0));
        r2.attempts.push(make_tcp(15.0));
        let html = render_multi(&[r1, r2], None, None);
        assert!(
            html.contains("Connect ms"),
            "TCP metric label should be 'Connect ms'"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // SVG chart helpers — basic structural checks
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn svg_boxplot_empty_input_returns_empty_string() {
        let result = svg_boxplot("title", &[], "ms");
        assert!(
            result.is_empty(),
            "empty groups should produce empty string"
        );
    }

    #[test]
    fn svg_boxplot_too_few_values_skipped() {
        // Groups with < 2 values are skipped per the code
        let values = vec![1.0f64]; // only 1 point
        let result = svg_boxplot("title", &[("label", &values, "#red")], "ms");
        assert!(
            result.is_empty(),
            "fewer than 2 values should produce empty svg"
        );
    }

    #[test]
    fn svg_boxplot_valid_data_produces_svg_element() {
        let values = vec![10.0f64, 20.0, 30.0, 40.0, 50.0];
        let result = svg_boxplot("Test Chart", &[("label", &values, "#4e79a7")], "ms");
        assert!(result.starts_with("<svg"), "should produce an svg element");
        assert!(
            result.contains("Test Chart"),
            "chart title should appear in svg"
        );
        assert!(result.ends_with("</svg>"), "svg must be closed");
    }

    #[test]
    fn svg_boxplot_escapes_title() {
        let values = vec![10.0f64, 20.0, 30.0, 40.0, 50.0];
        let result = svg_boxplot(
            "Chart <with> special & chars",
            &[("label", &values, "#red")],
            "ms",
        );
        assert!(
            result.contains("&lt;with&gt;"),
            "title must be HTML-escaped"
        );
        assert!(
            result.contains("&amp;"),
            "ampersand in title must be escaped"
        );
    }

    #[test]
    fn svg_cdf_empty_input_returns_empty() {
        let result = svg_cdf("title", &[], "ms");
        assert!(result.is_empty(), "empty series produces empty string");
    }

    #[test]
    fn svg_cdf_single_value_series_skipped() {
        let values = vec![42.0f64];
        let result = svg_cdf("title", &[("s", &values, "#red")], "ms");
        assert!(
            result.is_empty(),
            "series with < 2 values should be skipped"
        );
    }

    #[test]
    fn svg_cdf_valid_data_produces_svg() {
        let values = vec![10.0f64, 20.0, 30.0, 40.0];
        let result = svg_cdf("CDF Chart", &[("series1", &values, "#4e79a7")], "ms");
        assert!(result.starts_with("<svg"), "should produce svg element");
        assert!(result.contains("CDF Chart"), "title must appear");
    }

    #[test]
    fn svg_hbar_valid_data_produces_svg() {
        let bars = vec![("item1", 100.0_f64), ("item2", 50.0_f64)];
        let colors = vec!["#4e79a7", "#e07b39"];
        let result = svg_hbar("Bar Chart", &bars, "ms", &colors);
        assert!(result.starts_with("<svg"), "should produce svg element");
        assert!(result.contains("Bar Chart"), "title must appear");
        assert!(result.contains("item1"), "bar labels must appear");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // HTML structure — footer timestamp
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn footer_contains_generator_info_and_timestamp() {
        let run = make_run();
        let html = render(&run, None, None);
        assert!(
            html.contains("networker-tester"),
            "footer should mention generator"
        );
        assert!(html.contains("UTC"), "footer should include UTC timestamp");
        assert!(html.contains("<footer>"), "footer element must be present");
    }

    #[test]
    fn render_multi_footer_uses_last_run_timestamp() {
        let r1 = make_run_with_url("https://a.example.com/");
        let r2 = make_run_with_url("https://b.example.com/");
        let html = render_multi(&[r1, r2], None, None);
        assert!(
            html.contains("<footer>"),
            "footer must appear in multi-target render"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // render_multi: target URL escaping
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn render_multi_escapes_target_url_in_summary() {
        let r1 = make_run_with_url("https://a.example.com/path?q=1&v=2");
        let r2 = make_run_with_url("https://b.example.com/");
        let html = render_multi(&[r1, r2], None, None);
        // The URL with & appears escaped in the HTML table
        assert!(
            html.contains("q=1&amp;v=2"),
            "& in URL must be HTML-escaped in table"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // All-attempts section open/closed behavior
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn all_attempts_section_open_when_twenty_or_fewer() {
        let mut run = make_run();
        run.attempts.clear();
        for i in 0..5 {
            let mut a = make_attempt(Protocol::Http1, true);
            a.sequence_num = i;
            run.attempts.push(a);
        }
        let html = render(&run, None, None);
        // With <= 20 attempts the details element gets the open attribute
        assert!(
            html.contains("<details open>")
                || html.contains("<details open ")
                || html.contains(" open>"),
            "few attempts should render details as open"
        );
    }

    #[test]
    fn all_attempts_section_closed_when_over_twenty() {
        let mut run = make_run();
        run.attempts.clear();
        for i in 0..25 {
            let mut a = make_attempt(Protocol::Http1, true);
            a.sequence_num = i;
            run.attempts.push(a);
        }
        let html = render(&run, None, None);
        assert!(
            html.contains("25 attempts"),
            "should show attempt count in summary"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Memory formatting boundary cases (already covered partially; add edge cases)
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn host_info_card_exactly_1024_mb_shows_gb() {
        let mut run = make_run();
        let mut info = make_host_info(Some("srv"), "Linux", None, None);
        info.total_memory_mb = Some(1024);
        run.server_info = Some(info);
        let html = render(&run, None, None);
        assert!(
            html.contains("1.0 GB"),
            "exactly 1024 MB should display as 1.0 GB"
        );
    }

    #[test]
    fn host_info_card_just_below_1024_mb_shows_mb() {
        let mut run = make_run();
        let mut info = make_host_info(Some("srv"), "Linux", None, None);
        info.total_memory_mb = Some(1023);
        run.server_info = Some(info);
        let html = render(&run, None, None);
        assert!(html.contains("1023 MB"), "1023 MB should display as MB");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // render: modes display
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn render_shows_all_modes_in_run_summary() {
        let mut run = make_run();
        run.modes = vec!["http1".into(), "http2".into(), "pageload".into()];
        let html = render(&run, None, None);
        assert!(
            html.contains("http1, http2, pageload"),
            "all modes should appear comma-separated"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // server_timing server version badge in run summary
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn run_summary_shows_server_version_from_server_timing() {
        let mut run = make_run();
        run.attempts[0].server_timing = Some(crate::metrics::ServerTimingResult {
            server_version: Some("0.13.2".into()),
            ..Default::default()
        });
        let html = render(&run, None, None);
        assert!(
            html.contains("0.13.2"),
            "server version from server_timing should appear in run summary"
        );
    }

    #[test]
    fn run_summary_shows_dash_when_no_server_version() {
        let run = make_run(); // no server_timing
        let html = render(&run, None, None);
        // The server_ver field defaults to "—"
        assert!(
            html.contains("<dd>—</dd>") || html.contains(">—<"),
            "no server version shows em dash"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Multi-target success/failure counts in summary table
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn render_multi_shows_success_and_failure_counts() {
        let mut r1 = make_run_with_url("https://a.example.com/");
        r1.attempts.push(make_attempt(Protocol::Http1, true));
        r1.attempts.push(make_attempt(Protocol::Http1, true));
        r1.attempts.push(make_attempt(Protocol::Http1, false));
        let r2 = make_run_with_url("https://b.example.com/");
        let html = render_multi(&[r1, r2], None, None);
        // r1: 3 attempts, 2 succeeded, 1 failed
        assert!(
            html.contains("<td class=\"ok\">2</td>"),
            "success count should appear with ok class"
        );
        assert!(
            html.contains("<td class=\"err\">1</td>"),
            "failure count should appear with err class"
        );
    }

    #[test]
    fn render_multi_failure_count_zero_uses_ok_class() {
        let mut r1 = make_run_with_url("https://a.example.com/");
        r1.attempts.push(make_attempt(Protocol::Http1, true));
        let r2 = make_run_with_url("https://b.example.com/");
        let html = render_multi(&[r1, r2], None, None);
        // 0 failures → fail_cls = "ok"
        assert!(
            html.contains("<td class=\"ok\">0</td>"),
            "zero failures should use ok class"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Cloud hostname detection & short names
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn is_cloud_internal_hostname_aws_ip_detected() {
        assert!(super::is_cloud_internal_hostname("ip-172-31-78-2"));
        assert!(super::is_cloud_internal_hostname("ip-10-0-1-50"));
    }

    #[test]
    fn is_cloud_internal_hostname_normal_not_detected() {
        assert!(!super::is_cloud_internal_hostname("my-vm-01"));
        assert!(!super::is_cloud_internal_hostname("web-server"));
        assert!(!super::is_cloud_internal_hostname("turing"));
    }

    #[test]
    fn is_cloud_internal_hostname_empty_and_unknown() {
        assert!(!super::is_cloud_internal_hostname(""));
        assert!(!super::is_cloud_internal_hostname("unknown"));
    }

    #[test]
    fn os_short_label_variants() {
        assert_eq!(super::os_short_label("Ubuntu 22.04 LTS"), "Ubuntu");
        assert_eq!(super::os_short_label("Windows Server 2022"), "Windows");
        assert_eq!(super::os_short_label("Debian GNU/Linux 11"), "Debian");
        assert_eq!(super::os_short_label("CentOS 8"), "Linux");
    }

    #[test]
    fn provider_from_region_detects_clouds() {
        assert_eq!(super::provider_from_region("azure/eastus"), Some("Azure"));
        assert_eq!(super::provider_from_region("aws/us-east-1"), Some("AWS"));
        assert_eq!(super::provider_from_region("gcp/us-central1"), Some("GCP"));
        assert_eq!(super::provider_from_region("on-prem/dc1"), None);
    }

    #[test]
    fn derive_display_name_aws_internal_hostname() {
        let info = make_host_info(
            Some("ip-172-31-78-2"),
            "Ubuntu 22.04 LTS",
            Some("aws/us-east-1"),
            None,
        );
        assert_eq!(
            super::derive_display_name(Some(&info), "fallback"),
            "AWS Ubuntu"
        );
    }

    #[test]
    fn derive_display_name_normal_hostname_kept() {
        let info = make_host_info(Some("my-vm"), "Ubuntu 22.04", None, None);
        assert_eq!(super::derive_display_name(Some(&info), "fallback"), "my-vm");
    }

    #[test]
    fn derive_display_name_none_uses_fallback() {
        assert_eq!(super::derive_display_name(None, "Target 1"), "Target 1");
    }

    #[test]
    fn derive_display_name_empty_hostname_with_gcp_windows() {
        let info = make_host_info(Some(""), "Windows Server 2022", Some("gcp/us-east1"), None);
        assert_eq!(
            super::derive_display_name(Some(&info), "fallback"),
            "GCP Windows"
        );
    }

    #[test]
    fn build_target_short_names_deduplicates() {
        let mut r1 = make_run_with_url("https://a.example.com/");
        r1.server_info = Some(make_host_info(
            Some("ip-172-31-1-1"),
            "Ubuntu 22.04",
            Some("aws/us-east-1"),
            None,
        ));
        let mut r2 = make_run_with_url("https://b.example.com/");
        r2.server_info = Some(make_host_info(
            Some("ip-172-31-2-2"),
            "Ubuntu 22.04",
            Some("aws/us-east-1"),
            None,
        ));
        let names = super::build_target_short_names(&[r1, r2]);
        assert_eq!(names[0], "AWS Ubuntu #1");
        assert_eq!(names[1], "AWS Ubuntu #2");
    }

    #[test]
    fn build_target_short_names_unique_no_suffix() {
        let mut r1 = make_run_with_url("https://a.example.com/");
        r1.server_info = Some(make_host_info(Some("turing"), "Ubuntu 22.04", None, None));
        let mut r2 = make_run_with_url("https://b.example.com/");
        r2.server_info = Some(make_host_info(
            Some("ip-172-31-1-1"),
            "Ubuntu 22.04",
            Some("aws/us-east-1"),
            None,
        ));
        let names = super::build_target_short_names(&[r1, r2]);
        assert_eq!(names[0], "turing");
        assert_eq!(names[1], "AWS Ubuntu");
    }

    #[test]
    fn render_multi_uses_short_names_in_cross_target_headers() {
        let mut r1 = make_run_with_url("https://10.0.0.1:8443/health");
        r1.server_info = Some(make_host_info(Some("turing"), "Ubuntu 22.04", None, None));
        r1.attempts.push(make_attempt(Protocol::Http1, true));
        let mut r2 = make_run_with_url("https://44.211.79.193:8443/health");
        r2.server_info = Some(make_host_info(
            Some("ip-172-31-78-2"),
            "Ubuntu 22.04 LTS",
            Some("aws/us-east-1"),
            None,
        ));
        r2.attempts.push(make_attempt(Protocol::Http1, true));
        let html = render_multi(&[r1, r2], None, None);
        // Cross-target headers should use short names, not full URLs
        assert!(
            html.contains("<th>turing</th>"),
            "expected short name 'turing' in header"
        );
        assert!(
            html.contains("<th>AWS Ubuntu</th>"),
            "expected short name 'AWS Ubuntu' in header"
        );
        // Full URLs should NOT be in the table headers
        assert!(
            !html.contains("<th>Target 1 <small>"),
            "should not have old 'Target N <small>URL' format"
        );
    }

    #[test]
    fn render_multi_aws_internal_hostname_shows_provider_name_in_summary() {
        let mut r1 = make_run_with_url("https://44.211.79.193:8443/health");
        r1.server_info = Some(make_host_info(
            Some("ip-172-31-78-2"),
            "Ubuntu 22.04 LTS",
            Some("aws/us-east-1"),
            None,
        ));
        r1.attempts.push(make_attempt(Protocol::Http1, true));
        let mut r2 = make_run_with_url("https://34.148.238.88:8443/health");
        r2.server_info = Some(make_host_info(
            Some(""),
            "Windows Server 2022",
            Some("gcp/us-east1"),
            None,
        ));
        r2.attempts.push(make_attempt(Protocol::Http1, true));
        let html = render_multi(&[r1, r2], None, None);
        // Summary table should show "AWS Ubuntu" not "ip-172-31-78-2" in display name
        assert!(
            html.contains("AWS Ubuntu"),
            "AWS internal hostname should be replaced"
        );
        // The internal hostname may still appear in the detailed host info card,
        // but the summary/header display names should use the provider+OS form.
        // Check the summary row uses the provider name
        assert!(
            html.contains("AWS Ubuntu<br>"),
            "summary display name should be provider+OS"
        );
    }

    #[test]
    fn render_stack_as_independent_section_when_stack_attempts_present() {
        let mut run = make_run();
        // Add default endpoint pageload attempts
        run.attempts
            .push(make_page_load_attempt(Protocol::PageLoad2, 150.0, false));
        run.attempts
            .push(make_page_load_attempt(Protocol::PageLoad2, 160.0, false));

        // Add nginx stack attempts
        let mut nginx1 = make_page_load_attempt(Protocol::PageLoad2, 120.0, false);
        nginx1.http_stack = Some("nginx".into());
        let mut nginx2 = make_page_load_attempt(Protocol::PageLoad2, 130.0, false);
        nginx2.http_stack = Some("nginx".into());
        run.attempts.push(nginx1);
        run.attempts.push(nginx2);

        let html = render_multi(&[run], None, None);
        // Should NOT have the old combined comparison table
        assert!(
            !html.contains("HTTP Stack Comparison"),
            "should not have combined comparison table"
        );
        // Should have independent stack section
        assert!(
            html.contains("NGINX Stack Results"),
            "should have independent nginx section"
        );
        // Endpoint data should appear in the main sections
        assert!(
            html.contains("Timing Breakdown by Protocol"),
            "should have endpoint timing section"
        );
    }

    #[test]
    fn render_no_stack_section_when_no_stack_attempts() {
        let mut run = make_run();
        run.attempts
            .push(make_page_load_attempt(Protocol::PageLoad2, 150.0, false));
        let html = render_multi(&[run], None, None);
        assert!(
            !html.contains("Stack Results"),
            "should not show stack section without stack attempts"
        );
    }
}
