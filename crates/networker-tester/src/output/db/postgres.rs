/// PostgreSQL backend using `tokio-postgres`.
///
/// Schema mirrors the SQL Server tables (same column names, equivalent types).
/// Migration is embedded and tracked via a `_schema_versions` table.
use super::DatabaseBackend;
use crate::metrics::{RequestAttempt, TestRun};
use anyhow::Context;
use async_trait::async_trait;
use tokio_postgres::Client as PgClient;

/// PostgreSQL database backend.
pub struct PostgresBackend {
    client: PgClient,
}

impl PostgresBackend {
    /// Connect to PostgreSQL using a `postgres://` URL.
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let (client, connection) = tokio_postgres::connect(url, tokio_postgres::NoTls)
            .await
            .context("PostgreSQL connection failed")?;

        // Spawn the connection future — it drives the actual I/O.
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::error!("PostgreSQL connection error: {e}");
            }
        });

        Ok(Self { client })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Embedded migration SQL
// ─────────────────────────────────────────────────────────────────────────────

const V001_MIGRATION: &str = r#"
-- V001: Create all tables + indexes for NetworkDiagnostics

CREATE TABLE IF NOT EXISTS TestRun (
    RunId          UUID            NOT NULL,
    StartedAt      TIMESTAMPTZ     NOT NULL,
    FinishedAt     TIMESTAMPTZ     NULL,
    TargetUrl      VARCHAR(2048)   NOT NULL,
    TargetHost     VARCHAR(255)    NOT NULL,
    Modes          VARCHAR(200)    NOT NULL,
    TotalRuns      INT             NOT NULL DEFAULT 1,
    Concurrency    INT             NOT NULL DEFAULT 1,
    TimeoutMs      BIGINT          NOT NULL DEFAULT 30000,
    ClientOs       VARCHAR(50)     NOT NULL,
    ClientVersion  VARCHAR(50)     NOT NULL,
    SuccessCount   INT             NOT NULL DEFAULT 0,
    FailureCount   INT             NOT NULL DEFAULT 0,
    CONSTRAINT PK_TestRun PRIMARY KEY (RunId)
);

