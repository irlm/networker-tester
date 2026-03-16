/// Excel (.xlsx) report generation using `rust_xlsxwriter`.
///
/// One workbook per run with 10 worksheets:
///   1.  Summary        – run metadata
///   2.  Statistics     – per-protocol aggregate stats (min/mean/p50/p95/p99/max/stddev)
///   3.  HTTP Timings   – per-attempt HTTP phase times
///   4.  TCP Stats      – kernel-level socket metrics
///   5.  TLS Details    – handshake + certificate info
///   6.  UDP Stats      – loss / RTT / jitter
///   7.  Throughput     – download/upload bandwidth
///   8.  UDP Throughput – bulk UDP transfer metrics
///   9.  Server Timing  – server-side timing from X-Networker-* headers
///   10. Errors         – failed attempts
use crate::{
    capture::PacketCaptureSummary,
    metrics::{compute_stats, primary_metric_label, primary_metric_value, Protocol, TestRun},
};
use anyhow::Context;
use rust_xlsxwriter::{Format, Workbook};
use std::path::Path;

/// Write the full test run to `path` as an Excel workbook.
pub fn save(
    run: &TestRun,
    path: &Path,
    packet_capture: Option<&PacketCaptureSummary>,
) -> anyhow::Result<()> {
    let mut wb = Workbook::new();

    let bold = Format::new().set_bold();
    let num2 = Format::new().set_num_format("0.00");
    let num0 = Format::new().set_num_format("0");

    write_summary(&mut wb, run, &bold, packet_capture)?;
    write_statistics(&mut wb, run, &bold, &num2)?;
    write_http_timings(&mut wb, run, &bold, &num2)?;
    write_tcp_stats(&mut wb, run, &bold, &num2, &num0)?;
    write_tls_details(&mut wb, run, &bold, &num2)?;
    write_udp_stats(&mut wb, run, &bold, &num2)?;
    write_throughput(&mut wb, run, &bold, &num2)?;
    write_udp_throughput(&mut wb, run, &bold, &num2)?;
    write_server_timing(&mut wb, run, &bold, &num2)?;
    write_errors(&mut wb, run, &bold)?;

    wb.save(path).context("Failed to save Excel workbook")?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Sheet 1 – Summary
// ─────────────────────────────────────────────────────────────────────────────

fn write_summary(
    wb: &mut Workbook,
    run: &TestRun,
    bold: &Format,
    packet_capture: Option<&PacketCaptureSummary>,
) -> anyhow::Result<()> {
    let ws = wb.add_worksheet();
    ws.set_name("Summary")?;

    let headers = [
        "RunID",
        "Target",
        "Modes",
        "Runs",
        "Concurrency",
        "Timeout ms",
        "Success",
        "Fail",
        "Duration ms",
        "OS",
        "Version",
    ];
    for (col, h) in headers.iter().enumerate() {
        ws.write_with_format(0, col as u16, *h, bold)?;
    }

    let duration_ms = run
        .finished_at
        .map(|f| (f - run.started_at).num_milliseconds())
        .unwrap_or(0);

    ws.write(1, 0, run.run_id.to_string())?;
    ws.write(1, 1, run.target_url.as_str())?;
    ws.write(1, 2, run.modes.join(", "))?;
    ws.write(1, 3, run.total_runs as f64)?;
    ws.write(1, 4, run.concurrency as f64)?;
    ws.write(1, 5, run.timeout_ms as f64)?;
    ws.write(1, 6, run.success_count() as f64)?;
    ws.write(1, 7, run.failure_count() as f64)?;
    ws.write(1, 8, duration_ms as f64)?;
    ws.write(1, 9, run.client_os.as_str())?;
    ws.write(1, 10, run.client_version.as_str())?;

    if let Some(summary) = packet_capture {
        let base = 4u32;
        ws.write_with_format(base, 0, "Packet Capture", bold)?;
        ws.write(base + 1, 0, "Status")?;
        ws.write(base + 1, 1, summary.capture_status.as_str())?;
        ws.write(base + 2, 0, "Interface")?;
        ws.write(base + 2, 1, summary.interface.as_str())?;
        ws.write(base + 3, 0, "Total packets")?;
        ws.write(base + 3, 1, summary.total_packets as f64)?;
        ws.write(base + 4, 0, "Observed QUIC")?;
        ws.write(base + 4, 1, summary.observed_quic)?;
        ws.write(base + 5, 0, "Observed TCP only")?;
        ws.write(base + 5, 1, summary.observed_tcp_only)?;
        ws.write(base + 6, 0, "Observed mixed transport")?;
        ws.write(base + 6, 1, summary.observed_mixed_transport)?;
        ws.write(base + 7, 0, "Capture may be ambiguous")?;
        ws.write(base + 7, 1, summary.capture_may_be_ambiguous)?;
        ws.write(base + 8, 0, "Likely target packets")?;
        ws.write(base + 8, 1, summary.likely_target_packets as f64)?;
        ws.write(base + 9, 0, "Likely target %")?;
        ws.write(base + 9, 1, summary.likely_target_pct_of_total)?;
        ws.write(base + 10, 0, "Capture confidence")?;
        ws.write(base + 10, 1, summary.capture_confidence.as_str())?;
        ws.write(base + 11, 0, "Dominant trace port")?;
        if let Some(port) = summary.dominant_trace_port {
            ws.write(base + 11, 1, port as f64)?;
        }

        let mut row = base + 13;
        ws.write_with_format(row, 0, "Protocol shares", bold)?;
        row += 1;
        ws.write_with_format(row, 0, "Protocol", bold)?;
        ws.write_with_format(row, 1, "Packets", bold)?;
        ws.write_with_format(row, 2, "% of total", bold)?;
        row += 1;
        for share in &summary.transport_shares {
            ws.write(row, 0, share.protocol.as_str())?;
            ws.write(row, 1, share.packets as f64)?;
            ws.write(row, 2, share.pct_of_total)?;
            row += 1;
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Sheet 2 – Statistics
// ─────────────────────────────────────────────────────────────────────────────

fn write_statistics(
    wb: &mut Workbook,
    run: &TestRun,
    bold: &Format,
    num2: &Format,
) -> anyhow::Result<()> {
    let ws = wb.add_worksheet();
    ws.set_name("Statistics")?;

    let headers = [
        "Protocol",
        "Metric",
        "N",
        "Min",
        "Mean",
        "p50",
        "p95",
        "p99",
        "Max",
        "StdDev",
        "Success %",
    ];
    for (col, h) in headers.iter().enumerate() {
        ws.write_with_format(0, col as u16, *h, bold)?;
    }

    let all_protos = [
        Protocol::Http1,
        Protocol::Http2,
        Protocol::Http3,
        Protocol::Tcp,
        Protocol::Udp,
        Protocol::Download,
        Protocol::Upload,
        Protocol::WebDownload,
        Protocol::WebUpload,
        Protocol::UdpDownload,
        Protocol::UdpUpload,
    ];

    let mut row = 1u32;
    for proto in &all_protos {
        let attempts: Vec<_> = run
            .attempts
            .iter()
            .filter(|a| &a.protocol == proto)
            .collect();
        if attempts.is_empty() {
            continue;
        }
        let total = attempts.len();
        let success = attempts.iter().filter(|a| a.success).count();
        let success_pct = success as f64 / total as f64 * 100.0;
        let vals: Vec<f64> = attempts
            .iter()
            .filter_map(|a| primary_metric_value(a))
            .collect();
        let s = match compute_stats(&vals) {
            Some(s) => s,
            None => continue,
        };

        ws.write(row, 0, proto.to_string())?;
        ws.write(row, 1, primary_metric_label(proto))?;
        ws.write(row, 2, s.count as f64)?;
        ws.write_with_format(row, 3, s.min, num2)?;
        ws.write_with_format(row, 4, s.mean, num2)?;
        ws.write_with_format(row, 5, s.p50, num2)?;
        ws.write_with_format(row, 6, s.p95, num2)?;
        ws.write_with_format(row, 7, s.p99, num2)?;
        ws.write_with_format(row, 8, s.max, num2)?;
        ws.write_with_format(row, 9, s.stddev, num2)?;
        ws.write_with_format(row, 10, success_pct, num2)?;
        row += 1;
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Sheet 3 – HTTP Timings
// ─────────────────────────────────────────────────────────────────────────────

fn write_http_timings(
    wb: &mut Workbook,
    run: &TestRun,
    bold: &Format,
    num2: &Format,
) -> anyhow::Result<()> {
    let ws = wb.add_worksheet();
    ws.set_name("HTTP Timings")?;

    let headers = [
        "Seq", "Protocol", "Status", "Version", "DNS ms", "TCP ms", "TLS ms", "TTFB ms",
        "Total ms", "Success", "Retry#",
    ];
    for (col, h) in headers.iter().enumerate() {
        ws.write_with_format(0, col as u16, *h, bold)?;
    }

    let mut row = 1u32;
    for a in &run.attempts {
        if !matches!(
            a.protocol,
            Protocol::Http1 | Protocol::Http2 | Protocol::Http3 | Protocol::Tcp
        ) {
            continue;
        }
        let h = match &a.http {
            Some(h) => h,
            None => {
                // TCP-only or failed before HTTP
                ws.write(row, 0, a.sequence_num as f64)?;
                ws.write(row, 1, a.protocol.to_string())?;
                ws.write(row, 9, a.success)?;
                ws.write(row, 10, a.retry_count as f64)?;
                row += 1;
                continue;
            }
        };
        ws.write(row, 0, a.sequence_num as f64)?;
        ws.write(row, 1, a.protocol.to_string())?;
        ws.write(row, 2, h.status_code as f64)?;
        ws.write(row, 3, h.negotiated_version.as_str())?;
        if let Some(d) = &a.dns {
            ws.write_with_format(row, 4, d.duration_ms, num2)?;
        }
        if let Some(t) = &a.tcp {
            ws.write_with_format(row, 5, t.connect_duration_ms, num2)?;
        }
        if let Some(tls) = &a.tls {
            ws.write_with_format(row, 6, tls.handshake_duration_ms, num2)?;
        }
        ws.write_with_format(row, 7, h.ttfb_ms, num2)?;
        ws.write_with_format(row, 8, h.total_duration_ms, num2)?;
        ws.write(row, 9, a.success)?;
        ws.write(row, 10, a.retry_count as f64)?;
        row += 1;
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Sheet 3 – TCP Stats
// ─────────────────────────────────────────────────────────────────────────────

fn write_tcp_stats(
    wb: &mut Workbook,
    run: &TestRun,
    bold: &Format,
    num2: &Format,
    num0: &Format,
) -> anyhow::Result<()> {
    let ws = wb.add_worksheet();
    ws.set_name("TCP Stats")?;

    let headers = [
        "Seq",
        "Protocol",
        "Local Addr",
        "Remote Addr",
        "Connect ms",
        "MSS bytes",
        "RTT ms",
        "RTT Var ms",
        "Min RTT ms",
        "cwnd",
        "ssthresh",
        "Retransmits",
        "TotalRetrans",
        "rcv_space",
        "segs_out",
        "segs_in",
        "Delivery MB/s",
        "Congestion",
    ];
    for (col, h) in headers.iter().enumerate() {
        ws.write_with_format(0, col as u16, *h, bold)?;
    }

    let mut row = 1u32;
    for a in &run.attempts {
        let t = match &a.tcp {
            Some(t) => t,
            None => continue,
        };
        ws.write(row, 0, a.sequence_num as f64)?;
        ws.write(row, 1, a.protocol.to_string())?;
        ws.write(row, 2, t.local_addr.as_deref().unwrap_or(""))?;
        ws.write(row, 3, t.remote_addr.as_str())?;
        ws.write_with_format(row, 4, t.connect_duration_ms, num2)?;
        if let Some(v) = t.mss_bytes {
            ws.write_with_format(row, 5, v as f64, num0)?;
        }
        if let Some(v) = t.rtt_estimate_ms {
            ws.write_with_format(row, 6, v, num2)?;
        }
        if let Some(v) = t.rtt_variance_ms {
            ws.write_with_format(row, 7, v, num2)?;
        }
        if let Some(v) = t.min_rtt_ms {
            ws.write_with_format(row, 8, v, num2)?;
        }
        if let Some(v) = t.snd_cwnd {
            ws.write_with_format(row, 9, v as f64, num0)?;
        }
        if let Some(v) = t.snd_ssthresh {
            ws.write_with_format(row, 10, v as f64, num0)?;
        }
        if let Some(v) = t.retransmits {
            ws.write_with_format(row, 11, v as f64, num0)?;
        }
        if let Some(v) = t.total_retrans {
            ws.write_with_format(row, 12, v as f64, num0)?;
        }
        if let Some(v) = t.rcv_space {
            ws.write_with_format(row, 13, v as f64, num0)?;
        }
        if let Some(v) = t.segs_out {
            ws.write_with_format(row, 14, v as f64, num0)?;
        }
        if let Some(v) = t.segs_in {
            ws.write_with_format(row, 15, v as f64, num0)?;
        }
        if let Some(v) = t.delivery_rate_bps {
            ws.write_with_format(row, 16, v as f64 / 1_000_000.0, num2)?;
        }
        if let Some(v) = t.congestion_algorithm.as_deref() {
            ws.write(row, 17, v)?;
        }
        row += 1;
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Sheet 4 – TLS Details
// ─────────────────────────────────────────────────────────────────────────────

fn write_tls_details(
    wb: &mut Workbook,
    run: &TestRun,
    bold: &Format,
    num2: &Format,
) -> anyhow::Result<()> {
    let ws = wb.add_worksheet();
    ws.set_name("TLS Details")?;

    let headers = [
        "Seq",
        "Protocol",
        "TLS Version",
        "Cipher",
        "ALPN",
        "Cert Subject",
        "Cert Issuer",
        "Cert Expiry",
        "Handshake ms",
    ];
    for (col, h) in headers.iter().enumerate() {
        ws.write_with_format(0, col as u16, *h, bold)?;
    }

    let mut row = 1u32;
    for a in &run.attempts {
        let tls = match &a.tls {
            Some(t) => t,
            None => continue,
        };
        ws.write(row, 0, a.sequence_num as f64)?;
        ws.write(row, 1, a.protocol.to_string())?;
        ws.write(row, 2, tls.protocol_version.as_str())?;
        ws.write(row, 3, tls.cipher_suite.as_str())?;
        ws.write(row, 4, tls.alpn_negotiated.as_deref().unwrap_or(""))?;
        ws.write(row, 5, tls.cert_subject.as_deref().unwrap_or(""))?;
        ws.write(row, 6, tls.cert_issuer.as_deref().unwrap_or(""))?;
        if let Some(exp) = tls.cert_expiry {
            ws.write(row, 7, exp.format("%Y-%m-%d").to_string())?;
        }
        ws.write_with_format(row, 8, tls.handshake_duration_ms, num2)?;
        row += 1;
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Sheet 5 – UDP Stats
// ─────────────────────────────────────────────────────────────────────────────

fn write_udp_stats(
    wb: &mut Workbook,
    run: &TestRun,
    bold: &Format,
    num2: &Format,
) -> anyhow::Result<()> {
    let ws = wb.add_worksheet();
    ws.set_name("UDP Stats")?;

    let headers = [
        "Seq",
        "Remote Addr",
        "Probe Count",
        "Success",
        "Loss %",
        "RTT Min ms",
        "RTT Avg ms",
        "RTT p95 ms",
        "Jitter ms",
        "Retry#",
    ];
    for (col, h) in headers.iter().enumerate() {
        ws.write_with_format(0, col as u16, *h, bold)?;
    }

    let mut row = 1u32;
    for a in &run.attempts {
        let u = match &a.udp {
            Some(u) => u,
            None => continue,
        };
        ws.write(row, 0, a.sequence_num as f64)?;
        ws.write(row, 1, u.remote_addr.as_str())?;
        ws.write(row, 2, u.probe_count as f64)?;
        ws.write(row, 3, u.success_count as f64)?;
        ws.write_with_format(row, 4, u.loss_percent, num2)?;
        ws.write_with_format(row, 5, u.rtt_min_ms, num2)?;
        ws.write_with_format(row, 6, u.rtt_avg_ms, num2)?;
        ws.write_with_format(row, 7, u.rtt_p95_ms, num2)?;
        ws.write_with_format(row, 8, u.jitter_ms, num2)?;
        ws.write(row, 9, a.retry_count as f64)?;
        row += 1;
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Sheet 6 – Throughput
// ─────────────────────────────────────────────────────────────────────────────

fn write_throughput(
    wb: &mut Workbook,
    run: &TestRun,
    bold: &Format,
    num2: &Format,
) -> anyhow::Result<()> {
    let ws = wb.add_worksheet();
    ws.set_name("Throughput")?;

    let headers = [
        "Seq",
        "Mode",
        "Payload bytes",
        "Payload (human)",
        "Throughput MB/s",
        "TTFB ms",
        "Total ms",
        "Status",
        "Retry#",
    ];
    for (col, h) in headers.iter().enumerate() {
        ws.write_with_format(0, col as u16, *h, bold)?;
    }

    let mut row = 1u32;
    for a in &run.attempts {
        if !matches!(
            a.protocol,
            Protocol::Download | Protocol::Upload | Protocol::WebDownload | Protocol::WebUpload
        ) {
            continue;
        }
        let h = match &a.http {
            Some(h) => h,
            None => continue,
        };
        ws.write(row, 0, a.sequence_num as f64)?;
        ws.write(row, 1, a.protocol.to_string())?;
        ws.write(row, 2, h.payload_bytes as f64)?;
        ws.write(row, 3, format_bytes(h.payload_bytes))?;
        if let Some(mbps) = h.throughput_mbps {
            ws.write_with_format(row, 4, mbps, num2)?;
        }
        ws.write_with_format(row, 5, h.ttfb_ms, num2)?;
        ws.write_with_format(row, 6, h.total_duration_ms, num2)?;
        ws.write(row, 7, h.status_code as f64)?;
        ws.write(row, 8, a.retry_count as f64)?;
        row += 1;
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Sheet 7 – UDP Throughput
// ─────────────────────────────────────────────────────────────────────────────

fn write_udp_throughput(
    wb: &mut Workbook,
    run: &TestRun,
    bold: &Format,
    num2: &Format,
) -> anyhow::Result<()> {
    let ws = wb.add_worksheet();
    ws.set_name("UDP Throughput")?;

    let headers = [
        "Seq",
        "Mode",
        "Remote Addr",
        "Payload bytes",
        "Payload (human)",
        "Sent",
        "Received",
        "Loss %",
        "Throughput MB/s",
        "Transfer ms",
        "Bytes Acked",
        "Retry#",
    ];
    for (col, h) in headers.iter().enumerate() {
        ws.write_with_format(0, col as u16, *h, bold)?;
    }

    let mut row = 1u32;
    for a in &run.attempts {
        let u = match &a.udp_throughput {
            Some(u) => u,
            None => continue,
        };
        ws.write(row, 0, a.sequence_num as f64)?;
        ws.write(row, 1, a.protocol.to_string())?;
        ws.write(row, 2, u.remote_addr.as_str())?;
        ws.write(row, 3, u.payload_bytes as f64)?;
        ws.write(row, 4, format_bytes(u.payload_bytes))?;
        ws.write(row, 5, u.datagrams_sent as f64)?;
        ws.write(row, 6, u.datagrams_received as f64)?;
        ws.write_with_format(row, 7, u.loss_percent, num2)?;
        if let Some(mbps) = u.throughput_mbps {
            ws.write_with_format(row, 8, mbps, num2)?;
        }
        ws.write_with_format(row, 9, u.transfer_ms, num2)?;
        if let Some(acked) = u.bytes_acked {
            ws.write(row, 10, acked as f64)?;
        }
        ws.write(row, 11, a.retry_count as f64)?;
        row += 1;
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Sheet 8 – Server Timing
// ─────────────────────────────────────────────────────────────────────────────

fn write_server_timing(
    wb: &mut Workbook,
    run: &TestRun,
    bold: &Format,
    num2: &Format,
) -> anyhow::Result<()> {
    let ws = wb.add_worksheet();
    ws.set_name("Server Timing")?;

    let headers = [
        "Seq",
        "Protocol",
        "Request ID",
        "Server Timestamp",
        "Clock Skew ms",
        "Recv Body ms",
        "Processing ms",
        "Total Server ms",
    ];
    for (col, h) in headers.iter().enumerate() {
        ws.write_with_format(0, col as u16, *h, bold)?;
    }

    let mut row = 1u32;
    for a in &run.attempts {
        let st = match &a.server_timing {
            Some(st) => st,
            None => continue,
        };
        ws.write(row, 0, a.sequence_num as f64)?;
        ws.write(row, 1, a.protocol.to_string())?;
        ws.write(row, 2, st.request_id.as_deref().unwrap_or(""))?;
        if let Some(ts) = st.server_timestamp {
            ws.write(row, 3, ts.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())?;
        }
        if let Some(v) = st.clock_skew_ms {
            ws.write_with_format(row, 4, v, num2)?;
        }
        if let Some(v) = st.recv_body_ms {
            ws.write_with_format(row, 5, v, num2)?;
        }
        if let Some(v) = st.processing_ms {
            ws.write_with_format(row, 6, v, num2)?;
        }
        if let Some(v) = st.total_server_ms {
            ws.write_with_format(row, 7, v, num2)?;
        }
        row += 1;
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Sheet 8 – Errors
// ─────────────────────────────────────────────────────────────────────────────

fn write_errors(wb: &mut Workbook, run: &TestRun, bold: &Format) -> anyhow::Result<()> {
    let ws = wb.add_worksheet();
    ws.set_name("Errors")?;

    let headers = ["Seq", "Protocol", "Category", "Message", "Detail", "Time"];
    for (col, h) in headers.iter().enumerate() {
        ws.write_with_format(0, col as u16, *h, bold)?;
    }

    let mut row = 1u32;
    for a in &run.attempts {
        let e = match &a.error {
            Some(e) => e,
            None => continue,
        };
        ws.write(row, 0, a.sequence_num as f64)?;
        ws.write(row, 1, a.protocol.to_string())?;
        ws.write(row, 2, e.category.to_string())?;
        ws.write(row, 3, e.message.as_str())?;
        ws.write(row, 4, e.detail.as_deref().unwrap_or(""))?;
        ws.write(
            row,
            5,
            e.occurred_at.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        )?;
        row += 1;
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        capture::{EndpointPacketCount, PacketShare, PortPacketCount},
        metrics::*,
    };
    use chrono::Utc;
    use tempfile::NamedTempFile;
    use uuid::Uuid;

    #[test]
    fn format_bytes_values() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(65536), "64.0 KiB");
        assert_eq!(format_bytes(1 << 20), "1.0 MiB");
        assert_eq!(format_bytes(1 << 30), "1.0 GiB");
    }

    /// Build a TestRun that touches every sheet: HTTP, TCP, TLS, UDP, throughput,
    /// UDP throughput, server timing, page-load, and errors.
    fn make_full_run() -> TestRun {
        let run_id = Uuid::new_v4();
        let now = Utc::now();

        let http_attempt = RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id,
            protocol: Protocol::Http1,
            sequence_num: 0,
            started_at: now,
            finished_at: Some(now),
            success: true,
            dns: Some(DnsResult {
                query_name: "localhost".into(),
                resolved_ips: vec!["127.0.0.1".into()],
                duration_ms: 0.5,
                started_at: now,
                success: true,
            }),
            tcp: Some(TcpResult {
                local_addr: Some("127.0.0.1:54321".into()),
                remote_addr: "127.0.0.1:8080".into(),
                connect_duration_ms: 1.2,
                attempt_count: 1,
                started_at: now,
                success: true,
                mss_bytes: Some(1460),
                rtt_estimate_ms: Some(0.3),
                retransmits: Some(0),
                total_retrans: Some(0),
                snd_cwnd: Some(10),
                snd_ssthresh: None,
                rtt_variance_ms: Some(0.05),
                rcv_space: Some(65536),
                segs_out: Some(5),
                segs_in: Some(4),
                congestion_algorithm: Some("cubic".into()),
                delivery_rate_bps: Some(1_000_000),
                min_rtt_ms: Some(0.2),
            }),
            tls: Some(TlsResult {
                protocol_version: "TLSv1.3".into(),
                cipher_suite: "TLS_AES_256_GCM_SHA384".into(),
                alpn_negotiated: Some("http/1.1".into()),
                cert_subject: Some("CN=localhost".into()),
                cert_issuer: Some("CN=Test CA".into()),
                cert_expiry: Some(now),
                handshake_duration_ms: 4.2,
                started_at: now,
                success: true,
                cert_chain: vec![CertEntry {
                    subject: "CN=localhost".into(),
                    issuer: "CN=Test CA".into(),
                    expiry: Some(now),
                    sans: vec!["localhost".into()],
                }],
                tls_backend: Some("rustls".into()),
            }),
            http: Some(HttpResult {
                negotiated_version: "HTTP/1.1".into(),
                status_code: 200,
                headers_size_bytes: 120,
                body_size_bytes: 42,
                ttfb_ms: 3.1,
                total_duration_ms: 8.4,
                redirect_count: 0,
                started_at: now,
                response_headers: vec![("content-type".into(), "application/json".into())],
                payload_bytes: 65536,
                throughput_mbps: Some(105.2),
                goodput_mbps: Some(98.1),
                cpu_time_ms: Some(1.4),
                csw_voluntary: Some(3),
                csw_involuntary: Some(0),
            }),
            udp: None,
            error: None,
            retry_count: 0,
            server_timing: Some(ServerTimingResult {
                request_id: Some("req-1".into()),
                server_timestamp: Some(now),
                clock_skew_ms: Some(0.5),
                recv_body_ms: None,
                processing_ms: Some(2.1),
                total_server_ms: Some(2.3),
                server_version: Some("0.11.3".into()),
                srv_csw_voluntary: Some(1),
                srv_csw_involuntary: Some(0),
            }),
            udp_throughput: None,
            page_load: None,
            browser: None,
            http_stack: None,
        };

        let udp_attempt = RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id,
            protocol: Protocol::Udp,
            sequence_num: 1,
            started_at: now,
            finished_at: Some(now),
            success: true,
            dns: None,
            tcp: None,
            tls: None,
            http: None,
            udp: Some(UdpResult {
                remote_addr: "127.0.0.1:9999".into(),
                probe_count: 10,
                success_count: 10,
                loss_percent: 0.0,
                rtt_min_ms: 0.1,
                rtt_avg_ms: 0.2,
                rtt_p95_ms: 0.3,
                jitter_ms: 0.05,
                started_at: now,
                probe_rtts_ms: vec![Some(0.2); 10],
            }),
            error: None,
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
            http_stack: None,
        };

        let udp_throughput_attempt = RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id,
            protocol: Protocol::UdpDownload,
            sequence_num: 2,
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
            udp_throughput: Some(UdpThroughputResult {
                remote_addr: "127.0.0.1:9998".into(),
                payload_bytes: 1_048_576,
                datagrams_sent: 800,
                datagrams_received: 798,
                bytes_acked: Some(1_048_576),
                loss_percent: 0.25,
                transfer_ms: 85.0,
                throughput_mbps: Some(98.6),
                started_at: now,
            }),
            page_load: None,
            browser: None,
            http_stack: None,
        };

        let pageload_attempt = RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id,
            protocol: Protocol::PageLoad,
            sequence_num: 3,
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
            page_load: Some(PageLoadResult {
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
        };

        let error_attempt = RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id,
            protocol: Protocol::Http1,
            sequence_num: 4,
            started_at: now,
            finished_at: Some(now),
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
                occurred_at: now,
            }),
            retry_count: 1,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
            http_stack: None,
        };

        TestRun {
            run_id,
            started_at: now,
            finished_at: Some(now),
            target_url: "http://localhost:8080/health".into(),
            target_host: "localhost".into(),
            modes: vec![
                "http1".into(),
                "udp".into(),
                "udpdownload".into(),
                "pageload".into(),
            ],
            total_runs: 5,
            concurrency: 1,
            timeout_ms: 5000,
            client_os: "linux".into(),
            client_version: "0.11.3".into(),
            server_info: None,
            client_info: None,
            baseline: None,
            attempts: vec![
                http_attempt,
                udp_attempt,
                udp_throughput_attempt,
                pageload_attempt,
                error_attempt,
            ],
        }
    }

    #[test]
    fn save_writes_xlsx_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let run = make_full_run();
        save(&run, tmp.path(), None).unwrap();
        // File must be non-empty (valid xlsx is at least a few KB)
        let metadata = std::fs::metadata(tmp.path()).unwrap();
        assert!(
            metadata.len() > 1024,
            "xlsx file too small: {} bytes",
            metadata.len()
        );
    }

    #[test]
    fn save_empty_run_does_not_panic() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let run_id = Uuid::new_v4();
        let run = TestRun {
            run_id,
            started_at: Utc::now(),
            finished_at: None,
            target_url: "http://localhost/health".into(),
            target_host: "localhost".into(),
            modes: vec![],
            total_runs: 0,
            concurrency: 1,
            timeout_ms: 5000,
            client_os: "test".into(),
            client_version: "0.0.0".into(),
            server_info: None,
            client_info: None,
            baseline: None,
            attempts: vec![],
        };
        save(&run, tmp.path(), None).unwrap();
    }

    fn sample_packet_capture_summary() -> PacketCaptureSummary {
        PacketCaptureSummary {
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
            transport_shares: vec![PacketShare {
                protocol: "udp".into(),
                packets: 20,
                pct_of_total: 47.6,
            }],
            top_endpoints: vec![EndpointPacketCount {
                endpoint: "127.0.0.1".into(),
                packets: 20,
            }],
            top_ports: vec![PortPacketCount {
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
    fn save_with_packet_capture_summary_does_not_panic() {
        let tmp = NamedTempFile::new().unwrap();
        let run = make_full_run();
        save(&run, tmp.path(), Some(&sample_packet_capture_summary())).unwrap();
        let metadata = std::fs::metadata(tmp.path()).unwrap();
        assert!(metadata.len() > 1000);
    }

    #[test]
    fn save_with_throughput_attempts_does_not_panic() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let run_id = Uuid::new_v4();
        let now = Utc::now();

        let download_attempt = RequestAttempt {
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
                csw_voluntary: Some(20),
                csw_involuntary: Some(1),
            }),
            udp: None,
            error: None,
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
            http_stack: None,
        };

        let upload_attempt = RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id,
            protocol: Protocol::Upload,
            sequence_num: 1,
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
                body_size_bytes: 0,
                ttfb_ms: 90.0,
                total_duration_ms: 90.0,
                redirect_count: 0,
                started_at: now,
                response_headers: vec![],
                payload_bytes: 1_048_576,
                throughput_mbps: Some(110.0),
                goodput_mbps: Some(102.0),
                cpu_time_ms: Some(15.0),
                csw_voluntary: Some(25),
                csw_involuntary: Some(0),
            }),
            udp: None,
            error: None,
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
            http_stack: None,
        };

        let run = TestRun {
            run_id,
            started_at: now,
            finished_at: Some(now),
            target_url: "http://localhost:8080/health".into(),
            target_host: "localhost".into(),
            modes: vec!["download".into(), "upload".into()],
            total_runs: 1,
            concurrency: 1,
            timeout_ms: 5000,
            client_os: "test".into(),
            client_version: "0.0.0".into(),
            server_info: None,
            client_info: None,
            baseline: None,
            attempts: vec![download_attempt, upload_attempt],
        };

        save(&run, tmp.path(), None).unwrap();
        let metadata = std::fs::metadata(tmp.path()).unwrap();
        assert!(
            metadata.len() > 1000,
            "xlsx with throughput data must be non-empty"
        );
    }
}
