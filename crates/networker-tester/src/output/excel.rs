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
use crate::metrics::{
    compute_stats, primary_metric_label, primary_metric_value, Protocol, TestRun,
};
use anyhow::Context;
use rust_xlsxwriter::{Format, Workbook};
use std::path::Path;

/// Write the full test run to `path` as an Excel workbook.
pub fn save(run: &TestRun, path: &Path) -> anyhow::Result<()> {
    let mut wb = Workbook::new();

    let bold = Format::new().set_bold();
    let num2 = Format::new().set_num_format("0.00");
    let num0 = Format::new().set_num_format("0");

    write_summary(&mut wb, run, &bold)?;
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

fn write_summary(wb: &mut Workbook, run: &TestRun, bold: &Format) -> anyhow::Result<()> {
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

    #[test]
    fn format_bytes_values() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(65536), "64.0 KiB");
        assert_eq!(format_bytes(1 << 20), "1.0 MiB");
        assert_eq!(format_bytes(1 << 30), "1.0 GiB");
    }
}
