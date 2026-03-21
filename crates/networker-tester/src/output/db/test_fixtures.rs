/// Shared test fixtures for database backend tests.
///
/// Both `mssql` and `postgres` integration tests use these helpers to construct
/// sample `TestRun` / `RequestAttempt` values.
use crate::metrics::{
    DnsResult, ErrorCategory, ErrorRecord, HttpResult, Protocol, RequestAttempt,
    ServerTimingResult, TcpResult, TestRun, TlsResult, UdpResult,
};
use chrono::Utc;
use uuid::Uuid;

pub(crate) fn make_run(run_id: Uuid, attempts: Vec<RequestAttempt>) -> TestRun {
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
        client_os: std::env::consts::OS.into(),
        client_version: env!("CARGO_PKG_VERSION").into(),
        server_info: None,
        client_info: None,
        baseline: None,
        packet_capture_summary: None,
        attempts,
    }
}

pub(crate) fn bare_attempt(run_id: Uuid) -> RequestAttempt {
    RequestAttempt {
        attempt_id: Uuid::new_v4(),
        run_id,
        protocol: Protocol::Http1,
        sequence_num: 0,
        started_at: Utc::now(),
        finished_at: Some(Utc::now()),
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
        page_load: None,
        browser: None,
        http_stack: None,
    }
}

