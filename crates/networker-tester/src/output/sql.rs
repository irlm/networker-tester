/// SQL Server persistence layer using `tiberius`.
///
/// Schema targets the tables defined in `sql/01_CreateDatabase.sql`.
/// Parameterized inserts are used throughout; no stored procedures required
/// (SPs are provided separately in `sql/02_StoredProcedures.sql` as optional).
///
/// # Connection
/// Pass an ADO.NET-style connection string, e.g.:
///   "Server=localhost;Database=NetworkDiagnostics;User Id=sa;Password=Pass!;TrustServerCertificate=true"
use crate::metrics::{RequestAttempt, TestRun};
use anyhow::Context;
use tiberius::{Client, Config, Query};
use tokio::net::TcpStream;
use tokio_util::compat::TokioAsyncWriteCompatExt;

type SqlClient = Client<tokio_util::compat::Compat<TcpStream>>;

// ─────────────────────────────────────────────────────────────────────────────
// Public entry point
// ─────────────────────────────────────────────────────────────────────────────

/// Insert the entire test run (header + all attempts + sub-results) into SQL.
pub async fn save(run: &TestRun, connection_string: &str) -> anyhow::Result<()> {
    let mut client = connect(connection_string)
        .await
        .context("SQL Server connection failed")?;

    insert_test_run(run, &mut client).await?;

    for attempt in &run.attempts {
        insert_request_attempt(attempt, &mut client).await?;

        if let Some(dns) = &attempt.dns {
            insert_dns_result(attempt, dns, &mut client).await?;
        }
        if let Some(tcp) = &attempt.tcp {
            insert_tcp_result(attempt, tcp, &mut client).await?;
        }
        if let Some(tls) = &attempt.tls {
            insert_tls_result(attempt, tls, &mut client).await?;
        }
        if let Some(http) = &attempt.http {
            insert_http_result(attempt, http, &mut client).await?;
        }
        if let Some(udp) = &attempt.udp {
            insert_udp_result(attempt, udp, &mut client).await?;
        }
        if let Some(err) = &attempt.error {
            insert_error(attempt, err, &mut client).await?;
        }
        if let Some(st) = &attempt.server_timing {
            insert_server_timing_result(attempt, st, &mut client).await?;
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Connection
// ─────────────────────────────────────────────────────────────────────────────

async fn connect(conn_str: &str) -> anyhow::Result<SqlClient> {
    let config = Config::from_ado_string(conn_str).context("Failed to parse connection string")?;
    let tcp = TcpStream::connect(config.get_addr())
        .await
        .context("TCP connect to SQL Server")?;
    tcp.set_nodelay(true)?;
    let client = Client::connect(config, tcp.compat_write())
        .await
        .context("SQL Server handshake")?;
    Ok(client)
}

// ─────────────────────────────────────────────────────────────────────────────
// Insert helpers
// ─────────────────────────────────────────────────────────────────────────────

async fn insert_test_run(run: &TestRun, c: &mut SqlClient) -> anyhow::Result<()> {
    // Bind temporary Strings to local vars so they live long enough.
    let run_id = run.run_id.to_string();
    let modes = run.modes.join(",");
    let started = run.started_at.naive_utc();
    let finished = run.finished_at.map(|t| t.naive_utc());

    let mut q = Query::new(
        "INSERT INTO dbo.TestRun (
            RunId, StartedAt, FinishedAt, TargetUrl, TargetHost,
            Modes, TotalRuns, Concurrency, TimeoutMs,
            ClientOs, ClientVersion, SuccessCount, FailureCount
         ) VALUES (
            @P1,@P2,@P3,@P4,@P5,@P6,@P7,@P8,@P9,@P10,@P11,@P12,@P13
         )",
    );
    q.bind(run_id.as_str());
    q.bind(started);
    q.bind(finished);
    q.bind(run.target_url.as_str());
    q.bind(run.target_host.as_str());
    q.bind(modes.as_str());
    q.bind(run.total_runs as i32);
    q.bind(run.concurrency as i32);
    q.bind(run.timeout_ms as i64);
    q.bind(run.client_os.as_str());
    q.bind(run.client_version.as_str());
    q.bind(run.success_count() as i32);
    q.bind(run.failure_count() as i32);
    q.execute(c).await.context("INSERT TestRun")?;
    Ok(())
}

async fn insert_request_attempt(a: &RequestAttempt, c: &mut SqlClient) -> anyhow::Result<()> {
    let attempt_id = a.attempt_id.to_string();
    let run_id = a.run_id.to_string();
    let protocol = a.protocol.to_string();
    let started = a.started_at.naive_utc();
    let finished = a.finished_at.map(|t| t.naive_utc());
    let err_msg = a.error.as_ref().map(|e| e.message.as_str());

    let mut q = Query::new(
        "INSERT INTO dbo.RequestAttempt (
            AttemptId, RunId, Protocol, SequenceNum,
            StartedAt, FinishedAt, Success, ErrorMessage, RetryCount
         ) VALUES (@P1,@P2,@P3,@P4,@P5,@P6,@P7,@P8,@P9)",
    );
    q.bind(attempt_id.as_str());
    q.bind(run_id.as_str());
    q.bind(protocol.as_str());
    q.bind(a.sequence_num as i32);
    q.bind(started);
    q.bind(finished);
    q.bind(a.success);
    q.bind(err_msg);
    q.bind(a.retry_count as i32);
    q.execute(c).await.context("INSERT RequestAttempt")?;
    Ok(())
}

async fn insert_dns_result(
    a: &RequestAttempt,
    dns: &crate::metrics::DnsResult,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    let attempt_id = a.attempt_id.to_string();
    let ips = dns.resolved_ips.join(",");
    let started = dns.started_at.naive_utc();

    let mut q = Query::new(
        "INSERT INTO dbo.DnsResult (
            DnsId, AttemptId, QueryName, ResolvedIPs,
            DurationMs, StartedAt, Success
         ) VALUES (@P1,@P2,@P3,@P4,@P5,@P6,@P7)",
    );
    q.bind(id.as_str());
    q.bind(attempt_id.as_str());
    q.bind(dns.query_name.as_str());
    q.bind(ips.as_str());
    q.bind(dns.duration_ms);
    q.bind(started);
    q.bind(dns.success);
    q.execute(c).await.context("INSERT DnsResult")?;
    Ok(())
}

