//! HTML report snapshot test.
//!
//! Renders a fully deterministic sample report (fixed UUIDs + timestamps) and
//! asserts an FNV-1a hash of the output. The crate version string in the
//! footer is normalized so version bumps do not break the snapshot.
//!
//! Purpose: prove that refactors of `output/html` are byte-identical
//! (move-only). If this test fails, the rendered HTML changed.

use chrono::{DateTime, TimeZone, Utc};
use networker_tester::metrics::{
    ErrorCategory, ErrorRecord, HttpResult, Protocol, RequestAttempt, TcpResult, TestRun, UdpResult,
};
use networker_tester::output::html;
use uuid::Uuid;

fn fixed_time(offset_secs: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap() + chrono::Duration::seconds(offset_secs)
}

fn http_attempt(run_id: Uuid, seq: u32, proto: Protocol, ms: f64) -> RequestAttempt {
    RequestAttempt {
        attempt_id: Uuid::from_u128(0xA000 + seq as u128),
        run_id,
        protocol: proto,
        sequence_num: seq,
        started_at: fixed_time(seq as i64),
        finished_at: Some(fixed_time(seq as i64 + 1)),
        success: true,
        dns: None,
        tcp: Some(TcpResult {
            local_addr: Some("127.0.0.1:12345".into()),
            remote_addr: "127.0.0.1:8443".into(),
            connect_duration_ms: 1.5,
            attempt_count: 1,
            started_at: fixed_time(seq as i64),
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
            ttfb_ms: ms / 2.0,
            total_duration_ms: ms,
            redirect_count: 0,
            started_at: fixed_time(seq as i64),
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

fn failed_attempt(run_id: Uuid, seq: u32) -> RequestAttempt {
    RequestAttempt {
        attempt_id: Uuid::from_u128(0xB000 + seq as u128),
        run_id,
        protocol: Protocol::Http2,
        sequence_num: seq,
        started_at: fixed_time(seq as i64),
        finished_at: Some(fixed_time(seq as i64 + 2)),
        success: false,
        dns: None,
        tcp: None,
        tls: None,
        http: None,
        udp: None,
        error: Some(ErrorRecord {
            category: ErrorCategory::Tls,
            message: "handshake failed <cert>".into(),
            detail: Some("self-signed & untrusted".into()),
            occurred_at: fixed_time(seq as i64),
        }),
        retry_count: 1,
        server_timing: None,
        udp_throughput: None,
        page_load: None,
        browser: None,
        http_stack: None,
        rpm: None,
    }
}

fn udp_attempt(run_id: Uuid, seq: u32) -> RequestAttempt {
    RequestAttempt {
        attempt_id: Uuid::from_u128(0xC000 + seq as u128),
        run_id,
        protocol: Protocol::Udp,
        sequence_num: seq,
        started_at: fixed_time(seq as i64),
        finished_at: Some(fixed_time(seq as i64 + 1)),
        success: true,
        dns: None,
        tcp: None,
        tls: None,
        http: None,
        udp: Some(UdpResult {
            remote_addr: "127.0.0.1:9999".into(),
            probe_count: 20,
            success_count: 19,
            loss_percent: 5.0,
            rtt_min_ms: 0.4,
            rtt_avg_ms: 0.9,
            rtt_p95_ms: 1.7,
            jitter_ms: 0.2,
            started_at: fixed_time(seq as i64),
            probe_rtts_ms: vec![Some(0.4), Some(0.9), None, Some(1.7)],
        }),
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

fn make_run(run_seed: u128, target: &str, attempts: Vec<RequestAttempt>) -> TestRun {
    let run_id = Uuid::from_u128(run_seed);
    let attempts = attempts
        .into_iter()
        .map(|mut a| {
            a.run_id = run_id;
            a
        })
        .collect::<Vec<_>>();
    TestRun {
        schema_version: networker_tester::metrics::SCHEMA_VERSION.to_string(),
        run_id,
        started_at: fixed_time(0),
        finished_at: Some(fixed_time(60)),
        target_url: target.to_string(),
        target_host: "localhost".into(),
        modes: vec!["http1".into(), "http2".into(), "udp".into()],
        total_runs: 2,
        concurrency: 1,
        timeout_ms: 5000,
        client_os: "linux".into(),
        client_version: "0.0.0-snapshot".into(),
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
        attempts,
    }
}

/// FNV-1a 64-bit hash (dependency-free).
fn fnv1a(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Normalize the only version-dependent substring (footer "networker-tester
/// vX.Y.Z") so routine version bumps do not invalidate the snapshot.
fn normalized(html: &str) -> String {
    html.replace(
        concat!("networker-tester v", env!("CARGO_PKG_VERSION")),
        "networker-tester vNORMALIZED",
    )
}

#[test]
fn html_render_single_run_snapshot_is_stable() {
    let run = make_run(
        0x1111,
        "https://localhost:8443/health",
        vec![
            http_attempt(Uuid::nil(), 0, Protocol::Http1, 10.0),
            http_attempt(Uuid::nil(), 1, Protocol::Http1, 12.0),
            failed_attempt(Uuid::nil(), 2),
            udp_attempt(Uuid::nil(), 3),
        ],
    );
    let html = normalized(&html::render(&run, Some("report.css"), None));
    assert_eq!(
        (html.len(), fnv1a(html.as_bytes())),
        (11270, 9317960495895442682),
        "single-run HTML output changed — html renderer is expected to be byte-identical",
    );
}

#[test]
fn html_render_multi_run_snapshot_is_stable() {
    let run_a = make_run(
        0x2222,
        "https://alpha.example.com:8443/health",
        vec![
            http_attempt(Uuid::nil(), 0, Protocol::Http1, 10.0),
            http_attempt(Uuid::nil(), 1, Protocol::Http2, 8.0),
            udp_attempt(Uuid::nil(), 2),
        ],
    );
    let run_b = make_run(
        0x3333,
        "https://beta.example.com:8443/health",
        vec![
            http_attempt(Uuid::nil(), 0, Protocol::Http1, 20.0),
            http_attempt(Uuid::nil(), 1, Protocol::Http2, 16.0),
            failed_attempt(Uuid::nil(), 2),
        ],
    );
    let html = normalized(&html::render_multi(&[run_a, run_b], None, None));
    assert_eq!(
        (html.len(), fnv1a(html.as_bytes())),
        (19278, 13542180586597415269),
        "multi-run HTML output changed — html renderer is expected to be byte-identical",
    );
}
