//! Shared fixtures for the HTML renderer tests, split by area:
//! - `single_run_tests` — render()/save(), escaping, byte formatting,
//!   per-section output, and row renderers
//! - `render_detail_tests` — single- and multi-target report structure
//! - `section_tests` — statistics/page-load/browser/udp/tcp/error sections,
//!   SVG charts, footer, and helper functions

mod render_detail_tests;
mod section_tests;
mod single_run_tests;

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
        schema_version: crate::metrics::SCHEMA_VERSION.to_string(),
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
        benchmark_environment_check: None,
        benchmark_stability_check: None,
        benchmark_phase: None,
        benchmark_scenario: None,
        benchmark_launch_index: None,
        benchmark_warmup_attempt_count: 0,
        benchmark_pilot_attempt_count: 0,
        benchmark_overhead_attempt_count: 0,
        benchmark_cooldown_attempt_count: 0,
        benchmark_execution_plan: None,
        benchmark_noise_thresholds: None,
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
                http_handshake_ms: None,
                socket_stats: None,
                content_encoding: None,
                content_length_header: None,
            }),
            udp: None,
            error: None,
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
            http_stack: None,
            rpm: None,
        }],
    }
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
            http_handshake_ms: None,
            socket_stats: None,
            content_encoding: None,
            content_length_header: None,
        }),
        udp: None,
        error: None,
        retry_count: 0,
        server_timing: None,
        udp_throughput: None,
        page_load: None,
        browser: None,
        http_stack: None,
        rpm: None,
    }
}
// ─────────────────────────────────────────────────────────────────────────
// Helpers / fixture builders shared by new tests
// ─────────────────────────────────────────────────────────────────────────

fn make_run_with_url(url: &str) -> TestRun {
    let run_id = Uuid::new_v4();
    TestRun {
        schema_version: crate::metrics::SCHEMA_VERSION.to_string(),
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
        benchmark_environment_check: None,
        benchmark_stability_check: None,
        benchmark_phase: None,
        benchmark_scenario: None,
        benchmark_launch_index: None,
        benchmark_warmup_attempt_count: 0,
        benchmark_pilot_attempt_count: 0,
        benchmark_overhead_attempt_count: 0,
        benchmark_cooldown_attempt_count: 0,
        benchmark_execution_plan: None,
        benchmark_noise_thresholds: None,
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
            Protocol::Http1 | Protocol::Http2 | Protocol::Http3 | Protocol::Native | Protocol::Curl
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
                http_handshake_ms: None,
                socket_stats: None,
                content_encoding: None,
                content_length_header: None,
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
        rpm: None,
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
