//! Row renderers (protocol summary rows, attempt rows), byte formatting,
//! the packet-capture section, and HTML escaping.

use super::*;

// ─────────────────────────────────────────────────────────────────────────────
// Row renderers
// ─────────────────────────────────────────────────────────────────────────────

pub(super) fn append_proto_row(out: &mut String, proto: &Protocol, rows: &[&RequestAttempt]) {
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

pub(super) fn append_attempt_row(out: &mut String, a: &RequestAttempt, show_stack: bool) {
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

pub(super) fn format_bytes(n: usize) -> String {
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

pub(super) fn write_packet_capture_section(
    packet_capture: Option<&PacketCaptureSummary>,
    out: &mut String,
) {
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

pub(super) fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
