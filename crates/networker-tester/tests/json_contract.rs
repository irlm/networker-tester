//! Golden-style contract test for the `networker-tester` JSON output.
//!
//! This is the Rust half of the frozen tester JSON contract that the hybrid
//! (Rust probe core + C# app layer) migration depends on. It asserts, without
//! any network I/O, that a serialized [`TestRun`]:
//!
//!   * carries a top-level `schema_version` string, and
//!   * exposes the key per-phase timing fields (dns / tcp / tls / ttfb / total).
//!
//! If this test breaks, the C# `Networker.Contracts` DTOs in `hybrid/` must be
//! updated in lockstep and the `schema_version` bumped.

use chrono::Utc;
use networker_tester::metrics::{
    DnsResult, HttpResult, Protocol, RequestAttempt, TcpResult, TestRun, TlsResult, SCHEMA_VERSION,
};
use uuid::Uuid;

/// Build a fully populated single-attempt run with every phase present.
fn sample_run() -> TestRun {
    let now = Utc::now();
    let run_id = Uuid::new_v4();

    let attempt = RequestAttempt {
        attempt_id: Uuid::new_v4(),
        run_id,
        protocol: Protocol::Http1,
        sequence_num: 0,
        started_at: now,
        finished_at: Some(now),
        success: true,
        dns: Some(DnsResult {
            query_name: "example.com".into(),
            resolved_ips: vec!["93.184.216.34".into()],
            duration_ms: 3.5,
            started_at: now,
            success: true,
            resolver: Some("system (192.168.1.1:53)".into()),
        }),
        tcp: Some(TcpResult {
            local_addr: Some("10.0.0.2:51000".into()),
            remote_addr: "93.184.216.34:443".into(),
            connect_duration_ms: 12.0,
            attempt_count: 1,
            started_at: now,
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
        tls: Some(TlsResult {
            protocol_version: "TLSv1.3".into(),
            cipher_suite: "TLS13_AES_128_GCM_SHA256".into(),
            alpn_negotiated: Some("h2".into()),
            cert_subject: None,
            cert_issuer: None,
            cert_expiry: None,
            handshake_duration_ms: 25.0,
            started_at: now,
            success: true,
            cert_chain: vec![],
            tls_backend: Some("rustls".into()),
            resumed: Some(false),
            handshake_kind: Some("full".into()),
            tls13_tickets_received: None,
            previous_handshake_duration_ms: None,
            previous_handshake_kind: None,
            previous_http_status_code: None,
            http_status_code: None,
        }),
        http: Some(HttpResult {
            negotiated_version: "HTTP/2".into(),
            status_code: 200,
            headers_size_bytes: 128,
            body_size_bytes: 1024,
            ttfb_ms: 40.0,
            total_duration_ms: 55.0,
            redirect_count: 0,
            started_at: now,
            response_headers: vec![],
            payload_bytes: 0,
            throughput_mbps: None,
            goodput_mbps: None,
            cpu_time_ms: None,
            csw_voluntary: None,
            csw_involuntary: None,
            http_handshake_ms: None,
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
    };

    TestRun {
        schema_version: SCHEMA_VERSION.to_string(),
        run_id,
        started_at: now,
        finished_at: Some(now),
        target_url: "https://example.com/health".into(),
        target_host: "example.com".into(),
        modes: vec!["http1".into()],
        total_runs: 1,
        concurrency: 1,
        timeout_ms: 30_000,
        client_os: "test".into(),
        client_version: "0.0.0-test".into(),
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
        attempts: vec![attempt],
    }
}

#[test]
fn json_output_carries_schema_version() {
    let run = sample_run();
    let v: serde_json::Value = serde_json::to_value(&run).expect("serialize TestRun");

    let schema_version = v
        .get("schema_version")
        .and_then(|s| s.as_str())
        .expect("schema_version must be present as a string");
    assert_eq!(
        schema_version, SCHEMA_VERSION,
        "serialized schema_version must match the crate constant"
    );
    assert_eq!(schema_version, "1.0", "frozen contract version is 1.0");
}

#[test]
fn json_output_carries_all_phase_timings() {
    let run = sample_run();
    let v: serde_json::Value = serde_json::to_value(&run).expect("serialize TestRun");

    let attempt = v
        .get("attempts")
        .and_then(|a| a.as_array())
        .and_then(|a| a.first())
        .expect("at least one attempt");

    // dns phase timing
    let dns_ms = attempt
        .pointer("/dns/duration_ms")
        .and_then(|n| n.as_f64())
        .expect("dns.duration_ms must be present");
    assert!(dns_ms > 0.0);

    // tcp phase timing
    let tcp_ms = attempt
        .pointer("/tcp/connect_duration_ms")
        .and_then(|n| n.as_f64())
        .expect("tcp.connect_duration_ms must be present");
    assert!(tcp_ms > 0.0);

    // tls phase timing
    let tls_ms = attempt
        .pointer("/tls/handshake_duration_ms")
        .and_then(|n| n.as_f64())
        .expect("tls.handshake_duration_ms must be present");
    assert!(tls_ms > 0.0);

    // ttfb + total (http) phase timings
    let ttfb_ms = attempt
        .pointer("/http/ttfb_ms")
        .and_then(|n| n.as_f64())
        .expect("http.ttfb_ms must be present");
    assert!(ttfb_ms > 0.0);

    let total_ms = attempt
        .pointer("/http/total_duration_ms")
        .and_then(|n| n.as_f64())
        .expect("http.total_duration_ms must be present");
    assert!(total_ms >= ttfb_ms);
}

/// v0.28.19 additive extension (trust audit V1): `dns.resolver` records which
/// resolver produced the DNS timing. The field is optional and serde-defaulted:
/// it is omitted when unknown and pre-0.28.19 JSON (without it) must still
/// deserialize. schema_version stays 1.0 — the change is purely additive.
#[test]
fn dns_resolver_field_is_additive_and_optional() {
    let run = sample_run();
    let mut v = serde_json::to_value(&run).expect("serialize");

    // Present when populated.
    assert_eq!(
        v.pointer("/attempts/0/dns/resolver")
            .and_then(|s| s.as_str()),
        Some("system (192.168.1.1:53)"),
        "resolver identity must serialize when known"
    );

    // Absent field (pre-0.28.19 producer) must still deserialize.
    v.pointer_mut("/attempts/0/dns")
        .and_then(|d| d.as_object_mut())
        .unwrap()
        .remove("resolver");
    let back: TestRun = serde_json::from_value(v).expect("deserialize without dns.resolver");
    assert_eq!(back.attempts[0].dns.as_ref().unwrap().resolver, None);
}

/// The v0.28.20 additive field `http.http_handshake_ms` is optional and
/// skip-serialized when `None`: pre-existing JSON (without the field) must
/// deserialize unchanged, and a run that doesn't set it serializes to the
/// exact same shape as before — the frozen 1.0 contract is untouched.
#[test]
fn http_handshake_ms_is_additive_and_optional() {
    let run = sample_run();
    let v: serde_json::Value = serde_json::to_value(&run).expect("serialize");

    let http = v
        .pointer("/attempts/0/http")
        .expect("http block must be present");
    assert!(
        http.get("http_handshake_ms").is_none(),
        "http_handshake_ms must be omitted when unset (shape unchanged)"
    );

    // Round-trip: absent field deserializes to None.
    let back: TestRun = serde_json::from_value(v).expect("deserialize");
    assert!(back.attempts[0]
        .http
        .as_ref()
        .expect("http")
        .http_handshake_ms
        .is_none());
}

/// A run serialized without `schema_version` (a pre-contract producer) must
/// still deserialize, defaulting the field — this proves the additive change is
/// backward compatible.
#[test]
fn schema_version_defaults_when_absent() {
    let run = sample_run();
    let mut v = serde_json::to_value(&run).expect("serialize");
    v.as_object_mut().unwrap().remove("schema_version");

    let back: TestRun = serde_json::from_value(v).expect("deserialize without schema_version");
    assert_eq!(back.schema_version, SCHEMA_VERSION);
}