CREATE TABLE IF NOT EXISTS RequestAttempt (
    AttemptId     UUID            NOT NULL,
    RunId         UUID            NOT NULL,
    Protocol      VARCHAR(20)     NOT NULL,
    SequenceNum   INT             NOT NULL,
    StartedAt     TIMESTAMPTZ     NOT NULL,
    FinishedAt    TIMESTAMPTZ     NULL,
    Success       BOOLEAN         NOT NULL DEFAULT FALSE,
    ErrorMessage  TEXT            NULL,
    RetryCount    INT             NOT NULL DEFAULT 0,
    CONSTRAINT PK_RequestAttempt PRIMARY KEY (AttemptId),
    CONSTRAINT FK_Attempt_Run    FOREIGN KEY (RunId)
        REFERENCES TestRun (RunId) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS DnsResult (
    DnsId       UUID              NOT NULL,
    AttemptId   UUID              NOT NULL,
    QueryName   VARCHAR(255)      NOT NULL,
    ResolvedIPs VARCHAR(1024)     NOT NULL,
    DurationMs  DOUBLE PRECISION  NOT NULL,
    StartedAt   TIMESTAMPTZ       NOT NULL,
    Success     BOOLEAN           NOT NULL,
    CONSTRAINT PK_DnsResult   PRIMARY KEY (DnsId),
    CONSTRAINT FK_Dns_Attempt FOREIGN KEY (AttemptId)
        REFERENCES RequestAttempt (AttemptId) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS TcpResult (
    TcpId              UUID              NOT NULL,
    AttemptId          UUID              NOT NULL,
    LocalAddr          VARCHAR(50)       NULL,
    RemoteAddr         VARCHAR(50)       NOT NULL,
    ConnectDurationMs  DOUBLE PRECISION  NOT NULL,
    AttemptCount       INT               NOT NULL DEFAULT 1,
    StartedAt          TIMESTAMPTZ       NOT NULL,
    Success            BOOLEAN           NOT NULL,
    MssBytesEstimate   INT               NULL,
    RttEstimateMs      DOUBLE PRECISION  NULL,
    Retransmits        BIGINT            NULL,
    TotalRetrans       BIGINT            NULL,
    SndCwnd            BIGINT            NULL,
    SndSsthresh        BIGINT            NULL,
    RttVarianceMs      DOUBLE PRECISION  NULL,
    RcvSpace           BIGINT            NULL,
    SegsOut            BIGINT            NULL,
    SegsIn             BIGINT            NULL,
    CongestionAlgorithm VARCHAR(32)      NULL,
    DeliveryRateBps    BIGINT            NULL,
    MinRttMs           DOUBLE PRECISION  NULL,
    CONSTRAINT PK_TcpResult    PRIMARY KEY (TcpId),
    CONSTRAINT FK_Tcp_Attempt  FOREIGN KEY (AttemptId)
        REFERENCES RequestAttempt (AttemptId) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS TlsResult (
    TlsId                UUID              NOT NULL,
    AttemptId            UUID              NOT NULL,
    ProtocolVersion      VARCHAR(20)       NOT NULL,
    CipherSuite          VARCHAR(100)      NOT NULL,
    AlpnNegotiated       VARCHAR(50)       NULL,
    CertSubject          VARCHAR(500)      NULL,
    CertIssuer           VARCHAR(500)      NULL,
    CertExpiry           TIMESTAMPTZ       NULL,
    HandshakeDurationMs  DOUBLE PRECISION  NOT NULL,
    StartedAt            TIMESTAMPTZ       NOT NULL,
    Success              BOOLEAN           NOT NULL,
    CONSTRAINT PK_TlsResult    PRIMARY KEY (TlsId),
    CONSTRAINT FK_Tls_Attempt  FOREIGN KEY (AttemptId)
        REFERENCES RequestAttempt (AttemptId) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS HttpResult (
    HttpId              UUID              NOT NULL,
    AttemptId           UUID              NOT NULL,
    NegotiatedVersion   VARCHAR(20)       NOT NULL,
    StatusCode          INT               NOT NULL,
    HeadersSizeBytes    INT               NOT NULL DEFAULT 0,
    BodySizeBytes       INT               NOT NULL DEFAULT 0,
    TtfbMs              DOUBLE PRECISION  NOT NULL,
    TotalDurationMs     DOUBLE PRECISION  NOT NULL,
    RedirectCount       INT               NOT NULL DEFAULT 0,
    StartedAt           TIMESTAMPTZ       NOT NULL,
    PayloadBytes        BIGINT            NULL,
    ThroughputMbps      DOUBLE PRECISION  NULL,
    CONSTRAINT PK_HttpResult   PRIMARY KEY (HttpId),
    CONSTRAINT FK_Http_Attempt FOREIGN KEY (AttemptId)
        REFERENCES RequestAttempt (AttemptId) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS UdpResult (
    UdpId         UUID              NOT NULL,
    AttemptId     UUID              NOT NULL,
    RemoteAddr    VARCHAR(50)       NOT NULL,
    ProbeCount    INT               NOT NULL,
    SuccessCount  INT               NOT NULL,
    LossPercent   DOUBLE PRECISION  NOT NULL,
    RttMinMs      DOUBLE PRECISION  NOT NULL,
    RttAvgMs      DOUBLE PRECISION  NOT NULL,
    RttP95Ms      DOUBLE PRECISION  NOT NULL,
    JitterMs      DOUBLE PRECISION  NOT NULL,
    StartedAt     TIMESTAMPTZ       NOT NULL,
    CONSTRAINT PK_UdpResult    PRIMARY KEY (UdpId),
    CONSTRAINT FK_Udp_Attempt  FOREIGN KEY (AttemptId)
        REFERENCES RequestAttempt (AttemptId) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS ErrorRecord (
    ErrorId        UUID     NOT NULL,
    AttemptId      UUID     NULL,
    RunId          UUID     NOT NULL,
    ErrorCategory  VARCHAR(50)  NOT NULL,
    ErrorMessage   TEXT         NOT NULL,
    ErrorDetail    TEXT         NULL,
    OccurredAt     TIMESTAMPTZ  NOT NULL,
    CONSTRAINT PK_ErrorRecord   PRIMARY KEY (ErrorId),
    CONSTRAINT FK_Error_Attempt FOREIGN KEY (AttemptId)
        REFERENCES RequestAttempt (AttemptId) ON DELETE NO ACTION,
    CONSTRAINT FK_Error_Run     FOREIGN KEY (RunId)
        REFERENCES TestRun (RunId) ON DELETE NO ACTION
);

CREATE TABLE IF NOT EXISTS ServerTimingResult (
    ServerId        UUID              NOT NULL,
    AttemptId       UUID              NOT NULL,
    RequestId       VARCHAR(128)      NULL,
    ServerTimestamp  TIMESTAMPTZ      NULL,
    ClockSkewMs     DOUBLE PRECISION  NULL,
    RecvBodyMs      DOUBLE PRECISION  NULL,
    ProcessingMs    DOUBLE PRECISION  NULL,
    TotalServerMs   DOUBLE PRECISION  NULL,
    CONSTRAINT PK_ServerTimingResult PRIMARY KEY (ServerId),
    CONSTRAINT FK_ServerTimingResult_Attempt FOREIGN KEY (AttemptId)
        REFERENCES RequestAttempt (AttemptId)
);

-- Indexes
CREATE INDEX IF NOT EXISTS IX_TestRun_StartedAt    ON TestRun (StartedAt DESC);
CREATE INDEX IF NOT EXISTS IX_TestRun_TargetHost   ON TestRun (TargetHost);
CREATE INDEX IF NOT EXISTS IX_Attempt_Protocol     ON RequestAttempt (Protocol, Success);
CREATE INDEX IF NOT EXISTS IX_Attempt_RunId        ON RequestAttempt (RunId, SequenceNum);
CREATE INDEX IF NOT EXISTS IX_HttpResult_Version   ON HttpResult (NegotiatedVersion, StatusCode);
CREATE INDEX IF NOT EXISTS IX_HttpResult_Throughput ON HttpResult (ThroughputMbps) WHERE ThroughputMbps IS NOT NULL;
CREATE INDEX IF NOT EXISTS IX_Error_Category       ON ErrorRecord (ErrorCategory, OccurredAt DESC);
CREATE INDEX IF NOT EXISTS IX_ServerTimingResult_AttemptId ON ServerTimingResult (AttemptId);
"#;

#[async_trait]
impl DatabaseBackend for PostgresBackend {
    async fn migrate(&self) -> anyhow::Result<()> {
        // Create the version-tracking table if it doesn't exist.
        self.client
            .execute(
                "CREATE TABLE IF NOT EXISTS _schema_versions (
                    version  VARCHAR(20) NOT NULL PRIMARY KEY,
                    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
                )",
                &[],
            )
            .await
            .context("create _schema_versions")?;

        // Check if V001 has already been applied.
        let row = self
            .client
            .query_opt("SELECT 1 FROM _schema_versions WHERE version = 'V001'", &[])
            .await
            .context("check V001")?;

        if row.is_none() {
            self.client
                .batch_execute(V001_MIGRATION)
                .await
                .context("apply V001 migration")?;

            self.client
                .execute(
                    "INSERT INTO _schema_versions (version) VALUES ('V001')",
                    &[],
                )
                .await
                .context("record V001")?;
        }

        Ok(())
    }

    async fn save(&self, run: &TestRun) -> anyhow::Result<()> {
        insert_test_run(run, &self.client).await?;

        for attempt in &run.attempts {
            insert_request_attempt(attempt, &self.client).await?;

            if let Some(dns) = &attempt.dns {
                insert_dns_result(attempt, dns, &self.client).await?;
            }
            if let Some(tcp) = &attempt.tcp {
                insert_tcp_result(attempt, tcp, &self.client).await?;
            }
            if let Some(tls) = &attempt.tls {
                insert_tls_result(attempt, tls, &self.client).await?;
            }
            if let Some(http) = &attempt.http {
                insert_http_result(attempt, http, &self.client).await?;
            }
            if let Some(udp) = &attempt.udp {
                insert_udp_result(attempt, udp, &self.client).await?;
            }
            if let Some(err) = &attempt.error {
                insert_error(attempt, err, &self.client).await?;
            }
            if let Some(st) = &attempt.server_timing {
                insert_server_timing_result(attempt, st, &self.client).await?;
            }
        }

        Ok(())
    }

    async fn ping(&self) -> anyhow::Result<()> {
        self.client
            .execute("SELECT 1", &[])
            .await
            .context("PostgreSQL ping")?;
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Insert helpers
// ─────────────────────────────────────────────────────────────────────────────

async fn insert_test_run(run: &TestRun, c: &PgClient) -> anyhow::Result<()> {
    let modes = run.modes.join(",");

    c.execute(
        "INSERT INTO TestRun (
            RunId, StartedAt, FinishedAt, TargetUrl, TargetHost,
            Modes, TotalRuns, Concurrency, TimeoutMs,
            ClientOs, ClientVersion, SuccessCount, FailureCount
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
        &[
            &run.run_id,
            &run.started_at,
            &run.finished_at,
            &run.target_url,
            &run.target_host,
            &modes,
            &(run.total_runs as i32),
            &(run.concurrency as i32),
            &(run.timeout_ms as i64),
            &run.client_os,
            &run.client_version,
            &(run.success_count() as i32),
            &(run.failure_count() as i32),
        ],
    )
    .await
    .context("INSERT TestRun")?;
    Ok(())
}

async fn insert_request_attempt(a: &RequestAttempt, c: &PgClient) -> anyhow::Result<()> {
    let protocol = a.protocol.to_string();
    let err_msg: Option<&str> = a.error.as_ref().map(|e| e.message.as_str());

    c.execute(
        "INSERT INTO RequestAttempt (
            AttemptId, RunId, Protocol, SequenceNum,
            StartedAt, FinishedAt, Success, ErrorMessage, RetryCount
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
        &[
            &a.attempt_id,
            &a.run_id,
            &protocol,
            &(a.sequence_num as i32),
            &a.started_at,
            &a.finished_at,
            &a.success,
            &err_msg,
            &(a.retry_count as i32),
        ],
    )
    .await
    .context("INSERT RequestAttempt")?;
    Ok(())
}

async fn insert_dns_result(
    a: &RequestAttempt,
    dns: &crate::metrics::DnsResult,
    c: &PgClient,
) -> anyhow::Result<()> {
    let id = uuid::Uuid::new_v4();
    let ips = dns.resolved_ips.join(",");

    c.execute(
        "INSERT INTO DnsResult (
            DnsId, AttemptId, QueryName, ResolvedIPs,
            DurationMs, StartedAt, Success
         ) VALUES ($1,$2,$3,$4,$5,$6,$7)",
        &[
            &id,
            &a.attempt_id,
            &dns.query_name,
            &ips,
            &dns.duration_ms,
            &dns.started_at,
            &dns.success,
        ],
    )
    .await
    .context("INSERT DnsResult")?;
    Ok(())
}

async fn insert_tcp_result(
    a: &RequestAttempt,
    tcp: &crate::metrics::TcpResult,
    c: &PgClient,
) -> anyhow::Result<()> {
    let id = uuid::Uuid::new_v4();
    let mss = tcp.mss_bytes.map(|v| v as i32);
    let retransmits = tcp.retransmits.map(|v| v as i64);
    let total_retrans = tcp.total_retrans.map(|v| v as i64);
    let snd_cwnd = tcp.snd_cwnd.map(|v| v as i64);
    let snd_ssthresh = tcp.snd_ssthresh.map(|v| v as i64);
    let rcv_space = tcp.rcv_space.map(|v| v as i64);
    let segs_out = tcp.segs_out.map(|v| v as i64);
    let segs_in = tcp.segs_in.map(|v| v as i64);
    let delivery_rate = tcp.delivery_rate_bps.map(|v| v as i64);
    let local_addr: Option<&str> = tcp.local_addr.as_deref();
    let congestion: Option<&str> = tcp.congestion_algorithm.as_deref();

    c.execute(
        "INSERT INTO TcpResult (
            TcpId, AttemptId, LocalAddr, RemoteAddr,
            ConnectDurationMs, AttemptCount, StartedAt, Success,
            MssBytesEstimate, RttEstimateMs,
            Retransmits, TotalRetrans, SndCwnd, SndSsthresh,
            RttVarianceMs, RcvSpace, SegsOut, SegsIn,
            CongestionAlgorithm, DeliveryRateBps, MinRttMs
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,
                   $11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21)",
        &[
            &id,
            &a.attempt_id,
            &local_addr,
            &tcp.remote_addr.as_str(),
            &tcp.connect_duration_ms,
            &(tcp.attempt_count as i32),
            &tcp.started_at,
            &tcp.success,
            &mss,
            &tcp.rtt_estimate_ms,
            &retransmits,
            &total_retrans,
            &snd_cwnd,
            &snd_ssthresh,
            &tcp.rtt_variance_ms,
            &rcv_space,
            &segs_out,
            &segs_in,
            &congestion,
            &delivery_rate,
            &tcp.min_rtt_ms,
        ],
    )
    .await
    .context("INSERT TcpResult")?;
    Ok(())
}

async fn insert_tls_result(
    a: &RequestAttempt,
    tls: &crate::metrics::TlsResult,
    c: &PgClient,
) -> anyhow::Result<()> {
    let id = uuid::Uuid::new_v4();
    let alpn: Option<&str> = tls.alpn_negotiated.as_deref();
    let subj: Option<&str> = tls.cert_subject.as_deref();
    let issuer: Option<&str> = tls.cert_issuer.as_deref();

    c.execute(
        "INSERT INTO TlsResult (
            TlsId, AttemptId, ProtocolVersion, CipherSuite,
            AlpnNegotiated, CertSubject, CertIssuer, CertExpiry,
            HandshakeDurationMs, StartedAt, Success
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
        &[
            &id,
            &a.attempt_id,
            &tls.protocol_version.as_str(),
            &tls.cipher_suite.as_str(),
            &alpn,
            &subj,
            &issuer,
            &tls.cert_expiry,
            &tls.handshake_duration_ms,
            &tls.started_at,
            &tls.success,
        ],
    )
    .await
    .context("INSERT TlsResult")?;
    Ok(())
}

async fn insert_http_result(
    a: &RequestAttempt,
    http: &crate::metrics::HttpResult,
    c: &PgClient,
) -> anyhow::Result<()> {
    let id = uuid::Uuid::new_v4();

    c.execute(
        "INSERT INTO HttpResult (
            HttpId, AttemptId, NegotiatedVersion, StatusCode,
            HeadersSizeBytes, BodySizeBytes, TtfbMs,
            TotalDurationMs, RedirectCount, StartedAt,
            PayloadBytes, ThroughputMbps
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
        &[
            &id,
            &a.attempt_id,
            &http.negotiated_version.as_str(),
            &(http.status_code as i32),
            &(http.headers_size_bytes as i32),
            &(http.body_size_bytes as i32),
            &http.ttfb_ms,
            &http.total_duration_ms,
            &(http.redirect_count as i32),
            &http.started_at,
            &(http.payload_bytes as i64),
            &http.throughput_mbps,
        ],
    )
    .await
    .context("INSERT HttpResult")?;
    Ok(())
}

async fn insert_udp_result(
    a: &RequestAttempt,
    udp: &crate::metrics::UdpResult,
    c: &PgClient,
) -> anyhow::Result<()> {
    let id = uuid::Uuid::new_v4();

    c.execute(
        "INSERT INTO UdpResult (
            UdpId, AttemptId, RemoteAddr, ProbeCount,
            SuccessCount, LossPercent, RttMinMs, RttAvgMs,
            RttP95Ms, JitterMs, StartedAt
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
        &[
            &id,
            &a.attempt_id,
            &udp.remote_addr.as_str(),
            &(udp.probe_count as i32),
            &(udp.success_count as i32),
            &udp.loss_percent,
            &udp.rtt_min_ms,
            &udp.rtt_avg_ms,
            &udp.rtt_p95_ms,
            &udp.jitter_ms,
            &udp.started_at,
        ],
    )
    .await
    .context("INSERT UdpResult")?;
    Ok(())
}

async fn insert_error(
    a: &RequestAttempt,
    err: &crate::metrics::ErrorRecord,
    c: &PgClient,
) -> anyhow::Result<()> {
    let id = uuid::Uuid::new_v4();
    let category = err.category.to_string();
    let detail: Option<&str> = err.detail.as_deref();

    c.execute(
        "INSERT INTO ErrorRecord (
            ErrorId, AttemptId, RunId, ErrorCategory, ErrorMessage, ErrorDetail, OccurredAt
         ) VALUES ($1,$2,$3,$4,$5,$6,$7)",
        &[
            &id,
            &a.attempt_id,
            &a.run_id,
            &category,
            &err.message.as_str(),
            &detail,
            &err.occurred_at,
        ],
    )
    .await
    .context("INSERT ErrorRecord")?;
    Ok(())
}

async fn insert_server_timing_result(
    a: &RequestAttempt,
    st: &crate::metrics::ServerTimingResult,
    c: &PgClient,
) -> anyhow::Result<()> {
    let id = uuid::Uuid::new_v4();
    let req_id: Option<&str> = st.request_id.as_deref();

    c.execute(
        "INSERT INTO ServerTimingResult (
            ServerId, AttemptId, RequestId, ServerTimestamp,
            ClockSkewMs, RecvBodyMs, ProcessingMs, TotalServerMs
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
        &[
            &id,
            &a.attempt_id,
            &req_id,
            &st.server_timestamp,
            &st.clock_skew_ms,
            &st.recv_body_ms,
            &st.processing_ms,
            &st.total_server_ms,
        ],
    )
    .await
    .context("INSERT ServerTimingResult")?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::db::test_fixtures::{bare_attempt, full_attempt, make_run};
    use uuid::Uuid;

    /// Returns `NETWORKER_DB_URL` (postgres://...) or None if unset.
    fn pg_url() -> Option<String> {
        std::env::var("NETWORKER_DB_URL").ok()
    }

    /// Helper: connect and run migrations.
    async fn setup(url: &str) -> PostgresBackend {
        let backend = PostgresBackend::connect(url).await.unwrap();
        backend.migrate().await.unwrap();
        backend
    }

    /// Helper: connect a raw client for verification queries.
    async fn raw_client(url: &str) -> PgClient {
        let (client, conn) = tokio_postgres::connect(url, tokio_postgres::NoTls)
            .await
            .unwrap();
        tokio::spawn(async move {
            if let Err(e) = conn.await {
                eprintln!("raw pg connection error: {e}");
            }
        });
        client
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn db_postgres_insert_round_trip() {
        let url = match pg_url() {
            Some(u) => u,
            None => return,
        };
        let backend = setup(&url).await;
        let run_id = Uuid::new_v4();
        let run = make_run(run_id, vec![bare_attempt(run_id)]);
        backend.save(&run).await.expect("PG save should succeed");
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn db_postgres_full_round_trip() {
        let url = match pg_url() {
            Some(u) => u,
            None => return,
        };
        let backend = setup(&url).await;
        let run_id = Uuid::new_v4();
        let run = make_run(run_id, vec![bare_attempt(run_id), full_attempt(run_id)]);
        backend
            .save(&run)
            .await
            .expect("PG full save should succeed");
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn db_postgres_verify_test_run_fields() {
        let url = match pg_url() {
            Some(u) => u,
            None => return,
        };
        let backend = setup(&url).await;
        let run_id = Uuid::new_v4();
        let run = make_run(run_id, vec![bare_attempt(run_id)]);
        backend.save(&run).await.unwrap();

        let c = raw_client(&url).await;
        let row = c
            .query_one(
                "SELECT RunId, TargetUrl, TargetHost, Modes, TotalRuns,
                 Concurrency, TimeoutMs, ClientOs, ClientVersion,
                 SuccessCount, FailureCount
                 FROM TestRun WHERE RunId = $1",
                &[&run_id],
            )
            .await
            .expect("TestRun row must exist");

        let db_run_id: Uuid = row.get(0);
        assert_eq!(db_run_id, run_id);
        let db_url: &str = row.get(1);
        assert_eq!(db_url, "http://localhost/health");
        let db_host: &str = row.get(2);
        assert_eq!(db_host, "localhost");
        let db_modes: &str = row.get(3);
        assert_eq!(db_modes, "http1");
        let db_total: i32 = row.get(4);
        assert_eq!(db_total, 1);
        let db_conc: i32 = row.get(5);
        assert_eq!(db_conc, 1);
        let db_timeout: i64 = row.get(6);
        assert_eq!(db_timeout, 5000);
        let db_os: &str = row.get(7);
        assert_eq!(db_os, std::env::consts::OS);
        let db_version: &str = row.get(8);
        assert_eq!(db_version, env!("CARGO_PKG_VERSION"));
        let db_success: i32 = row.get(9);
        assert_eq!(db_success, 1);
        let db_fail: i32 = row.get(10);
        assert_eq!(db_fail, 0);
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn db_postgres_verify_all_sub_results() {
        let url = match pg_url() {
            Some(u) => u,
            None => return,
        };
        let backend = setup(&url).await;
        let run_id = Uuid::new_v4();
        let attempt = full_attempt(run_id);
        let attempt_id = attempt.attempt_id;
        let run = make_run(run_id, vec![attempt]);
        backend.save(&run).await.unwrap();

        let c = raw_client(&url).await;

        // RequestAttempt
        let row = c
            .query_one(
                "SELECT Protocol, SequenceNum, Success, RetryCount
                 FROM RequestAttempt WHERE AttemptId = $1",
                &[&attempt_id],
            )
            .await
            .expect("RequestAttempt row");
        let proto: &str = row.get(0);
        assert_eq!(proto, "http1");
        let seq: i32 = row.get(1);
        assert_eq!(seq, 1);
        let success: bool = row.get(2);
        assert!(!success);
        let retry: i32 = row.get(3);
        assert_eq!(retry, 2);

        // DnsResult
        let row = c
            .query_one(
                "SELECT QueryName, ResolvedIPs, DurationMs, Success
                 FROM DnsResult WHERE AttemptId = $1",
                &[&attempt_id],
            )
            .await
            .expect("DnsResult row");
        let qname: &str = row.get(0);
        assert_eq!(qname, "localhost");
        let ips: &str = row.get(1);
        assert_eq!(ips, "127.0.0.1");
        let dur: f64 = row.get(2);
        assert!((dur - 1.5).abs() < 0.01);
        let dns_ok: bool = row.get(3);
        assert!(dns_ok);

        // TcpResult
        let row = c
            .query_one(
                "SELECT RemoteAddr, ConnectDurationMs, MssBytesEstimate,
                 RttEstimateMs, CongestionAlgorithm, DeliveryRateBps, MinRttMs
                 FROM TcpResult WHERE AttemptId = $1",
                &[&attempt_id],
            )
            .await
            .expect("TcpResult row");
        let remote: &str = row.get(0);
        assert_eq!(remote, "127.0.0.1:8080");
        let connect_ms: f64 = row.get(1);
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

        // TlsResult
        let row = c
            .query_one(
                "SELECT ProtocolVersion, CipherSuite, AlpnNegotiated,
                 CertSubject, HandshakeDurationMs
                 FROM TlsResult WHERE AttemptId = $1",
                &[&attempt_id],
            )
            .await
            .expect("TlsResult row");
        let ver: &str = row.get(0);
        assert_eq!(ver, "TLSv1.3");
        let cipher: &str = row.get(1);
        assert_eq!(cipher, "TLS_AES_256_GCM_SHA384");
        let alpn: Option<&str> = row.get(2);
        assert_eq!(alpn, Some("http/1.1"));
        let subj: Option<&str> = row.get(3);
        assert_eq!(subj, Some("CN=localhost"));
        let hs_ms: f64 = row.get(4);
        assert!((hs_ms - 5.0).abs() < 0.01);

        // HttpResult
        let row = c
            .query_one(
                "SELECT NegotiatedVersion, StatusCode, TtfbMs, TotalDurationMs,
                 PayloadBytes, ThroughputMbps
                 FROM HttpResult WHERE AttemptId = $1",
                &[&attempt_id],
            )
            .await
            .expect("HttpResult row");
        let http_ver: &str = row.get(0);
        assert_eq!(http_ver, "HTTP/1.1");
        let status: i32 = row.get(1);
        assert_eq!(status, 200);
        let ttfb: f64 = row.get(2);
        assert!((ttfb - 8.0).abs() < 0.01);
        let total: f64 = row.get(3);
        assert!((total - 12.0).abs() < 0.01);
        let payload: Option<i64> = row.get(4);
        assert_eq!(payload, Some(65536));
        let tput: Option<f64> = row.get(5);
        assert!((tput.unwrap() - 105.0).abs() < 0.01);

        // UdpResult
        let row = c
            .query_one(
                "SELECT ProbeCount, SuccessCount, LossPercent,
                 RttMinMs, RttAvgMs, RttP95Ms, JitterMs
                 FROM UdpResult WHERE AttemptId = $1",
                &[&attempt_id],
            )
            .await
            .expect("UdpResult row");
        let probes: i32 = row.get(0);
        assert_eq!(probes, 5);
        let successes: i32 = row.get(1);
        assert_eq!(successes, 4);
        let loss: f64 = row.get(2);
        assert!((loss - 20.0).abs() < 0.01);
        let rtt_min_val: f64 = row.get(3);
        assert!((rtt_min_val - 0.1).abs() < 0.01);
        let rtt_avg: f64 = row.get(4);
        assert!((rtt_avg - 0.25).abs() < 0.01);
        let rtt_p95: f64 = row.get(5);
        assert!((rtt_p95 - 0.4).abs() < 0.01);
        let jitter: f64 = row.get(6);
        assert!((jitter - 0.05).abs() < 0.01);

        // ErrorRecord
        let row = c
            .query_one(
                "SELECT ErrorCategory, ErrorMessage, ErrorDetail
                 FROM ErrorRecord WHERE AttemptId = $1",
                &[&attempt_id],
            )
            .await
            .expect("ErrorRecord row");
        let cat: &str = row.get(0);
        assert_eq!(cat, "http");
        let msg: &str = row.get(1);
        assert_eq!(msg, "simulated error");
        let detail: Option<&str> = row.get(2);
        assert_eq!(detail, Some("detail text"));

        // ServerTimingResult
        let row = c
            .query_one(
                "SELECT RequestId, ClockSkewMs, ProcessingMs, TotalServerMs
                 FROM ServerTimingResult WHERE AttemptId = $1",
                &[&attempt_id],
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

    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn db_postgres_cascade_delete() {
        let url = match pg_url() {
            Some(u) => u,
            None => return,
        };
        let backend = setup(&url).await;
        let run_id = Uuid::new_v4();
        let attempt = full_attempt(run_id);
        let attempt_id = attempt.attempt_id;
        let run = make_run(run_id, vec![attempt]);
        backend.save(&run).await.unwrap();

        let c = raw_client(&url).await;

        // Verify rows exist
        let rows = c
            .query("SELECT 1 FROM RequestAttempt WHERE RunId = $1", &[&run_id])
            .await
            .unwrap();
        assert!(!rows.is_empty(), "attempt should exist before delete");

        // Delete ErrorRecord and ServerTimingResult first (ON DELETE NO ACTION FKs)
        c.execute(
            "DELETE FROM ErrorRecord WHERE AttemptId = $1",
            &[&attempt_id],
        )
        .await
        .unwrap();
        c.execute(
            "DELETE FROM ServerTimingResult WHERE AttemptId = $1",
            &[&attempt_id],
        )
        .await
        .unwrap();

        // Delete TestRun — should CASCADE to RequestAttempt and children
        c.execute("DELETE FROM TestRun WHERE RunId = $1", &[&run_id])
            .await
            .unwrap();

        let rows = c
            .query("SELECT 1 FROM RequestAttempt WHERE RunId = $1", &[&run_id])
            .await
            .unwrap();
        assert!(rows.is_empty(), "attempts should be cascade-deleted");

        let rows = c
            .query(
                "SELECT 1 FROM DnsResult WHERE AttemptId = $1",
                &[&attempt_id],
            )
            .await
            .unwrap();
        assert!(rows.is_empty(), "DNS results should be cascade-deleted");

        let rows = c
            .query(
                "SELECT 1 FROM HttpResult WHERE AttemptId = $1",
                &[&attempt_id],
            )
            .await
            .unwrap();
        assert!(rows.is_empty(), "HTTP results should be cascade-deleted");
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn db_postgres_duplicate_run_id_fails() {
        let url = match pg_url() {
            Some(u) => u,
            None => return,
        };
        let backend = setup(&url).await;
        let run_id = Uuid::new_v4();
        let run = make_run(run_id, vec![bare_attempt(run_id)]);
        backend.save(&run).await.unwrap();

        // Second insert with same RunId should fail on PK
        let backend2 = setup(&url).await;
        let run2 = make_run(run_id, vec![bare_attempt(run_id)]);
        let err = backend2.save(&run2).await;
        assert!(err.is_err(), "duplicate RunId should fail");
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn db_postgres_migrate_idempotent() {
        let url = match pg_url() {
            Some(u) => u,
            None => return,
        };
        let backend = PostgresBackend::connect(&url).await.unwrap();
        // Run migrate twice — second call should be a no-op.
        backend.migrate().await.unwrap();
        backend.migrate().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn db_postgres_multiple_attempts_count() {
        let url = match pg_url() {
            Some(u) => u,
            None => return,
        };
        let backend = setup(&url).await;
        let run_id = Uuid::new_v4();
        let attempts = vec![
            bare_attempt(run_id),
            bare_attempt(run_id),
            full_attempt(run_id),
        ];
        let mut run = make_run(run_id, attempts);
        run.total_runs = 3;
        backend.save(&run).await.unwrap();

        let c = raw_client(&url).await;

        let rows = c
            .query("SELECT 1 FROM RequestAttempt WHERE RunId = $1", &[&run_id])
            .await
            .unwrap();
        assert_eq!(rows.len(), 3);

        let dns_rows = c
            .query(
                "SELECT 1 FROM DnsResult d
                 JOIN RequestAttempt a ON d.AttemptId = a.AttemptId
                 WHERE a.RunId = $1",
                &[&run_id],
            )
            .await
            .unwrap();
        assert_eq!(dns_rows.len(), 1);

        let http_rows = c
            .query(
                "SELECT 1 FROM HttpResult h
                 JOIN RequestAttempt a ON h.AttemptId = a.AttemptId
                 WHERE a.RunId = $1",
                &[&run_id],
            )
            .await
            .unwrap();
        assert_eq!(http_rows.len(), 1);
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn db_postgres_bare_attempt_no_child_rows() {
        let url = match pg_url() {
            Some(u) => u,
            None => return,
        };
        let backend = setup(&url).await;
        let run_id = Uuid::new_v4();
        let attempt = bare_attempt(run_id);
        let attempt_id = attempt.attempt_id;
        let run = make_run(run_id, vec![attempt]);
        backend.save(&run).await.unwrap();

        let c = raw_client(&url).await;
        for table in &[
            "DnsResult",
            "TcpResult",
            "TlsResult",
            "HttpResult",
            "UdpResult",
            "ErrorRecord",
            "ServerTimingResult",
        ] {
            let rows = c
                .query(
                    &format!("SELECT 1 FROM {table} WHERE AttemptId = $1"),
                    &[&attempt_id],
                )
                .await
                .unwrap();
            assert!(rows.is_empty(), "bare attempt should have no {table} rows");
        }
    }
}
