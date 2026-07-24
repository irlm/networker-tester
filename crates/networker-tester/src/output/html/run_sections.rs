//! Per-run content sections (page header + all data cards) rendered by
//! `write_run_sections` for both `render()` and `render_multi()`.

use super::*;

pub(super) fn write_run_sections(run: &TestRun, out: &mut String) {
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
                        recv = u
                            .datagrams_received
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "n/a".into()),
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
            // Prefer the post-transfer kernel snapshot (http.socket_stats,
            // gap #5) — cwnd/retrans/delivery-rate describe the transfer
            // there, not a fresh connection. Fall back to the connect-time
            // values on TcpResult (bare tcp probes, Windows, older JSON).
            let post = a.http.as_ref().and_then(|h| h.socket_stats.as_ref());
            let mss_bytes = post.and_then(|s| s.mss_bytes).or(t.mss_bytes);
            let rtt_estimate_ms = post.and_then(|s| s.rtt_estimate_ms).or(t.rtt_estimate_ms);
            let rtt_variance_ms = post.and_then(|s| s.rtt_variance_ms).or(t.rtt_variance_ms);
            let min_rtt_ms = post.and_then(|s| s.min_rtt_ms).or(t.min_rtt_ms);
            let snd_cwnd = post.and_then(|s| s.snd_cwnd).or(t.snd_cwnd);
            let snd_ssthresh = post.and_then(|s| s.snd_ssthresh).or(t.snd_ssthresh);
            let retransmits = post.and_then(|s| s.retransmits).or(t.retransmits);
            let total_retrans = post.and_then(|s| s.total_retrans).or(t.total_retrans);
            let rcv_space = post.and_then(|s| s.rcv_space).or(t.rcv_space);
            let segs_out = post.and_then(|s| s.segs_out).or(t.segs_out);
            let segs_in = post.and_then(|s| s.segs_in).or(t.segs_in);
            let delivery_rate_bps = post
                .and_then(|s| s.delivery_rate_bps)
                .or(t.delivery_rate_bps);
            let congestion_algorithm = post
                .and_then(|s| s.congestion_algorithm.as_deref())
                .or(t.congestion_algorithm.as_deref());
            let local_remote = format!(
                "{} → {}",
                t.local_addr.as_deref().unwrap_or("?"),
                t.remote_addr
            );
            let delivery_mbps = delivery_rate_bps
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
                mss = mss_bytes
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into()),
                rtt = rtt_estimate_ms
                    .map(|v| format!("{v:.3}"))
                    .unwrap_or_else(|| "—".into()),
                rttvar = rtt_variance_ms
                    .map(|v| format!("{v:.3}"))
                    .unwrap_or_else(|| "—".into()),
                minrtt = min_rtt_ms
                    .map(|v| format!("{v:.3}"))
                    .unwrap_or_else(|| "—".into()),
                cwnd = snd_cwnd
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into()),
                ssthresh = snd_ssthresh
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "∞".into()),
                retrans = retransmits
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "0".into()),
                total_retrans = total_retrans
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "0".into()),
                rcvwin = rcv_space
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into()),
                segsout = segs_out
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into()),
                segsin = segs_in.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
                delivery = delivery_mbps,
                cong = congestion_algorithm.unwrap_or("—"),
            );
        }
        // Note when any row carries post-transfer stats (gap #5): the same
        // columns mean different things at connect time vs after the transfer.
        let has_post = tcp_rows
            .iter()
            .any(|a| a.http.as_ref().is_some_and(|h| h.socket_stats.is_some()));
        if has_post {
            let _ = writeln!(
                out,
                r##"  <p class="note">HTTP-family rows show kernel stats sampled <em>after</em> the transfer completed (cwnd, retransmissions, and delivery rate describe the transfer); bare <code>tcp</code> rows are sampled at connect time.</p>"##
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