async fn insert_tcp_result(
    a: &RequestAttempt,
    tcp: &crate::metrics::TcpResult,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    let attempt_id = a.attempt_id.to_string();
    let started = tcp.started_at.naive_utc();

    let mut q = Query::new(
        "INSERT INTO dbo.TcpResult (
            TcpId, AttemptId, LocalAddr, RemoteAddr,
            ConnectDurationMs, AttemptCount, StartedAt, Success,
            MssBytesEstimate, RttEstimateMs,
            Retransmits, TotalRetrans, SndCwnd, SndSsthresh,
            RttVarianceMs, RcvSpace, SegsOut, SegsIn,
            CongestionAlgorithm, DeliveryRateBps, MinRttMs
         ) VALUES (@P1,@P2,@P3,@P4,@P5,@P6,@P7,@P8,@P9,@P10,
                   @P11,@P12,@P13,@P14,@P15,@P16,@P17,@P18,
                   @P19,@P20,@P21)",
    );
    q.bind(id.as_str());
    q.bind(attempt_id.as_str());
    q.bind(tcp.local_addr.as_deref());
    q.bind(tcp.remote_addr.as_str());
    q.bind(tcp.connect_duration_ms);
    q.bind(tcp.attempt_count as i32);
    q.bind(started);
    q.bind(tcp.success);
    q.bind(tcp.mss_bytes.map(|v| v as i32));
    q.bind(tcp.rtt_estimate_ms);
    // Extended kernel stats (nullable)
    q.bind(tcp.retransmits.map(|v| v as i64));
    q.bind(tcp.total_retrans.map(|v| v as i64));
    q.bind(tcp.snd_cwnd.map(|v| v as i64));
    q.bind(tcp.snd_ssthresh.map(|v| v as i64));
    q.bind(tcp.rtt_variance_ms);
    q.bind(tcp.rcv_space.map(|v| v as i64));
    q.bind(tcp.segs_out.map(|v| v as i64));
    q.bind(tcp.segs_in.map(|v| v as i64));
    // New fields (07_MoreTcpStats.sql)
    q.bind(tcp.congestion_algorithm.as_deref());
    q.bind(tcp.delivery_rate_bps.map(|v| v as i64));
    q.bind(tcp.min_rtt_ms);
    q.execute(c).await.context("INSERT TcpResult")?;
    Ok(())
}

