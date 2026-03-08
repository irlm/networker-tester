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

    /// Helper: connect, execute a SELECT, return the first row's column values.
    async fn query_one(client: &mut SqlClient, sql: &str) -> Option<tiberius::Row> {
        let stream = client.query(sql, &[]).await.ok()?;
        let row = stream.into_row().await.ok()?;
        row
    }

    /// Helper: connect, execute a SELECT, return all rows.
    async fn query_all(client: &mut SqlClient, sql: &str) -> Vec<tiberius::Row> {
        let stream = client.query(sql, &[]).await.unwrap();
        stream.into_first_result().await.unwrap()
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

    // ── INSERT → SELECT verification tests ─────────────────────────────────

    /// Insert a TestRun then SELECT it back and verify every column.
    #[tokio::test]
    #[ignore = "requires SQL Server – set NETWORKER_SQL_CONN to enable"]
    async fn sql_verify_test_run_fields() {
        let conn_str = match sql_conn() {
            Some(c) => c,
            None => return,
        };
        let run_id = Uuid::new_v4();
        let attempt = bare_attempt(run_id);
        let run = make_run(run_id, vec![attempt]);
        save(&run, &conn_str).await.unwrap();

        let mut client = connect(&conn_str).await.unwrap();
        let sql = format!(
            "SELECT RunId, TargetUrl, TargetHost, Modes, TotalRuns, \
             Concurrency, TimeoutMs, ClientOs, ClientVersion, \
             SuccessCount, FailureCount \
             FROM dbo.TestRun WHERE RunId = '{}'",
            run_id
        );
        let row = query_one(&mut client, &sql)
            .await
            .expect("TestRun row must exist");

        let db_run_id: &str = row.get(0).unwrap();
        assert_eq!(db_run_id, run_id.to_string());
        let db_url: &str = row.get(1).unwrap();
        assert_eq!(db_url, "http://localhost/health");
        let db_host: &str = row.get(2).unwrap();
        assert_eq!(db_host, "localhost");
        let db_modes: &str = row.get(3).unwrap();
        assert_eq!(db_modes, "http1");
        let db_total: i32 = row.get(4).unwrap();
        assert_eq!(db_total, 1);
        let db_conc: i32 = row.get(5).unwrap();
        assert_eq!(db_conc, 1);
        let db_timeout: i64 = row.get(6).unwrap();
        assert_eq!(db_timeout, 5000);
        let db_os: &str = row.get(7).unwrap();
        assert_eq!(db_os, std::env::consts::OS);
        let db_version: &str = row.get(8).unwrap();
        assert_eq!(db_version, env!("CARGO_PKG_VERSION"));
        let db_success: i32 = row.get(9).unwrap();
        assert_eq!(db_success, 1); // one successful bare attempt
        let db_fail: i32 = row.get(10).unwrap();
        assert_eq!(db_fail, 0);
    }

    /// Insert a full attempt then SELECT back each sub-result table row.
    #[tokio::test]
    #[ignore = "requires SQL Server – set NETWORKER_SQL_CONN to enable"]
    async fn sql_verify_all_sub_results() {
        let conn_str = match sql_conn() {
            Some(c) => c,
            None => return,
        };
        let run_id = Uuid::new_v4();
        let attempt = full_attempt(run_id);
        let attempt_id = attempt.attempt_id;
        let run = make_run(run_id, vec![attempt]);
        save(&run, &conn_str).await.unwrap();

        let mut c = connect(&conn_str).await.unwrap();
        let aid = attempt_id.to_string();

        // ── RequestAttempt ──────────────────────────────────────────────────
        let row = query_one(
            &mut c,
            &format!(
                "SELECT Protocol, SequenceNum, Success, RetryCount \
                 FROM dbo.RequestAttempt WHERE AttemptId = '{aid}'"
            ),
        )
        .await
        .expect("RequestAttempt row");
        let proto: &str = row.get(0).unwrap();
        assert_eq!(proto, "http1");
        let seq: i32 = row.get(1).unwrap();
        assert_eq!(seq, 1);
        let success: bool = row.get(2).unwrap();
        assert!(!success); // full_attempt has success=false
        let retry: i32 = row.get(3).unwrap();
        assert_eq!(retry, 2);

        // ── DnsResult ───────────────────────────────────────────────────────
        let row = query_one(
            &mut c,
            &format!(
                "SELECT QueryName, ResolvedIPs, DurationMs, Success \
                 FROM dbo.DnsResult WHERE AttemptId = '{aid}'"
            ),
        )
        .await
        .expect("DnsResult row");
        let qname: &str = row.get(0).unwrap();
        assert_eq!(qname, "localhost");
        let ips: &str = row.get(1).unwrap();
        assert_eq!(ips, "127.0.0.1");
        let dur: f64 = row.get(2).unwrap();
        assert!((dur - 1.5).abs() < 0.01);
        let dns_ok: bool = row.get(3).unwrap();
        assert!(dns_ok);

        // ── TcpResult ───────────────────────────────────────────────────────
        let row = query_one(
            &mut c,
            &format!(
                "SELECT RemoteAddr, ConnectDurationMs, MssBytesEstimate, \
                 RttEstimateMs, CongestionAlgorithm, DeliveryRateBps, MinRttMs \
                 FROM dbo.TcpResult WHERE AttemptId = '{aid}'"
            ),
        )
        .await
        .expect("TcpResult row");
        let remote: &str = row.get(0).unwrap();
        assert_eq!(remote, "127.0.0.1:8080");
        let connect_ms: f64 = row.get(1).unwrap();
        assert!((connect_ms - 0.5).abs() < 0.01);
        let mss: Option<i32> = row.get(2);
        assert_eq!(mss, Some(1460));
        let rtt: Option<f64> = row.get(3);
        assert!((rtt.unwrap() - 0.3).abs() < 0.01);
        let algo: Option<&str> = row.get(4);
        assert_eq!(algo, Some("cubic"));
        let delivery: Option<i64> = row.get(5);
        assert_eq!(delivery, Some(1_000_000));
        let min_rtt: Option<f64> = row.get(6);
        assert!((min_rtt.unwrap() - 0.2).abs() < 0.01);

        // ── TlsResult ───────────────────────────────────────────────────────
        let row = query_one(
            &mut c,
            &format!(
                "SELECT ProtocolVersion, CipherSuite, AlpnNegotiated, \
                 CertSubject, HandshakeDurationMs \
                 FROM dbo.TlsResult WHERE AttemptId = '{aid}'"
            ),
        )
        .await
        .expect("TlsResult row");
        let ver: &str = row.get(0).unwrap();
        assert_eq!(ver, "TLSv1.3");
        let cipher: &str = row.get(1).unwrap();
        assert_eq!(cipher, "TLS_AES_256_GCM_SHA384");
        let alpn: Option<&str> = row.get(2);
        assert_eq!(alpn, Some("http/1.1"));
        let subj: Option<&str> = row.get(3);
        assert_eq!(subj, Some("CN=localhost"));
        let hs_ms: f64 = row.get(4).unwrap();
        assert!((hs_ms - 5.0).abs() < 0.01);

        // ── HttpResult ──────────────────────────────────────────────────────
        let row = query_one(
            &mut c,
            &format!(
                "SELECT NegotiatedVersion, StatusCode, TtfbMs, TotalDurationMs, \
                 PayloadBytes, ThroughputMbps \
                 FROM dbo.HttpResult WHERE AttemptId = '{aid}'"
            ),
        )
        .await
        .expect("HttpResult row");
        let http_ver: &str = row.get(0).unwrap();
        assert_eq!(http_ver, "HTTP/1.1");
        let status: i32 = row.get(1).unwrap();
        assert_eq!(status, 200);
        let ttfb: f64 = row.get(2).unwrap();
        assert!((ttfb - 8.0).abs() < 0.01);
        let total: f64 = row.get(3).unwrap();
        assert!((total - 12.0).abs() < 0.01);
        let payload: Option<i64> = row.get(4);
        assert_eq!(payload, Some(65536));
        let tput: Option<f64> = row.get(5);
        assert!((tput.unwrap() - 105.0).abs() < 0.01);

        // ── UdpResult ───────────────────────────────────────────────────────
        let row = query_one(
            &mut c,
            &format!(
                "SELECT ProbeCount, SuccessCount, LossPercent, \
                 RttMinMs, RttAvgMs, RttP95Ms, JitterMs \
                 FROM dbo.UdpResult WHERE AttemptId = '{aid}'"
            ),
        )
        .await
        .expect("UdpResult row");
        let probes: i32 = row.get(0).unwrap();
        assert_eq!(probes, 5);
        let successes: i32 = row.get(1).unwrap();
        assert_eq!(successes, 4);
        let loss: f64 = row.get(2).unwrap();
        assert!((loss - 20.0).abs() < 0.01);
        let rtt_min: f64 = row.get(3).unwrap();
        assert!((rtt_min - 0.1).abs() < 0.01);
        let rtt_avg: f64 = row.get(4).unwrap();
        assert!((rtt_avg - 0.25).abs() < 0.01);
        let rtt_p95: f64 = row.get(5).unwrap();
        assert!((rtt_p95 - 0.4).abs() < 0.01);
        let jitter: f64 = row.get(6).unwrap();
        assert!((jitter - 0.05).abs() < 0.01);

        // ── ErrorRecord ─────────────────────────────────────────────────────
        let row = query_one(
            &mut c,
            &format!(
                "SELECT ErrorCategory, ErrorMessage, ErrorDetail \
                 FROM dbo.ErrorRecord WHERE AttemptId = '{aid}'"
            ),
        )
        .await
        .expect("ErrorRecord row");
        let cat: &str = row.get(0).unwrap();
        assert_eq!(cat, "http");
        let msg: &str = row.get(1).unwrap();
        assert_eq!(msg, "simulated error");
        let detail: Option<&str> = row.get(2);
        assert_eq!(detail, Some("detail text"));

        // ── ServerTimingResult ──────────────────────────────────────────────
        let row = query_one(
            &mut c,
            &format!(
                "SELECT RequestId, ClockSkewMs, ProcessingMs, TotalServerMs \
                 FROM dbo.ServerTimingResult WHERE AttemptId = '{aid}'"
            ),
        )
        .await
        .expect("ServerTimingResult row");
        let req_id: Option<&str> = row.get(0);
        assert_eq!(req_id, Some("req-abc-123"));
        let skew: Option<f64> = row.get(1);
        assert!((skew.unwrap() - 0.5).abs() < 0.01);
        let proc_ms: Option<f64> = row.get(2);
        assert!((proc_ms.unwrap() - 3.0).abs() < 0.01);
        let total_srv: Option<f64> = row.get(3);
        assert!((total_srv.unwrap() - 4.0).abs() < 0.01);
    }

    /// Verify CASCADE DELETE: deleting a TestRun removes all child rows.
    #[tokio::test]
    #[ignore = "requires SQL Server – set NETWORKER_SQL_CONN to enable"]
    async fn sql_cascade_delete() {
        let conn_str = match sql_conn() {
            Some(c) => c,
            None => return,
        };
        let run_id = Uuid::new_v4();
        let attempt = full_attempt(run_id);
        let attempt_id = attempt.attempt_id;
        let run = make_run(run_id, vec![attempt]);
        save(&run, &conn_str).await.unwrap();

        let mut c = connect(&conn_str).await.unwrap();
        let rid = run_id.to_string();
        let aid = attempt_id.to_string();

        // Verify rows exist before delete.
        let rows = query_all(
            &mut c,
            &format!("SELECT 1 FROM dbo.RequestAttempt WHERE RunId = '{rid}'"),
        )
        .await;
        assert!(!rows.is_empty(), "attempt should exist before delete");

        // ErrorRecord has FK to TestRun with ON DELETE NO ACTION, so delete
        // error rows first to avoid FK violation.
        c.execute(
            &format!("DELETE FROM dbo.ErrorRecord WHERE AttemptId = '{aid}'") as &str,
            &[],
        )
        .await
        .unwrap();

        // Delete TestRun — should CASCADE to RequestAttempt → DnsResult, TcpResult, etc.
        c.execute(
            &format!("DELETE FROM dbo.TestRun WHERE RunId = '{rid}'") as &str,
            &[],
        )
        .await
        .unwrap();

        // Verify all child rows are gone.
        let rows = query_all(
            &mut c,
            &format!("SELECT 1 FROM dbo.RequestAttempt WHERE RunId = '{rid}'"),
        )
        .await;
        assert!(rows.is_empty(), "attempts should be cascade-deleted");

        let rows = query_all(
            &mut c,
            &format!("SELECT 1 FROM dbo.DnsResult WHERE AttemptId = '{aid}'"),
        )
        .await;
        assert!(rows.is_empty(), "DNS results should be cascade-deleted");

        let rows = query_all(
            &mut c,
            &format!("SELECT 1 FROM dbo.HttpResult WHERE AttemptId = '{aid}'"),
        )
        .await;
        assert!(rows.is_empty(), "HTTP results should be cascade-deleted");
    }

    /// Verify duplicate RunId insertion fails (PK constraint).
    #[tokio::test]
    #[ignore = "requires SQL Server – set NETWORKER_SQL_CONN to enable"]
    async fn sql_duplicate_run_id_fails() {
        let conn_str = match sql_conn() {
            Some(c) => c,
            None => return,
        };
        let run_id = Uuid::new_v4();
        let run = make_run(run_id, vec![bare_attempt(run_id)]);
        save(&run, &conn_str).await.unwrap();

        // Second insert with same RunId should fail on PK.
        let run2 = make_run(run_id, vec![bare_attempt(run_id)]);
        let err = save(&run2, &conn_str).await;
        assert!(err.is_err(), "duplicate RunId should fail");
    }

    /// Insert multiple attempts in one run, verify correct count.
    #[tokio::test]
    #[ignore = "requires SQL Server – set NETWORKER_SQL_CONN to enable"]
    async fn sql_multiple_attempts_count() {
        let conn_str = match sql_conn() {
            Some(c) => c,
            None => return,
        };
        let run_id = Uuid::new_v4();
        let attempts = vec![
            bare_attempt(run_id),
            bare_attempt(run_id),
            full_attempt(run_id),
        ];
        let mut run = make_run(run_id, attempts);
        run.total_runs = 3;
        save(&run, &conn_str).await.unwrap();

        let mut c = connect(&conn_str).await.unwrap();
        let rid = run_id.to_string();

        // 3 RequestAttempt rows
        let rows = query_all(
            &mut c,
            &format!("SELECT 1 FROM dbo.RequestAttempt WHERE RunId = '{rid}'"),
        )
        .await;
        assert_eq!(rows.len(), 3);

        // Only the full_attempt has sub-results — 1 DNS, 1 TCP, 1 TLS, 1 HTTP, 1 UDP
        let dns_rows = query_all(
            &mut c,
            &format!(
                "SELECT 1 FROM dbo.DnsResult d \
                 JOIN dbo.RequestAttempt a ON d.AttemptId = a.AttemptId \
                 WHERE a.RunId = '{rid}'"
            ),
        )
        .await;
        assert_eq!(dns_rows.len(), 1);

        let http_rows = query_all(
            &mut c,
            &format!(
                "SELECT 1 FROM dbo.HttpResult h \
                 JOIN dbo.RequestAttempt a ON h.AttemptId = a.AttemptId \
                 WHERE a.RunId = '{rid}'"
            ),
        )
        .await;
        assert_eq!(http_rows.len(), 1);
    }

    /// Verify NULL-heavy attempt (bare with no sub-results) doesn't leave
    /// orphan rows in child tables.
    #[tokio::test]
    #[ignore = "requires SQL Server – set NETWORKER_SQL_CONN to enable"]
    async fn sql_bare_attempt_no_child_rows() {
        let conn_str = match sql_conn() {
            Some(c) => c,
            None => return,
        };
        let run_id = Uuid::new_v4();
        let attempt = bare_attempt(run_id);
        let aid = attempt.attempt_id.to_string();
        let run = make_run(run_id, vec![attempt]);
        save(&run, &conn_str).await.unwrap();

        let mut c = connect(&conn_str).await.unwrap();
        for table in &[
            "DnsResult",
            "TcpResult",
            "TlsResult",
            "HttpResult",
            "UdpResult",
            "ErrorRecord",
            "ServerTimingResult",
        ] {
            let rows = query_all(
                &mut c,
                &format!("SELECT 1 FROM dbo.{table} WHERE AttemptId = '{aid}'"),
            )
            .await;
            assert!(rows.is_empty(), "bare attempt should have no {table} rows");
        }
    }
}