/// Constructs an attempt with all 7 sub-result types populated.
#[allow(dead_code)]
pub(crate) fn full_attempt(run_id: Uuid) -> RequestAttempt {
    RequestAttempt {
        attempt_id: Uuid::new_v4(),
        run_id,
        protocol: Protocol::Http1,
        sequence_num: 1,
        started_at: Utc::now(),
        finished_at: Some(Utc::now()),
        success: false,
        dns: Some(DnsResult {
            query_name: "localhost".into(),
            resolved_ips: vec!["127.0.0.1".into()],
            duration_ms: 1.5,
            started_at: Utc::now(),
            success: true,
        }),
        tcp: Some(TcpResult {
            local_addr: Some("127.0.0.1:12345".into()),
            remote_addr: "127.0.0.1:8080".into(),
            connect_duration_ms: 0.5,
            attempt_count: 1,
            started_at: Utc::now(),
            success: true,
            mss_bytes: Some(1460),
            rtt_estimate_ms: Some(0.3),
            retransmits: Some(0),
            total_retrans: Some(0),
            snd_cwnd: Some(10),
            snd_ssthresh: None,
            rtt_variance_ms: Some(0.05),
            rcv_space: Some(65535),
            segs_out: Some(5),
            segs_in: Some(5),
            congestion_algorithm: Some("cubic".into()),
            delivery_rate_bps: Some(1_000_000),
            min_rtt_ms: Some(0.2),
        }),
        tls: Some(TlsResult {
            protocol_version: "TLSv1.3".into(),
            cipher_suite: "TLS_AES_256_GCM_SHA384".into(),
            alpn_negotiated: Some("http/1.1".into()),
            cert_subject: Some("CN=localhost".into()),
            cert_issuer: Some("CN=localhost".into()),
            cert_expiry: None,
            handshake_duration_ms: 5.0,
            started_at: Utc::now(),
            success: true,
            cert_chain: vec![],
            tls_backend: Some("rustls".into()),
            resumed: None,
            handshake_kind: None,
            tls13_tickets_received: None,
            previous_handshake_duration_ms: None,
            previous_handshake_kind: None,
            previous_http_status_code: None,
            http_status_code: None,
        }),
        http: Some(HttpResult {
            negotiated_version: "HTTP/1.1".into(),
            status_code: 200,
            headers_size_bytes: 120,
            body_size_bytes: 42,
            ttfb_ms: 8.0,
            total_duration_ms: 12.0,
            redirect_count: 0,
            started_at: Utc::now(),
            response_headers: vec![],
            payload_bytes: 65536,
            throughput_mbps: Some(105.0),
            goodput_mbps: Some(98.0),
            cpu_time_ms: Some(1.2),
            csw_voluntary: Some(4),
            csw_involuntary: Some(1),
        }),
        udp: Some(UdpResult {
            remote_addr: "127.0.0.1:9999".into(),
            probe_count: 5,
            success_count: 4,
            loss_percent: 20.0,
            rtt_min_ms: 0.1,
            rtt_avg_ms: 0.25,
            rtt_p95_ms: 0.4,
            jitter_ms: 0.05,
            started_at: Utc::now(),
            probe_rtts_ms: vec![Some(0.1), Some(0.2), None, Some(0.3), Some(0.4)],
        }),
        error: Some(ErrorRecord {
            category: ErrorCategory::Http,
            message: "simulated error".into(),
            detail: Some("detail text".into()),
            occurred_at: Utc::now(),
        }),
        retry_count: 2,
        server_timing: Some(ServerTimingResult {
            request_id: Some("req-abc-123".into()),
            server_timestamp: Some(Utc::now()),
            clock_skew_ms: Some(0.5),
            recv_body_ms: None,
            processing_ms: Some(3.0),
            total_server_ms: Some(4.0),
            server_version: Some(env!("CARGO_PKG_VERSION").into()),
            srv_csw_voluntary: Some(2),
            srv_csw_involuntary: Some(0),
        }),
        udp_throughput: None,
        page_load: None,
        browser: None,
        http_stack: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests for the fixture helpers themselves
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::Protocol;
    use uuid::Uuid;

    // ── make_run ──────────────────────────────────────────────────────────────

    #[test]
    fn make_run_target_url_and_host() {
        let id = Uuid::new_v4();
        let run = make_run(id, vec![]);
        assert_eq!(run.target_url, "http://localhost/health");
        assert_eq!(run.target_host, "localhost");
    }

    #[test]
    fn make_run_id_preserved() {
        let id = Uuid::new_v4();
        let run = make_run(id, vec![]);
        assert_eq!(run.run_id, id);
    }

    #[test]
    fn make_run_defaults() {
        let id = Uuid::new_v4();
        let run = make_run(id, vec![]);
        assert_eq!(run.modes, vec!["http1"]);
        assert_eq!(run.total_runs, 1);
        assert_eq!(run.concurrency, 1);
        assert_eq!(run.timeout_ms, 5000);
    }

    #[test]
    fn make_run_client_metadata_non_empty() {
        let id = Uuid::new_v4();
        let run = make_run(id, vec![]);
        assert!(!run.client_os.is_empty(), "client_os should be set");
        assert!(
            !run.client_version.is_empty(),
            "client_version should be set"
        );
    }

    #[test]
    fn make_run_optional_fields_none() {
        let id = Uuid::new_v4();
        let run = make_run(id, vec![]);
        assert!(run.server_info.is_none());
        assert!(run.client_info.is_none());
        assert!(run.baseline.is_none());
    }

    #[test]
    fn make_run_attempts_stored() {
        let id = Uuid::new_v4();
        let a = bare_attempt(id);
        let run = make_run(id, vec![a]);
        assert_eq!(run.attempts.len(), 1);
    }

    #[test]
    fn make_run_empty_attempts() {
        let id = Uuid::new_v4();
        let run = make_run(id, vec![]);
        assert!(run.attempts.is_empty());
    }

    #[test]
    fn make_run_success_count_for_bare_attempt() {
        let id = Uuid::new_v4();
        let run = make_run(id, vec![bare_attempt(id)]);
        // bare_attempt has success = true
        assert_eq!(run.success_count(), 1);
        assert_eq!(run.failure_count(), 0);
    }

    // ── bare_attempt ──────────────────────────────────────────────────────────

    #[test]
    fn bare_attempt_run_id_matches() {
        let id = Uuid::new_v4();
        let a = bare_attempt(id);
        assert_eq!(a.run_id, id);
    }

    #[test]
    fn bare_attempt_protocol_is_http1() {
        let id = Uuid::new_v4();
        let a = bare_attempt(id);
        assert_eq!(a.protocol, Protocol::Http1);
    }

    #[test]
    fn bare_attempt_success_true() {
        let id = Uuid::new_v4();
        let a = bare_attempt(id);
        assert!(a.success);
    }

    #[test]
    fn bare_attempt_retry_count_zero() {
        let id = Uuid::new_v4();
        let a = bare_attempt(id);
        assert_eq!(a.retry_count, 0);
    }

    #[test]
    fn bare_attempt_sequence_num_zero() {
        let id = Uuid::new_v4();
        let a = bare_attempt(id);
        assert_eq!(a.sequence_num, 0);
    }

    #[test]
    fn bare_attempt_has_no_dns() {
        let id = Uuid::new_v4();
        assert!(bare_attempt(id).dns.is_none());
    }

    #[test]
    fn bare_attempt_has_no_tcp() {
        let id = Uuid::new_v4();
        assert!(bare_attempt(id).tcp.is_none());
    }

    #[test]
    fn bare_attempt_has_no_tls() {
        let id = Uuid::new_v4();
        assert!(bare_attempt(id).tls.is_none());
    }

    #[test]
    fn bare_attempt_has_no_http() {
        let id = Uuid::new_v4();
        assert!(bare_attempt(id).http.is_none());
    }

    #[test]
    fn bare_attempt_has_no_udp() {
        let id = Uuid::new_v4();
        assert!(bare_attempt(id).udp.is_none());
    }

    #[test]
    fn bare_attempt_has_no_error() {
        let id = Uuid::new_v4();
        assert!(bare_attempt(id).error.is_none());
    }

    #[test]
    fn bare_attempt_has_no_server_timing() {
        let id = Uuid::new_v4();
        assert!(bare_attempt(id).server_timing.is_none());
    }

    #[test]
    fn bare_attempt_has_no_udp_throughput() {
        let id = Uuid::new_v4();
        assert!(bare_attempt(id).udp_throughput.is_none());
    }

    #[test]
    fn bare_attempt_has_no_page_load() {
        let id = Uuid::new_v4();
        assert!(bare_attempt(id).page_load.is_none());
    }

    #[test]
    fn bare_attempt_has_no_browser() {
        let id = Uuid::new_v4();
        assert!(bare_attempt(id).browser.is_none());
    }

    #[test]
    fn bare_attempts_have_distinct_attempt_ids() {
        let id = Uuid::new_v4();
        let a1 = bare_attempt(id);
        let a2 = bare_attempt(id);
        assert_ne!(
            a1.attempt_id, a2.attempt_id,
            "each bare_attempt call should produce a fresh UUID"
        );
    }

    // ── full_attempt ──────────────────────────────────────────────────────────

    #[test]
    fn full_attempt_run_id_matches() {
        let id = Uuid::new_v4();
        let a = full_attempt(id);
        assert_eq!(a.run_id, id);
    }

    #[test]
    fn full_attempt_protocol_is_http1() {
        let id = Uuid::new_v4();
        let a = full_attempt(id);
        assert_eq!(a.protocol, Protocol::Http1);
    }

    #[test]
    fn full_attempt_success_false() {
        // full_attempt models a failed attempt (all sub-results present, success=false)
        let id = Uuid::new_v4();
        let a = full_attempt(id);
        assert!(!a.success, "full_attempt should have success = false");
    }

    #[test]
    fn full_attempt_retry_count_two() {
        let id = Uuid::new_v4();
        let a = full_attempt(id);
        assert_eq!(a.retry_count, 2);
    }

    #[test]
    fn full_attempt_sequence_num_one() {
        let id = Uuid::new_v4();
        let a = full_attempt(id);
        assert_eq!(a.sequence_num, 1);
    }

    #[test]
    fn full_attempt_has_dns() {
        let id = Uuid::new_v4();
        let a = full_attempt(id);
        let dns = a.dns.expect("full_attempt must have DnsResult");
        assert_eq!(dns.query_name, "localhost");
        assert_eq!(dns.resolved_ips, vec!["127.0.0.1"]);
        assert!((dns.duration_ms - 1.5).abs() < 0.001);
        assert!(dns.success);
    }

    #[test]
    fn full_attempt_has_tcp() {
        let id = Uuid::new_v4();
        let a = full_attempt(id);
        let tcp = a.tcp.expect("full_attempt must have TcpResult");
        assert_eq!(tcp.remote_addr, "127.0.0.1:8080");
        assert!(tcp.success);
        assert_eq!(tcp.mss_bytes, Some(1460));
        assert_eq!(tcp.congestion_algorithm.as_deref(), Some("cubic"));
    }

    #[test]
    fn full_attempt_has_tls() {
        let id = Uuid::new_v4();
        let a = full_attempt(id);
        let tls = a.tls.expect("full_attempt must have TlsResult");
        assert_eq!(tls.protocol_version, "TLSv1.3");
        assert_eq!(tls.cipher_suite, "TLS_AES_256_GCM_SHA384");
        assert_eq!(tls.alpn_negotiated.as_deref(), Some("http/1.1"));
    }

    #[test]
    fn full_attempt_has_http() {
        let id = Uuid::new_v4();
        let a = full_attempt(id);
        let http = a.http.expect("full_attempt must have HttpResult");
        assert_eq!(http.status_code, 200);
        assert_eq!(http.negotiated_version, "HTTP/1.1");
        assert!((http.ttfb_ms - 8.0).abs() < 0.001);
        assert!((http.total_duration_ms - 12.0).abs() < 0.001);
    }

    #[test]
    fn full_attempt_has_udp() {
        let id = Uuid::new_v4();
        let a = full_attempt(id);
        let udp = a.udp.expect("full_attempt must have UdpResult");
        assert_eq!(udp.probe_count, 5);
        assert_eq!(udp.success_count, 4);
        assert!((udp.loss_percent - 20.0).abs() < 0.001);
    }

    #[test]
    fn full_attempt_has_error() {
        let id = Uuid::new_v4();
        let a = full_attempt(id);
        let err = a.error.expect("full_attempt must have ErrorRecord");
        assert_eq!(err.message, "simulated error");
        assert_eq!(err.detail.as_deref(), Some("detail text"));
        assert!(
            matches!(err.category, ErrorCategory::Http),
            "error category should be Http"
        );
    }

    #[test]
    fn full_attempt_has_server_timing() {
        let id = Uuid::new_v4();
        let a = full_attempt(id);
        let st = a
            .server_timing
            .expect("full_attempt must have ServerTimingResult");
        assert_eq!(st.request_id.as_deref(), Some("req-abc-123"));
        assert!((st.clock_skew_ms.unwrap() - 0.5).abs() < 0.001);
        assert!((st.processing_ms.unwrap() - 3.0).abs() < 0.001);
        assert!((st.total_server_ms.unwrap() - 4.0).abs() < 0.001);
    }

    #[test]
    fn full_attempts_have_distinct_attempt_ids() {
        let id = Uuid::new_v4();
        let a1 = full_attempt(id);
        let a2 = full_attempt(id);
        assert_ne!(
            a1.attempt_id, a2.attempt_id,
            "each full_attempt call should produce a fresh UUID"
        );
    }

    // ── combined make_run + full_attempt ───────────────────────────────────────

    #[test]
    fn make_run_with_full_attempt_failure_counts() {
        let id = Uuid::new_v4();
        // full_attempt has success = false
        let run = make_run(id, vec![full_attempt(id)]);
        assert_eq!(run.success_count(), 0);
        assert_eq!(run.failure_count(), 1);
    }

    #[test]
    fn make_run_with_mixed_attempts_counts() {
        let id = Uuid::new_v4();
        // bare_attempt = success=true; full_attempt = success=false
        let run = make_run(id, vec![bare_attempt(id), full_attempt(id)]);
        assert_eq!(run.success_count(), 1);
        assert_eq!(run.failure_count(), 1);
    }
}