async fn insert_server_timing_result(
    a: &RequestAttempt,
    st: &crate::metrics::ServerTimingResult,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    let attempt_id = a.attempt_id.to_string();
    let server_ts = st.server_timestamp.map(|t| t.naive_utc());

    let mut q = Query::new(
        "INSERT INTO dbo.ServerTimingResult (
            ServerId, AttemptId, RequestId, ServerTimestamp,
            ClockSkewMs, RecvBodyMs, ProcessingMs, TotalServerMs
         ) VALUES (@P1,@P2,@P3,@P4,@P5,@P6,@P7,@P8)",
    );
    q.bind(id.as_str());
    q.bind(attempt_id.as_str());
    q.bind(st.request_id.as_deref());
    q.bind(server_ts);
    q.bind(st.clock_skew_ms);
    q.bind(st.recv_body_ms);
    q.bind(st.processing_ms);
    q.bind(st.total_server_ms);
    q.execute(c).await.context("INSERT ServerTimingResult")?;
    Ok(())
}

async fn insert_tls_result(
    a: &RequestAttempt,
    tls: &crate::metrics::TlsResult,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    let attempt_id = a.attempt_id.to_string();
    let started = tls.started_at.naive_utc();
    let expiry = tls.cert_expiry.map(|t| t.naive_utc());

    let mut q = Query::new(
        "INSERT INTO dbo.TlsResult (
            TlsId, AttemptId, ProtocolVersion, CipherSuite,
            AlpnNegotiated, CertSubject, CertIssuer, CertExpiry,
            HandshakeDurationMs, StartedAt, Success
         ) VALUES (@P1,@P2,@P3,@P4,@P5,@P6,@P7,@P8,@P9,@P10,@P11)",
    );
    q.bind(id.as_str());
    q.bind(attempt_id.as_str());
    q.bind(tls.protocol_version.as_str());
    q.bind(tls.cipher_suite.as_str());
    q.bind(tls.alpn_negotiated.as_deref());
    q.bind(tls.cert_subject.as_deref());
    q.bind(tls.cert_issuer.as_deref());
    q.bind(expiry);
    q.bind(tls.handshake_duration_ms);
    q.bind(started);
    q.bind(tls.success);
    q.execute(c).await.context("INSERT TlsResult")?;
    Ok(())
}

async fn insert_http_result(
    a: &RequestAttempt,
    http: &crate::metrics::HttpResult,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    let attempt_id = a.attempt_id.to_string();
    let started = http.started_at.naive_utc();

    let mut q = Query::new(
        "INSERT INTO dbo.HttpResult (
            HttpId, AttemptId, NegotiatedVersion, StatusCode,
            HeadersSizeBytes, BodySizeBytes, TtfbMs,
            TotalDurationMs, RedirectCount, StartedAt,
            PayloadBytes, ThroughputMbps
         ) VALUES (@P1,@P2,@P3,@P4,@P5,@P6,@P7,@P8,@P9,@P10,@P11,@P12)",
    );
    q.bind(id.as_str());
    q.bind(attempt_id.as_str());
    q.bind(http.negotiated_version.as_str());
    q.bind(http.status_code as i32);
    q.bind(http.headers_size_bytes as i32);
    q.bind(http.body_size_bytes as i32);
    q.bind(http.ttfb_ms);
    q.bind(http.total_duration_ms);
    q.bind(http.redirect_count as i32);
    q.bind(started);
    q.bind(http.payload_bytes as i64);
    q.bind(http.throughput_mbps);
    q.execute(c).await.context("INSERT HttpResult")?;
    Ok(())
}

