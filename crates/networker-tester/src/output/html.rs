/// Generate a self-contained HTML diagnostic report from a `TestRun`.
///
/// The report embeds a minimal inline CSS for offline viewing and optionally
/// adds a `<link rel="stylesheet">` for the external `report.css` file so
/// operators can customise the look without editing generated HTML.
use crate::metrics::{Protocol, RequestAttempt, TestRun};
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
    <dt>Client version</dt>  <dd>{ver}</dd>
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
        ver = escape_html(&run.client_version),
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
        Protocol::Tcp,
        Protocol::Udp,
        Protocol::Download,
        Protocol::Upload,
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

    // ── Throughput results ────────────────────────────────────────────────────
    let throughput_rows: Vec<&RequestAttempt> = run
        .attempts
        .iter()
        .filter(|a| matches!(a.protocol, Protocol::Download | Protocol::Upload) && a.http.is_some())
        .collect();
    if !throughput_rows.is_empty() {
        let _ = write!(
            out,
            r#"
<section class="card">
  <h2>Throughput Results</h2>
  <table>
    <thead>
      <tr><th>Run #</th><th>Mode</th><th>Payload</th><th>Throughput (MB/s)</th>
          <th>TTFB (ms)</th><th>Total (ms)</th><th>Status</th></tr>
    </thead>
    <tbody>
"#
        );
        for a in &throughput_rows {
            let h = a.http.as_ref().unwrap();
            let throughput = h
                .throughput_mbps
                .map(|m| format!("{m:.2}"))
                .unwrap_or_else(|| "—".into());
            let status_cell = {
                let cls = if h.status_code < 400 { "ok" } else { "err" };
                format!(r#"<span class="{cls}">{}</span>"#, h.status_code)
            };
            let _ = write!(
                out,
                r#"      <tr>
        <td>{seq}</td>
        <td>{proto}</td>
        <td>{payload}</td>
        <td class="{thr_cls}">{thr}</td>
        <td>{ttfb:.2}</td>
        <td>{total:.2}</td>
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
                ttfb = h.ttfb_ms,
                total = h.total_duration_ms,
                status = status_cell,
            );
        }
        let _ = writeln!(out, "    </tbody>\n  </table>\n</section>");
    }

    // ── Individual attempts ───────────────────────────────────────────────────
    let _ = write!(
        out,
        r#"
<section class="card">
  <h2>All Attempts</h2>
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
"#
    );

    for a in &run.attempts {
        append_attempt_row(&mut out, a);
    }
    let _ = writeln!(out, "    </tbody>\n  </table>\n</section>");

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
            Protocol::Download | Protocol::Upload => {
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
"#;

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{HttpResult, Protocol, RequestAttempt, TcpResult, TestRun};
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
                }),
                udp: None,
                error: None,
                retry_count: 0,
                server_timing: None,
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
    fn save_writes_html_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let run = make_run();
        save(&run, tmp.path(), None).unwrap();
        let content = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(content.starts_with("<!DOCTYPE html>"));
    }
}
