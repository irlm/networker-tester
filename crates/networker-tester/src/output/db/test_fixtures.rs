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
    }
}

/// Constructs an attempt with all 7 sub-result types populated.
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
    }
}