async fn insert_udp_result(
    a: &RequestAttempt,
    udp: &crate::metrics::UdpResult,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    let attempt_id = a.attempt_id.to_string();
    let started = udp.started_at.naive_utc();

    let mut q = Query::new(
        "INSERT INTO dbo.UdpResult (
            UdpId, AttemptId, RemoteAddr, ProbeCount,
            SuccessCount, LossPercent, RttMinMs, RttAvgMs,
            RttP95Ms, JitterMs, StartedAt
         ) VALUES (@P1,@P2,@P3,@P4,@P5,@P6,@P7,@P8,@P9,@P10,@P11)",
    );
    q.bind(id.as_str());
    q.bind(attempt_id.as_str());
    q.bind(udp.remote_addr.as_str());
    q.bind(udp.probe_count as i32);
    q.bind(udp.success_count as i32);
    q.bind(udp.loss_percent);
    q.bind(udp.rtt_min_ms);
    q.bind(udp.rtt_avg_ms);
    q.bind(udp.rtt_p95_ms);
    q.bind(udp.jitter_ms);
    q.bind(started);
    q.execute(c).await.context("INSERT UdpResult")?;
    Ok(())
}

async fn insert_error(
    a: &RequestAttempt,
    err: &crate::metrics::ErrorRecord,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    let attempt_id = a.attempt_id.to_string();
    let run_id = a.run_id.to_string();
    let category = err.category.to_string();
    let occurred = err.occurred_at.naive_utc();

    let mut q = Query::new(
        "INSERT INTO dbo.ErrorRecord (
            ErrorId, AttemptId, RunId, ErrorCategory, ErrorMessage, ErrorDetail, OccurredAt
         ) VALUES (@P1,@P2,@P3,@P4,@P5,@P6,@P7)",
    );
    q.bind(id.as_str());
    q.bind(attempt_id.as_str());
    q.bind(run_id.as_str());
    q.bind(category.as_str());
    q.bind(err.message.as_str());
    q.bind(err.detail.as_deref());
    q.bind(occurred);
    q.execute(c).await.context("INSERT ErrorRecord")?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{
        DnsResult, ErrorCategory, ErrorRecord, HttpResult, Protocol, RequestAttempt,
        ServerTimingResult, TcpResult, TestRun, TlsResult, UdpResult,
    };
    use chrono::Utc;
    use uuid::Uuid;

    /// Returns `NETWORKER_SQL_CONN` or skips the test (returns None) if unset.
    fn sql_conn() -> Option<String> {
        std::env::var("NETWORKER_SQL_CONN").ok()
    }

    fn make_run(run_id: Uuid, attempts: Vec<RequestAttempt>) -> TestRun {
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

    fn bare_attempt(run_id: Uuid) -> RequestAttempt {
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

    /// Inserts a run with all 7 sub-result types populated — exercises every
    /// insert helper (insert_dns_result, insert_tcp_result, insert_tls_result,
    /// insert_http_result, insert_udp_result, insert_error,
    /// insert_server_timing_result).
    fn full_attempt(run_id: Uuid) -> RequestAttempt {
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

    /// Basic round-trip: TestRun + bare RequestAttempt (no sub-results).
    #[tokio::test]
    #[ignore = "requires SQL Server – set NETWORKER_SQL_CONN to enable"]
    async fn sql_insert_round_trip() {
        let conn = match sql_conn() {
            Some(c) => c,
            None => return,
        };
        let run_id = Uuid::new_v4();
        let run = make_run(run_id, vec![bare_attempt(run_id)]);
        save(&run, &conn).await.expect("SQL save should succeed");
    }

    /// Full round-trip: exercises every sub-result insert helper by populating
    /// dns / tcp / tls / http / udp / error / server_timing on the attempt.
    #[tokio::test]
    #[ignore = "requires SQL Server – set NETWORKER_SQL_CONN to enable"]
    async fn sql_full_round_trip() {
        let conn = match sql_conn() {
            Some(c) => c,
            None => return,
        };
        let run_id = Uuid::new_v4();
        let run = make_run(run_id, vec![bare_attempt(run_id), full_attempt(run_id)]);
        save(&run, &conn)
            .await
            .expect("SQL full save should succeed");
    }
}
