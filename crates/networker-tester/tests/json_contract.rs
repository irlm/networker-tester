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
    BenchmarkEnvironmentCheck, BenchmarkNoiseThresholds, ClockSync, CpuUsage, DnsResult, GeoInfo,
    HostInfo, HttpResult, LoadSample, PageLoadResult, Protocol, QuicStats, RequestAttempt,
    SecurityHeaders, SocketStats, TcpResult, TestRun, TlsResult, UrlDiagnosticStatus,
    UrlPageLoadStrategy, UrlTestRun, SCHEMA_VERSION,
};
use uuid::Uuid;

/// Build a fully populated single-attempt run with every phase present.
fn sample_run() -> TestRun {
    let now = Utc::now();
    let run_id = Uuid::new_v4();

    let attempt = RequestAttempt {
        phase: None,
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
            a_ms: None,
            aaaa_ms: None,
            a_record_count: None,
            aaaa_record_count: None,
            cname_chain: Vec::new(),
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
            ocsp_stapled: None,
            ocsp_response_bytes: None,
            quic_resumed: None,
            zero_rtt_attempted: None,
            zero_rtt_accepted: None,
            quic_resumed_handshake_ms: None,
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
            socket_stats: None,
            content_encoding: None,
            content_length_header: None,
            security_headers: None,
            quic_stats: None,
            quic_resumption_stats: None,
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
        ping: None,
        path: None,
        dualstack: None,
        websocket: None,
        pmtud: None,
        responsiveness: None,
        stamp: None,
        mthroughput: None,
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
        client_network: None,
        client_load_before: None,
        client_load_after: None,
        cpu_usage: None,
        clock_sync: None,
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
        client_geo: None,
        target_geo: None,
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

/// Measurement-gap #6/#7 additive fields (dns.a_ms/aaaa_ms/record counts/
/// cname_chain, tls.ocsp_stapled/ocsp_response_bytes, cert key/signature
/// detail): all serde-defaulted and skip-serialized when unset, so a run that
/// doesn't populate them serializes to the exact pre-existing shape and old
/// JSON (without them) still deserializes. schema_version stays 1.0.
#[test]
fn dns_and_tls_depth_fields_are_additive_and_optional() {
    let run = sample_run();
    let v: serde_json::Value = serde_json::to_value(&run).expect("serialize");

    let dns = v.pointer("/attempts/0/dns").expect("dns block");
    for absent in [
        "a_ms",
        "aaaa_ms",
        "a_record_count",
        "aaaa_record_count",
        "cname_chain",
    ] {
        assert!(
            dns.get(absent).is_none(),
            "dns.{absent} must be omitted when unset (shape unchanged)"
        );
    }
    let tls = v.pointer("/attempts/0/tls").expect("tls block");
    for absent in ["ocsp_stapled", "ocsp_response_bytes"] {
        assert!(
            tls.get(absent).is_none(),
            "tls.{absent} must be omitted when unset (shape unchanged)"
        );
    }

    // Round-trip: absent fields deserialize to their defaults.
    let back: TestRun = serde_json::from_value(v).expect("deserialize");
    let dns = back.attempts[0].dns.as_ref().unwrap();
    assert!(dns.a_ms.is_none() && dns.aaaa_ms.is_none());
    assert!(dns.cname_chain.is_empty());
    let tls = back.attempts[0].tls.as_ref().unwrap();
    assert!(tls.ocsp_stapled.is_none() && tls.ocsp_response_bytes.is_none());
}

/// Additive QUIC resumption/0-RTT fields on `tls` (`quic_resumed`,
/// `zero_rtt_attempted`, `zero_rtt_accepted`, `quic_resumed_handshake_ms`)
/// are optional and skip-serialized when `None`: a run that doesn't set them
/// serializes to the exact same shape as before and pre-existing JSON
/// deserializes unchanged — schema_version stays 1.0.
#[test]
fn quic_zero_rtt_fields_are_additive_and_optional() {
    let run = sample_run();
    let v: serde_json::Value = serde_json::to_value(&run).expect("serialize");

    let tls = v
        .pointer("/attempts/0/tls")
        .expect("tls block must be present");
    for field in [
        "quic_resumed",
        "zero_rtt_attempted",
        "zero_rtt_accepted",
        "quic_resumed_handshake_ms",
    ] {
        assert!(
            tls.get(field).is_none(),
            "{field} must be omitted when unset (frozen 1.0 shape unchanged)"
        );
    }

    // Round-trip: absent fields deserialize to None.
    let back: TestRun = serde_json::from_value(v).expect("deserialize");
    let tls = back.attempts[0].tls.as_ref().expect("tls");
    assert!(tls.quic_resumed.is_none());
    assert!(tls.zero_rtt_attempted.is_none());
    assert!(tls.zero_rtt_accepted.is_none());
    assert!(tls.quic_resumed_handshake_ms.is_none());

    // And a populated producer serializes them.
    let mut run = sample_run();
    {
        let tls = run.attempts[0].tls.as_mut().unwrap();
        tls.quic_resumed = Some(true);
        tls.zero_rtt_attempted = Some(true);
        tls.zero_rtt_accepted = Some(true);
        tls.quic_resumed_handshake_ms = Some(1.25);
    }
    let v: serde_json::Value = serde_json::to_value(&run).expect("serialize");
    assert_eq!(
        v.pointer("/attempts/0/tls/zero_rtt_accepted")
            .and_then(|b| b.as_bool()),
        Some(true)
    );
    assert_eq!(
        v.pointer("/attempts/0/tls/quic_resumed_handshake_ms")
            .and_then(|n| n.as_f64()),
        Some(1.25)
    );
}

/// Additive extension (measurement gap #11): `client_network` carries the
/// SOURCE network context (default interface, kind, MTU, egress IP, gateway,
/// VPN heuristic). Optional and skip-serialized when `None`: old JSON without
/// it must deserialize, and a run that doesn't set it serializes to the same
/// shape as before. schema_version stays 1.0.
#[test]
fn client_network_field_is_additive_and_optional() {
    let run = sample_run();
    let mut v = serde_json::to_value(&run).expect("serialize");

    // Unset → omitted entirely (frozen 1.0 shape unchanged).
    assert!(
        v.get("client_network").is_none(),
        "client_network must be omitted when unset"
    );

    // Old-producer JSON (no client_network key) must deserialize to None.
    let back: TestRun = serde_json::from_value(v.clone()).expect("deserialize old shape");
    assert!(back.client_network.is_none());

    // A populated client_network round-trips, including partially-collected
    // (best-effort) shapes where most fields are absent.
    v.as_object_mut().unwrap().insert(
        "client_network".into(),
        serde_json::json!({
            "default_interface": "en0",
            "interface_kind": "wifi",
            "mtu": 1500,
            "local_ip": "192.168.1.23",
            "gateway_ip": "192.168.1.1",
            "vpn_detected": false,
            "ipv6_available": true
        }),
    );
    let back: TestRun = serde_json::from_value(v).expect("deserialize with client_network");
    let net = back.client_network.expect("client_network populated");
    assert_eq!(net.default_interface.as_deref(), Some("en0"));
    assert_eq!(net.interface_kind.as_deref(), Some("wifi"));
    assert_eq!(net.mtu, Some(1500));
    assert_eq!(net.local_ip.as_deref(), Some("192.168.1.23"));
    assert_eq!(net.gateway_ip.as_deref(), Some("192.168.1.1"));
    assert_eq!(net.vpn_detected, Some(false));
    assert_eq!(net.vpn_interface, None, "absent field defaults to None");
    assert_eq!(net.ipv6_available, Some(true));

    // An empty object also deserializes — every field is optional.
    let empty: networker_tester::metrics::NetworkContext =
        serde_json::from_str("{}").expect("all-optional struct");
    assert!(empty.is_empty());
}

/// Additive contract check for the offline GeoIP enrichment fields
/// (`client_geo` / `target_geo`): omitted when unset (old shape unchanged),
/// old JSON without them still deserializes, populated values round-trip.
/// schema_version stays 1.0.
#[test]
fn geo_enrichment_fields_are_additive_and_optional() {
    // 1. A run without enrichment serializes to the exact pre-existing shape.
    let run = sample_run();
    let v: serde_json::Value = serde_json::to_value(&run).expect("serialize");
    for absent in ["client_geo", "target_geo"] {
        assert!(
            v.get(absent).is_none(),
            "{absent} must be omitted when unset (shape unchanged)"
        );
    }

    // 2. Old JSON (no geo keys) deserializes with None.
    let back: TestRun = serde_json::from_value(v).expect("deserialize old shape");
    assert!(back.client_geo.is_none() && back.target_geo.is_none());

    // 3. Populated enrichment round-trips, including partial (ASN-only) data.
    let mut run = sample_run();
    run.client_geo = Some(GeoInfo {
        country: Some("US".into()),
        city: None,
        asn: Some(13335),
        as_org: Some("Cloudflare, Inc.".into()),
        db_date: Some("2026-01-05".into()),
    });
    run.target_geo = Some(GeoInfo {
        country: Some("SE".into()),
        city: Some("Linköping".into()),
        asn: None,
        as_org: None,
        db_date: Some("2026-01-05".into()),
    });
    let v = serde_json::to_value(&run).expect("serialize enriched");
    assert_eq!(
        v.pointer("/client_geo/asn").and_then(|x| x.as_u64()),
        Some(13335)
    );
    assert!(
        v.pointer("/client_geo/city").is_none(),
        "unset geo sub-fields must be omitted too"
    );
    assert_eq!(
        v.pointer("/target_geo/country").and_then(|x| x.as_str()),
        Some("SE")
    );
    let back: TestRun = serde_json::from_value(v).expect("round-trip enriched");
    assert_eq!(back.client_geo, run.client_geo);
    assert_eq!(back.target_geo, run.target_geo);
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

/// Measurement-gap #14 additive field `http.security_headers`: serde-defaulted
/// and skip-serialized when unset, so an unpopulated run keeps the exact
/// pre-existing shape, old JSON still deserializes, and a populated audit
/// round-trips. schema_version stays 1.0.
#[test]
fn security_headers_field_is_additive_and_optional() {
    // Unset → omitted (shape unchanged) and absent-field JSON deserializes.
    let run = sample_run();
    let v: serde_json::Value = serde_json::to_value(&run).expect("serialize");
    assert!(
        v.pointer("/attempts/0/http")
            .expect("http block")
            .get("security_headers")
            .is_none(),
        "http.security_headers must be omitted when unset (shape unchanged)"
    );
    let back: TestRun = serde_json::from_value(v).expect("deserialize");
    assert!(back.attempts[0]
        .http
        .as_ref()
        .expect("http")
        .security_headers
        .is_none());

    // Populated → round-trips.
    let mut run = sample_run();
    let headers = vec![
        (
            "Strict-Transport-Security".to_string(),
            "max-age=63072000; includeSubDomains".to_string(),
        ),
        ("X-Content-Type-Options".to_string(), "nosniff".to_string()),
    ];
    run.attempts[0].http.as_mut().unwrap().security_headers =
        SecurityHeaders::from_response_headers(&headers);
    let v = serde_json::to_value(&run).expect("serialize populated");
    assert_eq!(
        v.pointer("/attempts/0/http/security_headers/hsts_max_age_secs")
            .and_then(|n| n.as_u64()),
        Some(63_072_000)
    );
    let back: TestRun = serde_json::from_value(v).expect("deserialize populated");
    let sec = back.attempts[0]
        .http
        .as_ref()
        .unwrap()
        .security_headers
        .as_ref()
        .expect("security_headers round-trips");
    assert_eq!(sec.x_content_type_options_nosniff, Some(true));
    assert_eq!(sec.csp_present, Some(false));
}

/// Deep-measurement M1 B.1 / M3 G1 additive fields `http.quic_stats` and
/// `http.quic_resumption_stats` (post-transfer `quinn::Connection::stats()`
/// snapshots): serde-defaulted, skip-serialized when unset — an unpopulated
/// run keeps the exact pre-existing shape, old JSON still deserializes, and a
/// populated snapshot round-trips. schema_version stays 1.0.
#[test]
fn quic_stats_fields_are_additive_and_optional() {
    // Unset → omitted (shape unchanged) and absent-field JSON deserializes.
    let run = sample_run();
    let v: serde_json::Value = serde_json::to_value(&run).expect("serialize");
    let http = v.pointer("/attempts/0/http").expect("http block");
    for field in ["quic_stats", "quic_resumption_stats"] {
        assert!(
            http.get(field).is_none(),
            "http.{field} must be omitted when unset (frozen 1.0 shape unchanged)"
        );
    }
    let back: TestRun = serde_json::from_value(v).expect("deserialize");
    let http = back.attempts[0].http.as_ref().expect("http");
    assert!(http.quic_stats.is_none());
    assert!(http.quic_resumption_stats.is_none());

    // Populated → round-trips, and every field inside QuicStats is itself
    // optional (a partially-populated snapshot serializes only what it has).
    let mut run = sample_run();
    run.attempts[0].http.as_mut().unwrap().quic_stats = Some(QuicStats {
        rtt_ms: Some(12.34),
        cwnd_bytes: Some(98_765),
        current_mtu: Some(1_452),
        lost_packets: Some(3),
        lost_bytes: Some(3_600),
        sent_packets: Some(210),
        congestion_events: Some(2),
        sent_plpmtud_probes: Some(5),
        lost_plpmtud_probes: Some(1),
        black_holes_detected: Some(0),
        udp_tx_datagrams: Some(180),
        udp_tx_bytes: Some(42_000),
        udp_rx_datagrams: Some(150),
        udp_rx_bytes: Some(1_048_576),
        congestion_algorithm: Some("cubic (client-config)".into()),
    });
    run.attempts[0].http.as_mut().unwrap().quic_resumption_stats = Some(QuicStats {
        sent_packets: Some(9),
        ..Default::default()
    });
    let v = serde_json::to_value(&run).expect("serialize populated");
    assert_eq!(
        v.pointer("/attempts/0/http/quic_stats/cwnd_bytes")
            .and_then(|n| n.as_u64()),
        Some(98_765),
        "QUIC cwnd is bytes — must serialize under the explicit _bytes name"
    );
    assert_eq!(
        v.pointer("/attempts/0/http/quic_stats/lost_packets")
            .and_then(|n| n.as_u64()),
        Some(3)
    );
    assert_eq!(
        v.pointer("/attempts/0/http/quic_resumption_stats/sent_packets")
            .and_then(|n| n.as_u64()),
        Some(9)
    );
    // Unset optional fields inside the struct are skipped, not nulled.
    assert!(v
        .pointer("/attempts/0/http/quic_resumption_stats/rtt_ms")
        .is_none());
    let back: TestRun = serde_json::from_value(v).expect("deserialize populated");
    let q = back.attempts[0]
        .http
        .as_ref()
        .unwrap()
        .quic_stats
        .as_ref()
        .expect("quic_stats round-trips");
    assert_eq!(q.rtt_ms, Some(12.34));
    assert_eq!(q.current_mtu, Some(1_452));
    assert_eq!(
        q.congestion_algorithm.as_deref(),
        Some("cubic (client-config)")
    );
}

/// Measurement-gap #15 additive fields `client_load_before` /
/// `client_load_after`: serde-defaulted, skip-serialized when unset, and
/// round-trip when populated. schema_version stays 1.0.
#[test]
fn client_load_fields_are_additive_and_optional() {
    let run = sample_run();
    let v: serde_json::Value = serde_json::to_value(&run).expect("serialize");
    for absent in ["client_load_before", "client_load_after"] {
        assert!(
            v.get(absent).is_none(),
            "{absent} must be omitted when unset (shape unchanged)"
        );
    }
    let back: TestRun = serde_json::from_value(v).expect("deserialize");
    assert!(back.client_load_before.is_none() && back.client_load_after.is_none());

    let mut run = sample_run();
    run.client_load_before = Some(LoadSample {
        load_avg_1m: Some(0.75),
        cpu_busy_percent: None,
        mem_available_mb: Some(12_288),
    });
    run.client_load_after = Some(LoadSample {
        load_avg_1m: Some(9.5),
        cpu_busy_percent: None,
        mem_available_mb: None,
    });
    let v = serde_json::to_value(&run).expect("serialize populated");
    assert_eq!(
        v.pointer("/client_load_before/mem_available_mb")
            .and_then(|n| n.as_u64()),
        Some(12_288)
    );
    // Never-collected sub-fields stay omitted, not fabricated.
    assert!(v.pointer("/client_load_before/cpu_busy_percent").is_none());
    let back: TestRun = serde_json::from_value(v).expect("deserialize populated");
    assert_eq!(back.client_load_after.unwrap().load_avg_1m, Some(9.5));
}

/// Measurement-gap #16 additive field `clock_sync`: serde-defaulted,
/// skip-serialized when unset (or when the SNTP query failed / was disabled),
/// and round-trips when populated. The per-attempt `clock_skew_ms` heuristic
/// is untouched. schema_version stays 1.0.
#[test]
fn clock_sync_field_is_additive_and_optional() {
    let run = sample_run();
    let v: serde_json::Value = serde_json::to_value(&run).expect("serialize");
    assert!(
        v.get("clock_sync").is_none(),
        "clock_sync must be omitted when unset (shape unchanged)"
    );
    let back: TestRun = serde_json::from_value(v).expect("deserialize");
    assert!(back.clock_sync.is_none());

    let mut run = sample_run();
    run.clock_sync = Some(ClockSync {
        ntp_server: Some("pool.ntp.org:123".into()),
        offset_ms: Some(-12.4),
        round_trip_ms: Some(28.9),
    });
    let v = serde_json::to_value(&run).expect("serialize populated");
    assert_eq!(
        v.pointer("/clock_sync/offset_ms").and_then(|n| n.as_f64()),
        Some(-12.4)
    );
    let back: TestRun = serde_json::from_value(v).expect("deserialize populated");
    let cs = back.clock_sync.expect("clock_sync round-trips");
    assert_eq!(cs.ntp_server.as_deref(), Some("pool.ntp.org:123"));
    assert_eq!(cs.round_trip_ms, Some(28.9));
}

/// cpu_busy_percent (reserved in gap #15, collector now implemented): the
/// two-snapshot run-window busy%% lands on the *after* load sample only,
/// serde-defaulted + skip-serialized. schema_version stays 1.0.
#[test]
fn cpu_busy_percent_round_trips_on_after_sample() {
    let mut run = sample_run();
    run.client_load_after = Some(LoadSample {
        load_avg_1m: Some(1.2),
        cpu_busy_percent: Some(37.5),
        mem_available_mb: None,
    });
    let v = serde_json::to_value(&run).expect("serialize");
    assert_eq!(
        v.pointer("/client_load_after/cpu_busy_percent")
            .and_then(|n| n.as_f64()),
        Some(37.5)
    );
    let back: TestRun = serde_json::from_value(v).expect("deserialize");
    assert_eq!(back.client_load_after.unwrap().cpu_busy_percent, Some(37.5));
}

/// Additive envelope field `cpu_usage` (sampled tester CPU trust upgrade):
/// omitted when unset, tolerated when absent in old JSON, and round-trips
/// when populated — including honest `None` sub-fields on platforms without
/// steal or below the p95 sample-size gate. schema_version stays 1.0.
#[test]
fn cpu_usage_field_is_additive_and_optional() {
    let run = sample_run();
    let v: serde_json::Value = serde_json::to_value(&run).expect("serialize");
    assert!(
        v.get("cpu_usage").is_none(),
        "cpu_usage must be omitted when unset (shape unchanged)"
    );
    let back: TestRun = serde_json::from_value(v).expect("deserialize");
    assert!(back.cpu_usage.is_none());

    let mut run = sample_run();
    run.cpu_usage = Some(CpuUsage {
        mean_busy_percent: Some(23.4),
        max_busy_percent: Some(91.0),
        p95_busy_percent: None, // below the MIN_SAMPLES_P95 gate
        mean_steal_percent: Some(2.1),
        max_steal_percent: Some(6.5),
        sample_count: 12,
        sample_interval_ms: 1000,
    });
    let v = serde_json::to_value(&run).expect("serialize populated");
    assert_eq!(
        v.pointer("/cpu_usage/max_busy_percent")
            .and_then(|n| n.as_f64()),
        Some(91.0)
    );
    assert_eq!(
        v.pointer("/cpu_usage/mean_steal_percent")
            .and_then(|n| n.as_f64()),
        Some(2.1)
    );
    // Gated / unmeasured sub-fields stay omitted, never fabricated.
    assert!(v.pointer("/cpu_usage/p95_busy_percent").is_none());
    assert_eq!(
        v.pointer("/cpu_usage/sample_count")
            .and_then(|n| n.as_u64()),
        Some(12)
    );
    assert_eq!(v["schema_version"], "1.0");
    let back: TestRun = serde_json::from_value(v).expect("deserialize populated");
    let cpu = back.cpu_usage.expect("cpu_usage round-trips");
    assert_eq!(cpu.max_steal_percent, Some(6.5));
    assert_eq!(cpu.sample_interval_ms, 1000);

    // Old envelopes that predate the sampled struct (or contain only the
    // struct without steal) still deserialize — sub-fields serde-default.
    let minimal: CpuUsage = serde_json::from_str(
        r#"{"mean_busy_percent": 40.0, "sample_count": 0, "sample_interval_ms": 1000}"#,
    )
    .expect("minimal cpu_usage deserializes");
    assert_eq!(minimal.mean_busy_percent, Some(40.0));
    assert_eq!(minimal.max_steal_percent, None);
}

/// Additive environment-check fields `cpu_busy_percent`/`cpu_steal_percent`
/// and the `max_cpu_busy_percent`/`max_cpu_steal_percent` thresholds:
/// serde-defaulted so pre-existing benchmark JSON still deserializes.
#[test]
fn benchmark_cpu_fields_are_additive_and_optional() {
    let check: BenchmarkEnvironmentCheck = serde_json::from_str(
        r#"{
            "attempted_samples": 5, "successful_samples": 5, "failed_samples": 0,
            "duration_ms": 250.0, "rtt_min_ms": 0.5, "rtt_avg_ms": 0.7,
            "rtt_max_ms": 1.0, "rtt_p50_ms": 0.6, "rtt_p95_ms": 0.9,
            "packet_loss_percent": 0.0, "network_type": "Loopback"
        }"#,
    )
    .expect("pre-CPU environment-check JSON deserializes");
    assert_eq!(check.cpu_busy_percent, None);
    assert_eq!(check.cpu_steal_percent, None);
    // Unmeasured CPU fields are omitted on the wire.
    let v = serde_json::to_value(&check).expect("serialize");
    assert!(v.get("cpu_busy_percent").is_none());

    let thresholds: BenchmarkNoiseThresholds = serde_json::from_str(
        r#"{"max_packet_loss_percent": 5.0, "max_jitter_ratio": 0.25, "max_rtt_spread_ratio": 2.0}"#,
    )
    .expect("pre-CPU thresholds JSON deserializes");
    assert_eq!(thresholds.max_cpu_busy_percent, 85.0);
    assert_eq!(thresholds.max_cpu_steal_percent, 5.0);
}

/// Additive field `page_load.per_connection_socket_stats`: omitted when empty
/// (non-Unix / QUIC / warm reuse), tolerated when absent in old JSON, and
/// round-trips when populated. schema_version stays 1.0.
#[test]
fn pageload_per_connection_socket_stats_is_additive_and_optional() {
    let make_page_load = |stats: Vec<SocketStats>| PageLoadResult {
        asset_count: 2,
        assets_fetched: 2,
        total_bytes: 20_480,
        total_ms: 120.0,
        ttfb_ms: 15.0,
        connections_opened: 2,
        asset_timings_ms: vec![50.0, 60.0],
        started_at: Utc::now(),
        tls_setup_ms: 0.0,
        tls_overhead_ratio: 0.0,
        per_connection_tls_ms: vec![0.0, 0.0],
        cpu_time_ms: None,
        connection_reused: false,
        per_connection_socket_stats: stats,
        assets_failed: None,
    };

    // Empty → omitted (shape unchanged) and absent-field JSON deserializes.
    let mut run = sample_run();
    run.attempts[0].page_load = Some(make_page_load(vec![]));
    let v = serde_json::to_value(&run).expect("serialize");
    assert!(
        v.pointer("/attempts/0/page_load")
            .expect("page_load block")
            .get("per_connection_socket_stats")
            .is_none(),
        "per_connection_socket_stats must be omitted when empty (shape unchanged)"
    );
    let back: TestRun = serde_json::from_value(v).expect("deserialize");
    assert!(back.attempts[0]
        .page_load
        .as_ref()
        .expect("page_load")
        .per_connection_socket_stats
        .is_empty());

    // Populated → round-trips with per-connection granularity.
    let mut run = sample_run();
    run.attempts[0].page_load = Some(make_page_load(vec![
        SocketStats {
            total_retrans: Some(3),
            snd_cwnd: Some(40),
            congestion_algorithm: Some("cubic".into()),
            ..Default::default()
        },
        SocketStats {
            total_retrans: Some(0),
            ..Default::default()
        },
    ]));
    let v = serde_json::to_value(&run).expect("serialize populated");
    assert_eq!(
        v.pointer("/attempts/0/page_load/per_connection_socket_stats/0/total_retrans")
            .and_then(|n| n.as_u64()),
        Some(3)
    );
    let back: TestRun = serde_json::from_value(v).expect("deserialize populated");
    let stats = &back.attempts[0]
        .page_load
        .as_ref()
        .unwrap()
        .per_connection_socket_stats;
    assert_eq!(stats.len(), 2);
    assert_eq!(stats[0].congestion_algorithm.as_deref(), Some("cubic"));
}

/// Additive field `page_load.assets_failed` (Wave T: 404 ≠ fetched): omitted
/// when None (pre-v0.28.82 data shape unchanged), tolerated when absent in
/// old JSON, and round-trips when populated. schema_version stays 1.0.
#[test]
fn pageload_assets_failed_is_additive_and_optional() {
    let make_page_load = |failed: Option<u32>| PageLoadResult {
        asset_count: 3,
        assets_fetched: 2,
        total_bytes: 20_480,
        total_ms: 120.0,
        ttfb_ms: 15.0,
        connections_opened: 1,
        // Index-aligned: failed asset carries the 0.0 sentinel.
        asset_timings_ms: vec![50.0, 0.0, 60.0],
        started_at: Utc::now(),
        tls_setup_ms: 0.0,
        tls_overhead_ratio: 0.0,
        per_connection_tls_ms: vec![0.0],
        cpu_time_ms: None,
        connection_reused: false,
        per_connection_socket_stats: vec![],
        assets_failed: failed,
    };

    // None → omitted (shape unchanged) and absent-field JSON deserializes.
    let mut run = sample_run();
    run.attempts[0].page_load = Some(make_page_load(None));
    let v = serde_json::to_value(&run).expect("serialize");
    assert!(
        v.pointer("/attempts/0/page_load")
            .expect("page_load block")
            .get("assets_failed")
            .is_none(),
        "assets_failed must be omitted when None (shape unchanged)"
    );
    let back: TestRun = serde_json::from_value(v).expect("deserialize");
    assert!(back.attempts[0]
        .page_load
        .as_ref()
        .expect("page_load")
        .assets_failed
        .is_none());

    // Populated → round-trips.
    let mut run = sample_run();
    run.attempts[0].page_load = Some(make_page_load(Some(1)));
    let v = serde_json::to_value(&run).expect("serialize populated");
    assert_eq!(
        v.pointer("/attempts/0/page_load/assets_failed")
            .and_then(|n| n.as_u64()),
        Some(1)
    );
    let back: TestRun = serde_json::from_value(v).expect("deserialize populated");
    assert_eq!(
        back.attempts[0].page_load.as_ref().unwrap().assets_failed,
        Some(1)
    );
}

/// Additive `server_info.load_avg_1m` / `server_info.mem_available_mb`
/// (endpoint-side live load sampled when GET /info was served): omitted when
/// unset — old endpoints without the fields deserialize to None (additive
/// tolerance both directions). schema_version stays 1.0.
#[test]
fn host_info_server_load_fields_are_additive_and_optional() {
    let host_info = |load: Option<f64>, mem: Option<u64>| HostInfo {
        os: "linux".into(),
        arch: "x86_64".into(),
        cpu_cores: 2,
        total_memory_mb: Some(4096),
        os_version: None,
        hostname: None,
        server_version: Some("0.28.80".into()),
        uptime_secs: Some(3600),
        region: None,
        load_avg_1m: load,
        mem_available_mb: mem,
    };

    // Unset → omitted (shape unchanged) and absent-field JSON deserializes.
    let mut run = sample_run();
    run.server_info = Some(host_info(None, None));
    let v = serde_json::to_value(&run).expect("serialize");
    let info = v.pointer("/server_info").expect("server_info block");
    assert!(
        info.get("load_avg_1m").is_none() && info.get("mem_available_mb").is_none(),
        "server load fields must be omitted when unset (shape unchanged)"
    );
    let back: TestRun = serde_json::from_value(v).expect("deserialize");
    let info = back.server_info.expect("server_info");
    assert!(info.load_avg_1m.is_none() && info.mem_available_mb.is_none());

    // Populated → round-trips.
    let mut run = sample_run();
    run.server_info = Some(host_info(Some(0.42), Some(1536)));
    let v = serde_json::to_value(&run).expect("serialize populated");
    assert_eq!(
        v.pointer("/server_info/load_avg_1m")
            .and_then(|n| n.as_f64()),
        Some(0.42)
    );
    let back: TestRun = serde_json::from_value(v).expect("deserialize populated");
    let info = back.server_info.expect("server_info round-trips");
    assert_eq!(info.mem_available_mb, Some(1536));
}

/// Additive `UrlTestRun.security_headers` (wave-3 derivation wired into the
/// URL-diagnostics path): omitted when unset, tolerated when absent in old
/// JSON, and round-trips when populated.
#[test]
fn url_test_run_security_headers_is_additive_and_optional() {
    let sample_url_run = || UrlTestRun {
        id: Uuid::new_v4(),
        started_at: Utc::now(),
        completed_at: None,
        requested_url: "https://example.com/".into(),
        final_url: None,
        status: UrlDiagnosticStatus::Completed,
        page_load_strategy: UrlPageLoadStrategy::Browser,
        browser_engine: None,
        browser_version: None,
        user_agent: None,
        primary_origin: None,
        observed_protocol_primary_load: None,
        advertised_alt_svc: None,
        validated_http_versions: vec![],
        security_headers: None,
        tls_version: None,
        cipher_suite: None,
        alpn: None,
        dns_ms: None,
        connect_ms: None,
        handshake_ms: None,
        ttfb_ms: None,
        dom_content_loaded_ms: None,
        load_event_ms: None,
        network_idle_ms: None,
        capture_end_ms: None,
        total_requests: 0,
        total_transfer_bytes: 0,
        peak_concurrent_connections: None,
        redirect_count: 0,
        failure_count: 0,
        har_path: None,
        pcap_path: None,
        pcap_summary: None,
        capture_errors: vec![],
        environment_notes: None,
        origin_summaries: vec![],
        connection_summary: None,
        resources: vec![],
        protocol_runs: vec![],
    };

    // Unset → omitted (shape unchanged) and absent-field JSON deserializes.
    let run = sample_url_run();
    let v = serde_json::to_value(&run).expect("serialize");
    assert!(
        v.get("security_headers").is_none(),
        "security_headers must be omitted when unset (shape unchanged)"
    );
    let back: UrlTestRun = serde_json::from_value(v).expect("deserialize");
    assert!(back.security_headers.is_none());

    // Populated → round-trips with the same derivation as TestRun http results.
    let mut run = sample_url_run();
    run.security_headers = SecurityHeaders::from_response_headers(&[
        (
            "strict-transport-security".to_string(),
            "max-age=31536000".to_string(),
        ),
        ("x-frame-options".to_string(), "DENY".to_string()),
    ]);
    let v = serde_json::to_value(&run).expect("serialize populated");
    assert_eq!(
        v.pointer("/security_headers/hsts_max_age_secs")
            .and_then(|n| n.as_u64()),
        Some(31_536_000)
    );
    let back: UrlTestRun = serde_json::from_value(v).expect("deserialize populated");
    let sec = back.security_headers.expect("security_headers round-trips");
    assert_eq!(sec.x_frame_options.as_deref(), Some("DENY"));
    assert_eq!(sec.csp_present, Some(false));
}

/// The v0.28.81 additive field `attempt.phase` (structural benchmark phase
/// attribution, m5 G3) is optional and skip-serialized when `None`:
/// pre-existing JSON without the field deserializes to `None`, a phase-less
/// run serializes to the exact same shape as before (frozen 1.0 contract
/// untouched), and a populated phase round-trips.
#[test]
fn attempt_phase_field_is_additive_and_optional() {
    // Phase-less run (non-benchmark): field must be absent from the wire.
    let run = sample_run();
    let v = serde_json::to_value(&run).expect("serialize");
    assert!(
        v.pointer("/attempts/0/phase").is_none(),
        "phase must be skip-serialized when None"
    );

    // Old JSON without the field must deserialize to None.
    let back: TestRun = serde_json::from_value(v).expect("deserialize without phase");
    assert_eq!(back.attempts[0].phase, None);

    // Populated phase round-trips.
    let mut run = sample_run();
    run.attempts[0].phase = Some("warmup".to_string());
    let v = serde_json::to_value(&run).expect("serialize populated");
    assert_eq!(
        v.pointer("/attempts/0/phase").and_then(|s| s.as_str()),
        Some("warmup")
    );
    let back: TestRun = serde_json::from_value(v).expect("deserialize populated");
    assert_eq!(back.attempts[0].phase.as_deref(), Some("warmup"));

    // Schema version stays frozen at 1.0 — the field is additive.
    assert_eq!(SCHEMA_VERSION, "1.0");
}

/// Wave R additive attempt fields: `responsiveness` (draft-conformant
/// working-conditions RPM) and `stamp` (RFC 8762). Old JSON without them
/// deserializes to None, None is skip-serialized, populated results
/// round-trip. schema_version stays 1.0.
#[test]
fn responsiveness_and_stamp_fields_are_additive_and_optional() {
    use networker_tester::metrics::{ResponsivenessDirection, ResponsivenessResult, StampResult};

    // Absent from every attempt of a run that never ran the modes.
    let run = sample_run();
    let v = serde_json::to_value(&run).expect("serialize");
    assert!(
        v.pointer("/attempts/0/responsiveness").is_none(),
        "responsiveness must be skip-serialized when None"
    );
    assert!(
        v.pointer("/attempts/0/stamp").is_none(),
        "stamp must be skip-serialized when None"
    );

    // Old JSON (no fields) deserializes to None.
    let back: TestRun = serde_json::from_value(v).expect("deserialize pre-Wave-R attempt");
    assert!(back.attempts[0].responsiveness.is_none());
    assert!(back.attempts[0].stamp.is_none());

    // Populated results round-trip.
    let now = Utc::now();
    let direction = ResponsivenessDirection {
        saturation_reached: true,
        responsiveness_stable: true,
        saturated_connections: 6,
        intervals: 9,
        load_duration_ms: 9_000.0,
        bytes_transferred: 900_000_000,
        capacity_mbps: Some(100.0),
        rpm: Some(950.0),
        foreign_rpm: Some(900.0),
        self_rpm: Some(1000.0),
        foreign_tcp_tm_ms: Some(20.0),
        foreign_tls_tm_ms: None, // cleartext target — TCP-only variant
        foreign_http_tm_ms: Some(113.0),
        self_http_tm_ms: Some(60.0),
        foreign_probes_sent: 45,
        foreign_probes_ok: 44,
        self_probes_sent: 45,
        self_probes_ok: 45,
    };
    let mut run = sample_run();
    run.attempts[0].responsiveness = Some(ResponsivenessResult {
        remote_addr: "http://127.0.0.1:8080/".into(),
        rpm_download: Some(950.0),
        rpm_upload: Some(800.0),
        capacity_down_mbps: Some(100.0),
        capacity_up_mbps: Some(50.0),
        download: direction.clone(),
        upload: Some(direction),
        upload_error: None,
        started_at: now,
    });
    run.attempts[0].stamp = Some(StampResult {
        remote_addr: "127.0.0.1:9997".into(),
        probes_sent: 50,
        replies_received: 49,
        loss_percent: 2.0,
        loss_sent_percent: Some(2.0),
        loss_return_percent: Some(0.0),
        rtt_min_ms: 1.0,
        rtt_avg_ms: 1.4,
        rtt_p95_ms: 2.1,
        jitter_ms: 0.2,
        near_ipdv_mean_ms: Some(0.1),
        near_ipdv_p95_ms: Some(0.3),
        far_ipdv_mean_ms: Some(0.15),
        far_ipdv_p95_ms: Some(0.4),
        reflector_processing_avg_us: Some(42.0),
        reflector_seq_max: Some(48),
        near_owd_raw_avg_ms: Some(12.5),
        far_owd_raw_avg_ms: Some(-11.1),
        owd_forward_est_ms: Some(0.7),
        owd_return_est_ms: Some(0.7),
        owd_uncertainty_ms: Some(9.5),
        probe_rtts_ms: vec![Some(1.4); 49].into_iter().chain([None]).collect(),
        interval_ms: 50,
        started_at: now,
    });

    let v = serde_json::to_value(&run).expect("serialize populated");
    assert_eq!(
        v.pointer("/attempts/0/responsiveness/rpm_download")
            .and_then(|n| n.as_f64()),
        Some(950.0)
    );
    assert!(
        v.pointer("/attempts/0/responsiveness/download/foreign_tls_tm_ms")
            .is_none(),
        "None trimmed means must be omitted"
    );
    assert_eq!(
        v.pointer("/attempts/0/stamp/loss_return_percent")
            .and_then(|n| n.as_f64()),
        Some(0.0)
    );
    let back: TestRun = serde_json::from_value(v).expect("deserialize populated");
    let r = back.attempts[0].responsiveness.as_ref().unwrap();
    assert_eq!(r.rpm_download, Some(950.0));
    assert!(r.download.saturation_reached);
    let s = back.attempts[0].stamp.as_ref().unwrap();
    assert_eq!(s.replies_received, 49);
    assert_eq!(s.loss_sent_percent, Some(2.0));
    assert_eq!(s.probe_rtts_ms.len(), 50);

    // Schema version stays frozen at 1.0 — the fields are additive.
    assert_eq!(SCHEMA_VERSION, "1.0");
}

/// Wave W additive attempt field: `mthroughput` (multi-connection link
/// capacity). Old JSON without it deserializes to None, None is
/// skip-serialized, populated results round-trip. schema_version stays 1.0.
#[test]
fn mthroughput_field_is_additive_and_optional() {
    use networker_tester::metrics::{MthroughputConn, MthroughputDirection, MthroughputResult};

    // Absent from every attempt of a run that never ran the mode.
    let run = sample_run();
    let v = serde_json::to_value(&run).expect("serialize");
    assert!(
        v.pointer("/attempts/0/mthroughput").is_none(),
        "mthroughput must be skip-serialized when None"
    );

    // Old JSON (no field) deserializes to None.
    let back: TestRun = serde_json::from_value(v).expect("deserialize pre-Wave-W attempt");
    assert!(back.attempts[0].mthroughput.is_none());

    // Populated results round-trip.
    let now = Utc::now();
    let direction = MthroughputDirection {
        saturation_reached: true,
        connections: 3,
        intervals: 9,
        ramp_duration_ms: 5_000.0,
        measure_duration_ms: 4_000.0,
        load_duration_ms: 9_000.0,
        bytes_transferred: 1_200_000_000,
        capacity_mbps: Some(120.0),
        per_conn_min_mbps: Some(20.0),
        per_conn_max_mbps: Some(60.0),
        per_conn_mean_mbps: Some(40.0),
        fair_share_spread_pct: Some(100.0),
        rwnd_limited_conns: 1,
        sndbuf_limited_conns: 0,
        path_limited_conns: 2,
        unobserved_conns: 0,
        per_conn: vec![
            MthroughputConn {
                conn: 0,
                mbps: 60.0,
                verdict: "path-limited".into(),
                retrans: Some(0),
            },
            MthroughputConn {
                conn: 1,
                mbps: 40.0,
                verdict: "path-limited".into(),
                retrans: Some(12),
            },
            MthroughputConn {
                conn: 2,
                mbps: 20.0,
                verdict: "rwnd-limited 84%".into(),
                retrans: None, // kernel exposed no counter — omitted, not 0
            },
        ],
    };
    let mut run = sample_run();
    run.attempts[0].mthroughput = Some(MthroughputResult {
        remote_addr: "http://127.0.0.1:8080/".into(),
        capacity_down_mbps: Some(120.0),
        capacity_up_mbps: Some(45.0),
        conns_down: 3,
        conns_up: Some(3),
        fair_share_spread_down_pct: Some(100.0),
        fair_share_spread_up_pct: Some(10.0),
        download: direction.clone(),
        upload: Some(direction),
        upload_error: None,
        started_at: now,
    });

    let v = serde_json::to_value(&run).expect("serialize populated");
    assert_eq!(
        v.pointer("/attempts/0/mthroughput/capacity_down_mbps")
            .and_then(|n| n.as_f64()),
        Some(120.0)
    );
    assert_eq!(
        v.pointer("/attempts/0/mthroughput/download/per_conn/2/verdict")
            .and_then(|n| n.as_str()),
        Some("rwnd-limited 84%")
    );
    assert!(
        v.pointer("/attempts/0/mthroughput/download/per_conn/2/retrans")
            .is_none(),
        "None retrans must be omitted"
    );
    let back: TestRun = serde_json::from_value(v).expect("deserialize populated");
    let m = back.attempts[0].mthroughput.as_ref().unwrap();
    assert_eq!(m.capacity_down_mbps, Some(120.0));
    assert!(m.download.saturation_reached);
    assert_eq!(m.download.per_conn.len(), 3);
    assert_eq!(m.download.per_conn[1].retrans, Some(12));
    assert_eq!(m.conns_up, Some(3));

    // Schema version stays frozen at 1.0 — the field is additive.
    assert_eq!(SCHEMA_VERSION, "1.0");
}

/// Additive B.2 tcp_info fields on `socket_stats` (busy/rwnd/sndbuf triad,
/// bytes_retrans, delivered_ce, ECN/TFO/app-limited flags, pacing rate…):
/// omitted when unobserved (non-Linux, old kernels, pre-fix JSON) and
/// round-trip when populated. schema_version stays 1.0.
#[test]
fn socket_stats_b2_tcp_info_fields_are_additive_and_optional() {
    // Pre-B.2 JSON (no new keys) must deserialize to all-None.
    let old: SocketStats = serde_json::from_str(r#"{"mss_bytes": 1460, "rtt_estimate_ms": 1.5}"#)
        .expect("pre-B.2 socket_stats JSON deserializes");
    assert_eq!(old.busy_time_us, None);
    assert_eq!(old.rwnd_limited_us, None);
    assert_eq!(old.sndbuf_limited_us, None);
    assert_eq!(old.bytes_retrans, None);
    assert_eq!(old.delivered_ce, None);
    assert_eq!(old.ecn_negotiated, None);
    assert_eq!(old.tfo_used, None);
    assert_eq!(old.app_limited, None);
    assert_eq!(old.pacing_rate_bps, None);
    assert_eq!(old.rcv_rtt_ms, None);

    // Unobserved fields are omitted on the wire (shape unchanged for
    // non-Linux testers and old kernels).
    let v = serde_json::to_value(&old).expect("serialize");
    for key in [
        "busy_time_us",
        "rwnd_limited_us",
        "sndbuf_limited_us",
        "bytes_acked",
        "bytes_sent",
        "bytes_retrans",
        "delivered",
        "delivered_ce",
        "ecn_negotiated",
        "tfo_used",
        "app_limited",
        "pacing_rate_bps",
        "notsent_bytes",
        "reord_seen",
        "dsack_dups",
        "rcv_rtt_ms",
    ] {
        assert!(v.get(key).is_none(), "{key} must be omitted when None");
    }

    // Populated triad + flags round-trip.
    let populated = SocketStats {
        busy_time_us: Some(1_000_000),
        rwnd_limited_us: Some(840_000),
        sndbuf_limited_us: Some(0),
        bytes_retrans: Some(2_896),
        bytes_sent: Some(1_050_000),
        delivered_ce: Some(3),
        ecn_negotiated: Some(true),
        tfo_used: Some(false),
        app_limited: Some(true),
        ..Default::default()
    };
    let v = serde_json::to_value(&populated).expect("serialize populated");
    let back: SocketStats = serde_json::from_value(v).expect("deserialize populated");
    assert_eq!(back, populated);
    assert_eq!(SCHEMA_VERSION, "1.0");
}

/// Additive B.6 UDP local-drop split fields (`local_drops`,
/// `so_rcvbuf_bytes` on udp/udp_throughput; `loaded_local_drops` on rpm):
/// absent in pre-fix JSON, omitted when unobservable (None ≠ 0), and
/// round-trip when populated — loss_percent stays untouched (the split is
/// surfaced, never subtracted).
#[test]
fn udp_local_drop_fields_are_additive_and_optional() {
    use networker_tester::metrics::UdpThroughputResult;

    // Pre-B.6 JSON must deserialize with honest None (not zero).
    let old: UdpThroughputResult = serde_json::from_str(
        r#"{
            "remote_addr": "127.0.0.1:9998", "payload_bytes": 262144,
            "datagrams_sent": 188, "datagrams_received": 180,
            "loss_percent": 4.2, "transfer_ms": 10.0,
            "throughput_mbps": 25.0, "started_at": "2026-07-27T00:00:00Z"
        }"#,
    )
    .expect("pre-B.6 udp_throughput JSON deserializes");
    assert_eq!(old.local_drops, None);
    assert_eq!(old.so_rcvbuf_bytes, None);
    // loss_percent untouched by the split.
    assert!((old.loss_percent - 4.2).abs() < 1e-9);

    // None → omitted on the wire.
    let v = serde_json::to_value(&old).expect("serialize");
    assert!(v.get("local_drops").is_none());
    assert!(v.get("so_rcvbuf_bytes").is_none());

    // Populated → round-trips; both figures visible side by side.
    let mut populated = old.clone();
    populated.local_drops = Some(8);
    populated.so_rcvbuf_bytes = Some(212_992);
    let v = serde_json::to_value(&populated).expect("serialize populated");
    assert_eq!(v.get("local_drops").and_then(|n| n.as_u64()), Some(8));
    assert_eq!(
        v.get("so_rcvbuf_bytes").and_then(|n| n.as_u64()),
        Some(212_992)
    );
    let back: UdpThroughputResult = serde_json::from_value(v).expect("deserialize populated");
    assert_eq!(back.local_drops, Some(8));
    assert!((back.loss_percent - 4.2).abs() < 1e-9);
    assert_eq!(SCHEMA_VERSION, "1.0");
}

/// Wave W additive browser fields: Core Web Vitals (`lcp_ms`, `cls`,
/// `fcp_ms`, `tbt_ms`), the CDP request waterfall (`waterfall`,
/// `waterfall_truncated`) and real wire bytes (`wire_bytes_total`).
/// Pre-Wave-W browser JSON deserializes with all of them None/empty/false,
/// None/empty are skip-serialized, populated results round-trip.
/// schema_version stays 1.0.
#[test]
fn browser_cwv_and_waterfall_fields_are_additive_and_optional() {
    use networker_tester::metrics::{BrowserRequest, BrowserRequestTiming, BrowserResult};

    // Pre-Wave-W browser JSON (no new keys) must deserialize.
    let old: BrowserResult = serde_json::from_str(
        r#"{
            "load_ms": 350.0, "dom_content_loaded_ms": 200.0, "ttfb_ms": 50.0,
            "resource_count": 21, "transferred_bytes": 204800,
            "protocol": "h2", "resource_protocols": [["h2", 21]],
            "started_at": "2026-07-27T00:00:00Z"
        }"#,
    )
    .expect("pre-Wave-W browser JSON deserializes");
    assert_eq!(old.lcp_ms, None);
    assert_eq!(old.cls, None);
    assert_eq!(old.fcp_ms, None);
    assert_eq!(old.tbt_ms, None);
    assert_eq!(old.wire_bytes_total, None);
    assert!(old.waterfall.is_empty());
    assert!(!old.waterfall_truncated);

    // None/empty → omitted on the wire (0-as-missing is banned; absence is).
    let v = serde_json::to_value(&old).expect("serialize");
    assert!(v.get("lcp_ms").is_none());
    assert!(v.get("cls").is_none());
    assert!(v.get("fcp_ms").is_none());
    assert!(v.get("tbt_ms").is_none());
    assert!(v.get("wire_bytes_total").is_none());
    assert!(v.get("waterfall").is_none());
    assert!(v.get("waterfall_truncated").is_none());

    // Populated → round-trips. CLS 0.0 is a real value, distinct from None.
    let mut populated = old.clone();
    populated.lcp_ms = Some(321.5);
    populated.cls = Some(0.0);
    populated.fcp_ms = Some(120.25);
    populated.tbt_ms = Some(0.0);
    populated.wire_bytes_total = Some(212_345);
    populated.waterfall_truncated = true;
    populated.waterfall = vec![BrowserRequest {
        url: "https://localhost:8443/browser-page".into(),
        method: "GET".into(),
        status: Some(200),
        mime_type: Some("text/html".into()),
        protocol: Some("h2".into()),
        wire_bytes: Some(1_234),
        start_ms: Some(0.0),
        end_ms: Some(48.7),
        from_disk_cache: false,
        from_service_worker: false,
        timing: Some(BrowserRequestTiming {
            dns_ms: Some(0.1),
            connect_ms: Some(1.2),
            ssl_ms: Some(0.9),
            send_ms: Some(0.05),
            wait_ms: Some(12.0),
            receive_ms: Some(30.0),
        }),
    }];
    let v = serde_json::to_value(&populated).expect("serialize populated");
    assert_eq!(
        v.pointer("/cls").and_then(|n| n.as_f64()),
        Some(0.0),
        "CLS 0.0 must serialize (a value, not missing)"
    );
    assert_eq!(
        v.pointer("/wire_bytes_total").and_then(|n| n.as_u64()),
        Some(212_345)
    );
    assert_eq!(
        v.pointer("/waterfall/0/timing/wait_ms")
            .and_then(|n| n.as_f64()),
        Some(12.0)
    );
    assert_eq!(
        v.pointer("/waterfall/0/status").and_then(|n| n.as_u64()),
        Some(200)
    );
    assert_eq!(
        v.pointer("/waterfall_truncated").and_then(|b| b.as_bool()),
        Some(true)
    );
    let back: BrowserResult = serde_json::from_value(v).expect("deserialize populated");
    assert_eq!(back.lcp_ms, Some(321.5));
    assert_eq!(back.cls, Some(0.0));
    assert_eq!(back.waterfall.len(), 1);
    assert_eq!(back.waterfall[0].wire_bytes, Some(1_234));
    assert!(back.waterfall_truncated);
    // Legacy fields untouched.
    assert_eq!(back.transferred_bytes, 204_800);
    assert_eq!(SCHEMA_VERSION, "1.0");
}
