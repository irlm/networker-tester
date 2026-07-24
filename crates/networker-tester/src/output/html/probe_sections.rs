//! Data-gated report cards for the measurement-depth probe modes
//! (rpm / ping / path / dualstack / websocket / pmtud) plus the per-record
//! DNS depth captured by the standalone `dns` mode.
//!
//! Every writer renders ONLY when an attempt of its type carries a result —
//! runs without that data produce byte-identical reports (enforced by
//! `tests/html_snapshot.rs`). The aggregation rules mirror the CLI summary
//! (`summary.rs`): averages skip absent values, fully-lost attempts' 0.0
//! sentinel RTTs are never averaged in (trust audit V11), and hop/MTU data
//! is never fabricated.

use super::*;

/// All six depth-probe cards, in a fixed order. Called once per run detail.
pub(super) fn write_probe_depth_sections(run: &TestRun, out: &mut String) {
    write_rpm_section(run, out);
    write_ping_section(run, out);
    write_path_section(run, out);
    write_dualstack_section(run, out);
    write_websocket_section(run, out);
    write_pmtud_section(run, out);
}

// ─────────────────────────────────────────────────────────────────────────────
// rpm — latency under load / bufferbloat
// ─────────────────────────────────────────────────────────────────────────────

fn write_rpm_section(run: &TestRun, out: &mut String) {
    let results: Vec<&crate::metrics::RpmResult> = run
        .attempts
        .iter()
        .filter(|a| a.protocol == Protocol::Rpm)
        .filter_map(|a| a.rpm.as_ref())
        .collect();
    if results.is_empty() {
        return;
    }

    let avg = |f: &dyn Fn(&crate::metrics::RpmResult) -> f64| -> f64 {
        results.iter().map(|r| f(r)).sum::<f64>() / results.len() as f64
    };
    let avg_opt = |f: &dyn Fn(&crate::metrics::RpmResult) -> Option<f64>| -> Option<f64> {
        let vals: Vec<f64> = results.iter().filter_map(|r| f(r)).collect();
        (!vals.is_empty()).then(|| vals.iter().sum::<f64>() / vals.len() as f64)
    };
    let loss_cls = |loss: f64| if loss > 0.0 { "warn" } else { "ok" };

    let rpm_cell = avg_opt(&|r| r.rpm)
        .map(|x| format!("<strong>{x:.0}</strong> round-trips/min"))
        .unwrap_or_else(|| "—".into());
    // Warn styling when the loaded/unloaded ratio exceeds 2x — the link
    // queues noticeably under load (bufferbloat).
    let factor_cell = match avg_opt(&|r| r.bufferbloat_factor) {
        Some(f) if f > 2.0 => {
            format!(r#"<span class="warn">{f:.2}x — latency inflates under load</span>"#)
        }
        Some(f) => format!(r#"<span class="ok">{f:.2}x</span>"#),
        None => "—".into(),
    };
    let load_cell = avg_opt(&|r| r.load_throughput_mbps)
        .map(|x| format!("{x:.2} MB/s"))
        .unwrap_or_else(|| "—".into());

    let uloss = avg(&|r| r.unloaded_loss_percent);
    let lloss = avg(&|r| r.loaded_loss_percent);
    let _ = write!(
        out,
        r#"
<section class="card">
  <h2>Latency Under Load (RPM)</h2>
  <table>
    <thead>
      <tr><th>Phase</th><th>Min RTT</th><th>Avg RTT</th><th>P95 RTT</th><th>Jitter</th><th>Loss %</th></tr>
    </thead>
    <tbody>
      <tr>
        <td>Unloaded</td>
        <td>{umin:.2}ms</td>
        <td>{uavg:.2}ms</td>
        <td>{up95:.2}ms</td>
        <td>{ujit:.2}ms</td>
        <td class="{uloss_cls}">{uloss:.1}%</td>
      </tr>
      <tr>
        <td>Loaded</td>
        <td>{lmin:.2}ms</td>
        <td>{lavg:.2}ms</td>
        <td>{lp95:.2}ms</td>
        <td>{ljit:.2}ms</td>
        <td class="{lloss_cls}">{lloss:.1}%</td>
      </tr>
    </tbody>
  </table>
  <dl class="summary-grid" style="margin-top:1rem">
    <dt>RPM</dt>                <dd>{rpm}</dd>
    <dt>Bufferbloat Factor</dt> <dd>{factor}</dd>
    <dt>Load Throughput</dt>    <dd>{load}</dd>
  </dl>
  <p class="note">Averaged over {n} rpm attempt(s). UDP echo probes while sustained downloads saturate the link; RPM = 60000 / loaded avg RTT (higher is better), factor = loaded avg / unloaded avg (1.0 &asymp; no bufferbloat).</p>
</section>
"#,
        umin = avg(&|r| r.unloaded_rtt_min_ms),
        uavg = avg(&|r| r.unloaded_rtt_avg_ms),
        up95 = avg(&|r| r.unloaded_rtt_p95_ms),
        ujit = avg(&|r| r.unloaded_jitter_ms),
        uloss_cls = loss_cls(uloss),
        uloss = uloss,
        lmin = avg(&|r| r.loaded_rtt_min_ms),
        lavg = avg(&|r| r.loaded_rtt_avg_ms),
        lp95 = avg(&|r| r.loaded_rtt_p95_ms),
        ljit = avg(&|r| r.loaded_jitter_ms),
        lloss_cls = loss_cls(lloss),
        lloss = lloss,
        rpm = rpm_cell,
        factor = factor_cell,
        load = load_cell,
        n = results.len(),
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// ping — ICMP echo statistics
// ─────────────────────────────────────────────────────────────────────────────

fn write_ping_section(run: &TestRun, out: &mut String) {
    let rows: Vec<&RequestAttempt> = run
        .attempts
        .iter()
        .filter(|a| a.protocol == Protocol::Ping && a.ping.is_some())
        .collect();
    if rows.is_empty() {
        return;
    }

    let _ = write!(
        out,
        r#"
<section class="card">
  <h2>ICMP Ping Statistics</h2>
  <table>
    <thead>
      <tr><th>Run #</th><th>Target</th><th>Sent</th><th>Recv</th><th>Loss %</th>
          <th>Min RTT</th><th>Avg RTT</th><th>P95 RTT</th><th>Jitter</th><th>Reply TTL</th></tr>
    </thead>
    <tbody>
"#
    );
    for a in &rows {
        let p = a.ping.as_ref().unwrap();
        // A fully-lost attempt's 0.0 aggregate RTTs are sentinels, not
        // measurements — render dashes instead (trust audit V11).
        let ms = |v: f64| -> String {
            if p.success_count > 0 {
                format!("{v:.2}ms")
            } else {
                "—".into()
            }
        };
        let _ = write!(
            out,
            r#"      <tr>
        <td>{seq}</td>
        <td><code>{addr}</code></td>
        <td>{sent}</td>
        <td>{recv}</td>
        <td class="{loss_cls}">{loss:.1}%</td>
        <td>{min}</td>
        <td>{avg}</td>
        <td>{p95}</td>
        <td>{jitter}</td>
        <td>{ttl}</td>
      </tr>
"#,
            seq = a.sequence_num,
            addr = escape_html(&p.remote_addr),
            sent = p.probe_count,
            recv = p.success_count,
            loss = p.loss_percent,
            loss_cls = if p.loss_percent > 0.0 { "warn" } else { "ok" },
            min = ms(p.rtt_min_ms),
            avg = ms(p.rtt_avg_ms),
            p95 = ms(p.rtt_p95_ms),
            jitter = ms(p.jitter_ms),
            ttl = p
                .reply_ttl
                .map(|t| t.to_string())
                .unwrap_or_else(|| "—".into()),
        );
    }
    let _ = writeln!(out, "    </tbody>\n  </table>\n</section>");
}

// ─────────────────────────────────────────────────────────────────────────────
// path — hop discovery
// ─────────────────────────────────────────────────────────────────────────────

fn write_path_section(run: &TestRun, out: &mut String) {
    // First attempt's trace, like the CLI summary — the path rarely changes
    // between attempts of one run; per-attempt data is in the JSON output.
    let Some(p) = run
        .attempts
        .iter()
        .filter(|a| a.protocol == Protocol::Path)
        .find_map(|a| a.path.as_ref())
    else {
        return;
    };

    let hop_count = p
        .hop_count
        .map(|h| h.to_string())
        .unwrap_or_else(|| "unknown".into());
    let dest = if p.destination_reached {
        let rtt = p
            .destination_rtt_ms
            .map(|r| format!(" ({r:.2}ms RTT)"))
            .unwrap_or_default();
        format!(r#"<span class="ok">reached</span>{rtt}"#)
    } else {
        r#"<span class="warn">NOT reached</span>"#.into()
    };
    let _ = write!(
        out,
        r#"
<section class="card">
  <h2>Network Path</h2>
  <dl class="summary-grid">
    <dt>Destination</dt> <dd><code>{addr}</code></dd>
    <dt>Method</dt>      <dd><code>{method}</code></dd>
    <dt>Hop Count</dt>   <dd>{hop_count}</dd>
    <dt>Destination</dt> <dd>{dest}</dd>
    <dt>Max TTL</dt>     <dd>{max_ttl}</dd>
  </dl>
"#,
        addr = escape_html(&p.remote_addr),
        method = escape_html(&p.method),
        hop_count = hop_count,
        dest = dest,
        max_ttl = p.max_ttl,
    );
    if p.hops.is_empty() {
        // Hops are NEVER fabricated — degraded platforms report only the
        // TTL-scan verdict above.
        let _ = writeln!(
            out,
            r#"  <p class="note">Hop addresses are not observable unprivileged on this platform — only the TTL-scan reachability verdict is reported.</p>"#
        );
    } else {
        let _ = write!(
            out,
            r#"  <table style="margin-top:1rem">
    <thead>
      <tr><th>Hop</th><th>Address</th><th>RTT (ms)</th></tr>
    </thead>
    <tbody>
"#
        );
        for hop in &p.hops {
            let _ = write!(
                out,
                r#"      <tr>
        <td>{idx}</td>
        <td>{addr}</td>
        <td>{rtt}</td>
      </tr>
"#,
                idx = hop.index,
                addr = hop
                    .addr
                    .as_deref()
                    .map(|a| format!("<code>{}</code>", escape_html(a)))
                    .unwrap_or_else(|| "—".into()),
                rtt = hop
                    .rtt_ms
                    .map(|r| format!("{r:.2}"))
                    .unwrap_or_else(|| "—".into()),
            );
        }
        let _ = writeln!(out, "    </tbody>\n  </table>");
    }
    let _ = writeln!(out, "</section>");
}

// ─────────────────────────────────────────────────────────────────────────────
// dualstack — IPv4 vs IPv6 comparison
// ─────────────────────────────────────────────────────────────────────────────

fn write_dualstack_section(run: &TestRun, out: &mut String) {
    let results: Vec<&crate::metrics::DualStackResult> = run
        .attempts
        .iter()
        .filter(|a| a.protocol == Protocol::DualStack)
        .filter_map(|a| a.dualstack.as_ref())
        .collect();
    if results.is_empty() {
        return;
    }

    let avg = |f: &dyn Fn(&crate::metrics::DualStackResult) -> Option<f64>| -> Option<f64> {
        let vals: Vec<f64> = results.iter().filter_map(|r| f(r)).collect();
        (!vals.is_empty()).then(|| vals.iter().sum::<f64>() / vals.len() as f64)
    };
    let fmt = |v: Option<f64>| v.map(|x| format!("{x:.2}ms")).unwrap_or_else(|| "—".into());
    let leg_status = |attempted: bool, success: bool| -> &'static str {
        if !attempted {
            r#"<span class="warn">not attempted</span>"#
        } else if success {
            r#"<span class="ok">ok</span>"#
        } else {
            r#"<span class="err">FAILED</span>"#
        }
    };
    let leg_addr = |addr: &Option<String>| {
        addr.as_deref()
            .map(|a| format!("<code>{}</code>", escape_html(a)))
            .unwrap_or_else(|| "—".into())
    };

    // Status/addresses/verdict come from the first result (stable across
    // attempts); phase timings are averaged.
    let first = results[0];
    let _ = write!(
        out,
        r#"
<section class="card">
  <h2>Dual-Stack (IPv4 vs IPv6)</h2>
  <table>
    <thead>
      <tr><th>Phase</th><th>IPv4</th><th>IPv6</th></tr>
    </thead>
    <tbody>
      <tr><td>Status</td><td>{s4}</td><td>{s6}</td></tr>
      <tr><td>Address</td><td>{a4}</td><td>{a6}</td></tr>
"#,
        s4 = leg_status(first.ipv4.attempted, first.ipv4.success),
        s6 = leg_status(first.ipv6.attempted, first.ipv6.success),
        a4 = leg_addr(&first.ipv4.addr),
        a6 = leg_addr(&first.ipv6.addr),
    );
    let mut phase_row =
        |label: &str,
         f4: &dyn Fn(&crate::metrics::DualStackResult) -> Option<f64>,
         f6: &dyn Fn(&crate::metrics::DualStackResult) -> Option<f64>| {
            let _ = writeln!(
                out,
                "      <tr><td>{label}</td><td>{v4}</td><td>{v6}</td></tr>",
                v4 = fmt(avg(f4)),
                v6 = fmt(avg(f6)),
            );
        };
    phase_row("DNS", &|r| r.ipv4.dns_ms, &|r| r.ipv6.dns_ms);
    phase_row("TCP", &|r| r.ipv4.tcp_ms, &|r| r.ipv6.tcp_ms);
    phase_row("TLS", &|r| r.ipv4.tls_ms, &|r| r.ipv6.tls_ms);
    phase_row("TTFB", &|r| r.ipv4.ttfb_ms, &|r| r.ipv6.ttfb_ms);
    phase_row("Total", &|r| r.ipv4.total_ms, &|r| r.ipv6.total_ms);

    let faster = match (&first.faster_family, avg(&|r| r.delta_ms)) {
        (Some(fam), Some(delta)) => format!(
            "<strong>{fam}</strong> by {delta:.2}ms avg",
            fam = escape_html(fam)
        ),
        _ => "no comparison (only one family completed)".into(),
    };
    let _ = write!(
        out,
        r#"    </tbody>
  </table>
  <dl class="summary-grid" style="margin-top:1rem">
    <dt>Faster Family</dt>  <dd>{faster}</dd>
    <dt>Happy Eyeballs</dt> <dd>{verdict} <small>(RFC 8305, {grace:.0}ms grace)</small></dd>
  </dl>
  <p class="note">Phase timings averaged over {n} dualstack attempt(s).</p>
</section>
"#,
        faster = faster,
        verdict = escape_html(&first.happy_eyeballs_verdict),
        grace = first.happy_eyeballs_grace_ms,
        n = results.len(),
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// websocket — upgrade + message RTT
// ─────────────────────────────────────────────────────────────────────────────

fn write_websocket_section(run: &TestRun, out: &mut String) {
    let rows: Vec<&RequestAttempt> = run
        .attempts
        .iter()
        .filter(|a| a.protocol == Protocol::WebSocket && a.websocket.is_some())
        .collect();
    if rows.is_empty() {
        return;
    }

    let _ = write!(
        out,
        r#"
<section class="card">
  <h2>WebSocket</h2>
  <table>
    <thead>
      <tr><th>Run #</th><th>Upgrade (ms)</th><th>Status</th><th>Msgs</th><th>Echoes</th>
          <th>Loss %</th><th>Min RTT</th><th>Avg RTT</th><th>P95 RTT</th><th>Jitter</th></tr>
    </thead>
    <tbody>
"#
    );
    for a in &rows {
        let w = a.websocket.as_ref().unwrap();
        // Message RTTs only when echoes actually arrived — a fully-lost
        // attempt's 0.0 sentinel is not a measurement (trust audit V11).
        let ms = |v: f64| -> String {
            if w.echo_count > 0 {
                format!("{v:.2}ms")
            } else {
                "—".into()
            }
        };
        let status = match w.upgrade_status {
            Some(101) => r#"<span class="ok">101</span>"#.to_string(),
            Some(s) => format!(r#"<span class="warn">{s}</span>"#),
            None => "—".into(),
        };
        let _ = write!(
            out,
            r#"      <tr>
        <td>{seq}</td>
        <td>{upgrade:.2}</td>
        <td>{status}</td>
        <td>{sent}</td>
        <td>{recv}</td>
        <td class="{loss_cls}">{loss:.1}%</td>
        <td>{min}</td>
        <td>{avg}</td>
        <td>{p95}</td>
        <td>{jitter}</td>
      </tr>
"#,
            seq = a.sequence_num,
            upgrade = w.upgrade_ms,
            status = status,
            sent = w.message_count,
            recv = w.echo_count,
            loss = w.loss_percent,
            loss_cls = if w.loss_percent > 0.0 { "warn" } else { "ok" },
            min = ms(w.msg_rtt_min_ms),
            avg = ms(w.msg_rtt_avg_ms),
            p95 = ms(w.msg_rtt_p95_ms),
            jitter = ms(w.jitter_ms),
        );
    }
    let first = rows[0].websocket.as_ref().unwrap();
    let _ = writeln!(
        out,
        r#"    </tbody>
  </table>
  <p class="note">URL <code>{url}</code> &middot; {payload} B echo payload &middot; message RTTs exclude the one-time DNS/TCP/TLS + upgrade cost (reported in their own sections).</p>
</section>"#,
        url = escape_html(&first.url),
        payload = first.payload_size,
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// pmtud — path MTU discovery
// ─────────────────────────────────────────────────────────────────────────────

fn write_pmtud_section(run: &TestRun, out: &mut String) {
    // First attempt's verdict, like the CLI summary — the path MTU rarely
    // changes between attempts of one run.
    let Some(p) = run
        .attempts
        .iter()
        .filter(|a| a.protocol == Protocol::Pmtud)
        .find_map(|a| a.pmtud.as_ref())
    else {
        return;
    };

    let mtu_cell = match (p.path_mtu, p.lower_bound_only) {
        (Some(mtu), false) => {
            let payload = p
                .max_unfragmented_payload
                .map(|v| format!(" (max unfragmented payload {v} + {h} header)", h = p.header_bytes))
                .unwrap_or_default();
            format!("<strong>{mtu} bytes</strong>{payload}")
        }
        (Some(mtu), true) => format!(
            r#"<strong>&ge;{mtu} bytes</strong> <span class="warn">lower bound only — search ceiling fit unfragmented, true MTU may be higher</span>"#
        ),
        (None, _) => {
            r#"<span class="warn">unknown</span> — no echo, no ICMP, no send errors (black hole or silent path)"#.into()
        }
    };
    let icmp = p
        .icmp_mtu
        .map(|m| format!("{m} bytes"))
        .unwrap_or_else(|| "—".into());
    let local = match p.local_mtu {
        Some(m) => {
            let note = match p.path_mtu {
                Some(pm) if pm < m => {
                    r#" <span class="warn">— path is narrower than the local link</span>"#
                }
                _ => "",
            };
            format!("{m} bytes{note}")
        }
        None => "—".into(),
    };
    let _ = write!(
        out,
        r#"
<section class="card">
  <h2>Path MTU Discovery</h2>
  <dl class="summary-grid">
    <dt>Destination</dt>       <dd><code>{addr}</code></dd>
    <dt>Path MTU</dt>          <dd>{mtu}</dd>
    <dt>Method</dt>            <dd><code>{method}</code></dd>
    <dt>ICMP Next-Hop MTU</dt> <dd>{icmp}</dd>
    <dt>Local Interface MTU</dt> <dd>{local}</dd>
    <dt>DF Probes Sent</dt>    <dd>{probes}</dd>
  </dl>
</section>
"#,
        addr = escape_html(&p.remote_addr),
        mtu = mtu_cell,
        method = escape_html(&p.method),
        icmp = icmp,
        local = local,
        probes = p.probes_sent,
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// dns depth — A/AAAA split timing, record counts, CNAME chain
// ─────────────────────────────────────────────────────────────────────────────

/// Rendered next to the TLS details card. Silent unless a `dns`-mode attempt
/// captured the per-record depth (older probes and all other modes leave the
/// fields unset), so pre-existing reports stay byte-identical.
pub(super) fn write_dns_depth_section(run: &TestRun, out: &mut String) {
    let dns: Vec<&crate::metrics::DnsResult> = run
        .attempts
        .iter()
        .filter(|a| a.protocol == Protocol::Dns)
        .filter_map(|a| a.dns.as_ref())
        .collect();
    let has_detail = dns
        .iter()
        .any(|d| d.a_ms.is_some() || d.aaaa_ms.is_some() || !d.cname_chain.is_empty());
    if !has_detail {
        return;
    }

    let avg = |f: &dyn Fn(&crate::metrics::DnsResult) -> Option<f64>| -> Option<f64> {
        let vals: Vec<f64> = dns.iter().filter_map(|d| f(d)).collect();
        (!vals.is_empty()).then(|| vals.iter().sum::<f64>() / vals.len() as f64)
    };
    // Record counts and the chain are stable across attempts — report the
    // first observation rather than a meaningless average.
    let a_count = dns.iter().find_map(|d| d.a_record_count);
    let aaaa_count = dns.iter().find_map(|d| d.aaaa_record_count);
    let chain = dns.iter().find(|d| !d.cname_chain.is_empty());

    let records = |n: Option<u32>| match n {
        Some(1) => "1 record".to_string(),
        Some(n) => format!("{n} records"),
        None => "skipped".to_string(),
    };
    let lookup = |ms: Option<f64>, count: Option<u32>| match ms {
        Some(v) => format!("{v:.2}ms avg &middot; {}", records(count)),
        None => format!("— &middot; {}", records(count)),
    };
    let chain_cell = match chain {
        Some(d) => format!(
            "<code>{}</code> &rarr; {}",
            escape_html(&d.query_name),
            d.cname_chain
                .iter()
                .map(|c| format!("<code>{}</code>", escape_html(c)))
                .collect::<Vec<_>>()
                .join(" &rarr; "),
        ),
        None => "none (resolves directly)".into(),
    };

    let _ = write!(
        out,
        r#"
<section class="card">
  <h2>DNS Depth</h2>
  <dl class="summary-grid">
    <dt>A Lookup</dt>    <dd>{a}</dd>
    <dt>AAAA Lookup</dt> <dd>{aaaa}</dd>
    <dt>CNAME Chain</dt> <dd>{chain}</dd>
  </dl>
  <p class="note">Per-record-type timing from the standalone <code>dns</code> probe mode (over {n} probe(s)).</p>
</section>
"#,
        a = lookup(avg(&|d| d.a_ms), a_count),
        aaaa = lookup(avg(&|d| d.aaaa_ms), aaaa_count),
        chain = chain_cell,
        n = dns.len(),
    );
}
