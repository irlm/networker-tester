/// PostgreSQL backend using `tokio-postgres`.
///
/// Schema mirrors the SQL Server tables (same column names, equivalent types).
/// Migration is embedded and tracked via a `_schema_versions` table.
use super::DatabaseBackend;
use crate::metrics::{RequestAttempt, TestRun, UrlTestProtocolRun, UrlTestResource, UrlTestRun};
use crate::output::json::{
    benchmark_artifact_if_present, BenchmarkArtifact, BenchmarkCase, BenchmarkDataQuality,
    BenchmarkDiagnostics, BenchmarkEnvironment, BenchmarkLaunch, BenchmarkMetadata,
    BenchmarkMethodology, BenchmarkSample, BenchmarkSummary,
};
use anyhow::Context;
use async_trait::async_trait;
use tokio_postgres::error::SqlState;
use tokio_postgres::Client as PgClient;

/// PostgreSQL database backend.
pub struct PostgresBackend {
    client: tokio::sync::Mutex<PgClient>,
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

        Ok(Self {
            client: tokio::sync::Mutex::new(client),
        })
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

const V002_MIGRATION: &str = r#"
-- V002: Add URL page-load diagnostic foundation tables

CREATE TABLE IF NOT EXISTS UrlTestRun (
    Id                        UUID              NOT NULL,
    StartedAt                 TIMESTAMPTZ       NOT NULL,
    CompletedAt               TIMESTAMPTZ       NULL,
    RequestedUrl              VARCHAR(2048)     NOT NULL,
    FinalUrl                  VARCHAR(2048)     NULL,
    Status                    VARCHAR(32)       NOT NULL,
    PageLoadStrategy          VARCHAR(32)       NOT NULL,
    BrowserEngine             VARCHAR(64)       NULL,
    BrowserVersion            VARCHAR(64)       NULL,
    UserAgent                 TEXT              NULL,
    PrimaryOrigin             VARCHAR(1024)     NULL,
    ObservedProtocolPrimaryLoad VARCHAR(32)     NULL,
    AdvertisedAltSvc          TEXT              NULL,
    ValidatedHttpVersions     VARCHAR(128)      NOT NULL DEFAULT '',
    TlsVersion                VARCHAR(32)       NULL,
    CipherSuite               VARCHAR(128)      NULL,
    Alpn                      VARCHAR(32)       NULL,
    DnsMs                     DOUBLE PRECISION  NULL,
    ConnectMs                 DOUBLE PRECISION  NULL,
    HandshakeMs               DOUBLE PRECISION  NULL,
    TtfbMs                    DOUBLE PRECISION  NULL,
    DomContentLoadedMs        DOUBLE PRECISION  NULL,
    LoadEventMs               DOUBLE PRECISION  NULL,
    NetworkIdleMs             DOUBLE PRECISION  NULL,
    CaptureEndMs              DOUBLE PRECISION  NULL,
    TotalRequests             INT               NOT NULL DEFAULT 0,
    TotalTransferBytes        BIGINT            NOT NULL DEFAULT 0,
    PeakConcurrentConnections INT               NULL,
    RedirectCount             INT               NOT NULL DEFAULT 0,
    FailureCount              INT               NOT NULL DEFAULT 0,
    HarPath                   TEXT              NULL,
    PcapPath                  TEXT              NULL,
    PcapSummaryJson           TEXT              NULL,
    CaptureErrors             TEXT              NULL,
    EnvironmentNotes          TEXT              NULL,
    CONSTRAINT PK_UrlTestRun PRIMARY KEY (Id)
);

CREATE TABLE IF NOT EXISTS UrlTestResource (
    Id               UUID              NOT NULL,
    UrlTestRunId     UUID              NOT NULL,
    ResourceUrl      VARCHAR(2048)     NOT NULL,
    Origin           VARCHAR(1024)     NOT NULL,
    ResourceType     VARCHAR(64)       NOT NULL,
    MimeType         VARCHAR(255)      NULL,
    StatusCode       INT               NULL,
    Protocol         VARCHAR(32)       NULL,
    TransferSize     BIGINT            NULL,
    EncodedBodySize  BIGINT            NULL,
    DecodedBodySize  BIGINT            NULL,
    DurationMs       DOUBLE PRECISION  NULL,
    ConnectionId     VARCHAR(128)      NULL,
    ReusedConnection BOOLEAN           NULL,
    InitiatorType    VARCHAR(64)       NULL,
    FromCache        BOOLEAN           NULL,
    Redirected       BOOLEAN           NULL,
    Failed           BOOLEAN           NOT NULL DEFAULT FALSE,
    CONSTRAINT PK_UrlTestResource PRIMARY KEY (Id),
    CONSTRAINT FK_UrlTestResource_Run FOREIGN KEY (UrlTestRunId)
        REFERENCES UrlTestRun (Id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS UrlTestProtocolRun (
    Id               UUID              NOT NULL,
    UrlTestRunId     UUID              NOT NULL,
    ProtocolMode     VARCHAR(16)       NOT NULL,
    RunNumber        INT               NOT NULL,
    AttemptType      VARCHAR(16)       NOT NULL,
    ObservedProtocol VARCHAR(32)       NULL,
    FallbackOccurred BOOLEAN           NULL,
    Succeeded        BOOLEAN           NOT NULL DEFAULT FALSE,
    StatusCode       INT               NULL,
    TtfbMs           DOUBLE PRECISION  NULL,
    TotalMs          DOUBLE PRECISION  NULL,
    FailureReason    TEXT              NULL,
    Error            TEXT              NULL,
    CONSTRAINT PK_UrlTestProtocolRun PRIMARY KEY (Id),
    CONSTRAINT FK_UrlTestProtocolRun_Run FOREIGN KEY (UrlTestRunId)
        REFERENCES UrlTestRun (Id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS IX_UrlTestRun_StartedAt ON UrlTestRun (StartedAt DESC);
CREATE INDEX IF NOT EXISTS IX_UrlTestRun_Status ON UrlTestRun (Status, StartedAt DESC);
CREATE INDEX IF NOT EXISTS IX_UrlTestResource_RunId ON UrlTestResource (UrlTestRunId);
CREATE INDEX IF NOT EXISTS IX_UrlTestProtocolRun_RunId ON UrlTestProtocolRun (UrlTestRunId, ProtocolMode, RunNumber);
"#;

const V003_MIGRATION: &str = r#"
-- V003: Add normalized benchmark storage tables

CREATE TABLE IF NOT EXISTS BenchmarkRun (
    BenchmarkRunId      UUID            NOT NULL,
    ContractVersion     VARCHAR(20)     NOT NULL,
    GeneratedAt         TIMESTAMPTZ     NOT NULL,
    Source              VARCHAR(64)     NOT NULL,
    TargetUrl           VARCHAR(2048)   NOT NULL,
    TargetHost          VARCHAR(255)    NOT NULL,
    Modes               VARCHAR(200)    NOT NULL,
    TotalRuns           INT             NOT NULL,
    Concurrency         INT             NOT NULL,
    TimeoutMs           BIGINT          NOT NULL,
    ClientOs            VARCHAR(50)     NOT NULL,
    ClientVersion       VARCHAR(50)     NOT NULL,
    MethodologyJson     JSONB           NOT NULL,
    DiagnosticsJson     JSONB           NOT NULL,
    AggregateSummaryJson JSONB          NOT NULL,
    CONSTRAINT PK_BenchmarkRun PRIMARY KEY (BenchmarkRunId),
    CONSTRAINT FK_BenchmarkRun_TestRun FOREIGN KEY (BenchmarkRunId)
        REFERENCES TestRun (RunId) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS BenchmarkEnvironment (
    BenchmarkRunId        UUID          NOT NULL,
    ClientInfoJson        JSONB         NULL,
    ServerInfoJson        JSONB         NULL,
    NetworkBaselineJson   JSONB         NULL,
    PacketCaptureEnabled  BOOLEAN       NOT NULL DEFAULT FALSE,
    EnvironmentJson       JSONB         NOT NULL,
    CONSTRAINT PK_BenchmarkEnvironment PRIMARY KEY (BenchmarkRunId),
    CONSTRAINT FK_BenchmarkEnvironment_Run FOREIGN KEY (BenchmarkRunId)
        REFERENCES BenchmarkRun (BenchmarkRunId) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS BenchmarkDataQuality (
    BenchmarkRunId        UUID              NOT NULL,
    NoiseLevel            VARCHAR(16)       NOT NULL,
    SampleStabilityCv     DOUBLE PRECISION  NOT NULL,
    Sufficiency           VARCHAR(16)       NOT NULL,
    PublicationReady      BOOLEAN           NOT NULL DEFAULT FALSE,
    WarningsJson          JSONB             NOT NULL,
    QualityJson           JSONB             NOT NULL,
    CONSTRAINT PK_BenchmarkDataQuality PRIMARY KEY (BenchmarkRunId),
    CONSTRAINT FK_BenchmarkDataQuality_Run FOREIGN KEY (BenchmarkRunId)
        REFERENCES BenchmarkRun (BenchmarkRunId) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS BenchmarkCase (
    BenchmarkRunId        UUID            NOT NULL,
    CaseId                VARCHAR(255)    NOT NULL,
    Protocol              VARCHAR(32)     NOT NULL,
    PayloadBytes          BIGINT          NULL,
    HttpStack             VARCHAR(128)    NULL,
    MetricName            VARCHAR(64)     NOT NULL,
    MetricUnit            VARCHAR(32)     NOT NULL,
    HigherIsBetter        BOOLEAN         NOT NULL,
    CaseJson              JSONB           NOT NULL,
    CONSTRAINT PK_BenchmarkCase PRIMARY KEY (BenchmarkRunId, CaseId),
    CONSTRAINT FK_BenchmarkCase_Run FOREIGN KEY (BenchmarkRunId)
        REFERENCES BenchmarkRun (BenchmarkRunId) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS BenchmarkSample (
    AttemptId             UUID              NOT NULL,
    BenchmarkRunId        UUID              NOT NULL,
    CaseId                VARCHAR(255)      NOT NULL,
    LaunchIndex           INT               NOT NULL DEFAULT 0,
    Phase                 VARCHAR(32)       NOT NULL,
    IterationIndex        INT               NOT NULL,
    Success               BOOLEAN           NOT NULL DEFAULT FALSE,
    RetryCount            INT               NOT NULL DEFAULT 0,
    InclusionStatus       VARCHAR(64)       NOT NULL,
    MetricValue           DOUBLE PRECISION  NULL,
    MetricUnit            VARCHAR(32)       NOT NULL,
    StartedAt             TIMESTAMPTZ       NOT NULL,
    FinishedAt            TIMESTAMPTZ       NULL,
    TotalDurationMs       DOUBLE PRECISION  NULL,
    TtfbMs                DOUBLE PRECISION  NULL,
    SampleJson            JSONB             NOT NULL,
    CONSTRAINT PK_BenchmarkSample PRIMARY KEY (AttemptId),
    CONSTRAINT FK_BenchmarkSample_Run FOREIGN KEY (BenchmarkRunId)
        REFERENCES BenchmarkRun (BenchmarkRunId) ON DELETE CASCADE,
    CONSTRAINT FK_BenchmarkSample_Case FOREIGN KEY (BenchmarkRunId, CaseId)
        REFERENCES BenchmarkCase (BenchmarkRunId, CaseId) ON DELETE CASCADE,
    CONSTRAINT FK_BenchmarkSample_Attempt FOREIGN KEY (AttemptId)
        REFERENCES RequestAttempt (AttemptId) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS BenchmarkSummary (
    BenchmarkRunId         UUID              NOT NULL,
    CaseId                 VARCHAR(255)      NOT NULL,
    Protocol               VARCHAR(32)       NOT NULL,
    PayloadBytes           BIGINT            NULL,
    HttpStack              VARCHAR(128)      NULL,
    MetricName             VARCHAR(64)       NOT NULL,
    MetricUnit             VARCHAR(32)       NOT NULL,
    HigherIsBetter         BOOLEAN           NOT NULL,
    SampleCount            BIGINT            NOT NULL,
    IncludedSampleCount    BIGINT            NOT NULL,
    ExcludedSampleCount    BIGINT            NOT NULL,
    SuccessCount           BIGINT            NOT NULL,
    FailureCount           BIGINT            NOT NULL,
    TotalRequests          BIGINT            NOT NULL,
    ErrorCount             BIGINT            NOT NULL,
    BytesTransferred       BIGINT            NOT NULL,
    WallTimeMs             DOUBLE PRECISION  NOT NULL,
    Rps                    DOUBLE PRECISION  NOT NULL,
    Min                    DOUBLE PRECISION  NOT NULL,
    Mean                   DOUBLE PRECISION  NOT NULL,
    P5                     DOUBLE PRECISION  NOT NULL,
    P25                    DOUBLE PRECISION  NOT NULL,
    P50                    DOUBLE PRECISION  NOT NULL,
    P75                    DOUBLE PRECISION  NOT NULL,
    P95                    DOUBLE PRECISION  NOT NULL,
    P99                    DOUBLE PRECISION  NOT NULL,
    P999                   DOUBLE PRECISION  NOT NULL,
    Max                    DOUBLE PRECISION  NOT NULL,
    Stddev                 DOUBLE PRECISION  NOT NULL,
    LatencyMeanMs          DOUBLE PRECISION  NULL,
    LatencyP50Ms           DOUBLE PRECISION  NULL,
    LatencyP99Ms           DOUBLE PRECISION  NULL,
    LatencyP999Ms          DOUBLE PRECISION  NULL,
    LatencyMaxMs           DOUBLE PRECISION  NULL,
    SummaryJson            JSONB             NOT NULL,
    CONSTRAINT PK_BenchmarkSummary PRIMARY KEY (BenchmarkRunId, CaseId),
    CONSTRAINT FK_BenchmarkSummary_Run FOREIGN KEY (BenchmarkRunId)
        REFERENCES BenchmarkRun (BenchmarkRunId) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS IX_BenchmarkRun_GeneratedAt
    ON BenchmarkRun (GeneratedAt DESC);
CREATE INDEX IF NOT EXISTS IX_BenchmarkCase_Protocol
    ON BenchmarkCase (Protocol, BenchmarkRunId);
CREATE INDEX IF NOT EXISTS IX_BenchmarkSample_RunCase
    ON BenchmarkSample (BenchmarkRunId, CaseId, Phase, Success);
CREATE INDEX IF NOT EXISTS IX_BenchmarkSummary_RunProtocol
    ON BenchmarkSummary (BenchmarkRunId, Protocol);
CREATE INDEX IF NOT EXISTS IX_BenchmarkDataQuality_PublicationReady
    ON BenchmarkDataQuality (PublicationReady, NoiseLevel);
"#;

const V004_MIGRATION: &str = r#"
-- V004: Add explicit benchmark launch lifecycle table

CREATE TABLE IF NOT EXISTS BenchmarkLaunch (
    BenchmarkRunId       UUID            NOT NULL,
    LaunchIndex          INT             NOT NULL,
    Scenario             VARCHAR(64)     NOT NULL,
    PrimaryPhase         VARCHAR(32)     NOT NULL,
    StartedAt            TIMESTAMPTZ     NOT NULL,
    FinishedAt           TIMESTAMPTZ     NULL,
    SampleCount          BIGINT          NOT NULL,
    PrimarySampleCount   BIGINT          NOT NULL,
    WarmupSampleCount    BIGINT          NOT NULL,
    SuccessCount         BIGINT          NOT NULL,
    FailureCount         BIGINT          NOT NULL,
    PhasesJson           JSONB           NOT NULL,
    CONSTRAINT PK_BenchmarkLaunch PRIMARY KEY (BenchmarkRunId, LaunchIndex),
    CONSTRAINT FK_BenchmarkLaunch_Run FOREIGN KEY (BenchmarkRunId)
        REFERENCES BenchmarkRun (BenchmarkRunId) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS IX_BenchmarkLaunch_Phase
    ON BenchmarkLaunch (PrimaryPhase, Scenario, BenchmarkRunId);
"#;

#[async_trait]
impl DatabaseBackend for PostgresBackend {
    async fn migrate(&self) -> anyhow::Result<()> {
        let client = self.client.lock().await;

        // Serialize migrations across concurrently-running tests/processes.
        // CI runs PostgreSQL tests in parallel, and without a lock multiple
        // workers can race while creating/checking `_schema_versions`.
        client
            .execute("SELECT pg_advisory_lock($1)", &[&0x4E54505747524D31_i64])
            .await
            .context("acquire postgres migration advisory lock")?;

        let migrate_result: anyhow::Result<()> = async {
            // Create the version-tracking table if it doesn't exist.
            client
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
            let row = client
                .query_opt("SELECT 1 FROM _schema_versions WHERE version = 'V001'", &[])
                .await
                .context("check V001")?;

            if row.is_none() {
                client
                    .batch_execute(V001_MIGRATION)
                    .await
                    .context("apply V001 migration")?;

                client
                    .execute(
                        "INSERT INTO _schema_versions (version) VALUES ('V001')",
                        &[],
                    )
                    .await
                    .context("record V001")?;
            }

            let row = client
                .query_opt("SELECT 1 FROM _schema_versions WHERE version = 'V002'", &[])
                .await
                .context("check V002")?;

            if row.is_none() {
                client
                    .batch_execute(V002_MIGRATION)
                    .await
                    .context("apply V002 migration")?;

                client
                    .execute(
                        "INSERT INTO _schema_versions (version) VALUES ('V002')",
                        &[],
                    )
                    .await
                    .context("record V002")?;
            }

            let row = client
                .query_opt("SELECT 1 FROM _schema_versions WHERE version = 'V003'", &[])
                .await
                .context("check V003")?;

            if row.is_none() {
                client
                    .batch_execute(V003_MIGRATION)
                    .await
                    .context("apply V003 migration")?;

                client
                    .execute(
                        "INSERT INTO _schema_versions (version) VALUES ('V003')",
                        &[],
                    )
                    .await
                    .context("record V003")?;
            }

            let row = client
                .query_opt("SELECT 1 FROM _schema_versions WHERE version = 'V004'", &[])
                .await
                .context("check V004")?;

            if row.is_none() {
                client
                    .batch_execute(V004_MIGRATION)
                    .await
                    .context("apply V004 migration")?;

                client
                    .execute(
                        "INSERT INTO _schema_versions (version) VALUES ('V004')",
                        &[],
                    )
                    .await
                    .context("record V004")?;
            }

            Ok(())
        }
        .await;

        let unlock_result = client
            .execute("SELECT pg_advisory_unlock($1)", &[&0x4E54505747524D31_i64])
            .await
            .context("release postgres migration advisory lock");

        migrate_result?;
        unlock_result?;
        Ok(())
    }

    async fn save(&self, run: &TestRun) -> anyhow::Result<()> {
        let client = self.client.lock().await;
        let benchmark_schema_ready = benchmark_schema_installed(&client).await?;
        let benchmark_artifact = benchmark_artifact_if_present(run)?;
        client
            .batch_execute("BEGIN")
            .await
            .context("BEGIN TestRun transaction")?;

        let result = async {
            insert_test_run(run, &client).await?;

            for attempt in &run.attempts {
                insert_request_attempt(attempt, &client).await?;

                if let Some(dns) = &attempt.dns {
                    insert_dns_result(attempt, dns, &client).await?;
                }
                if let Some(tcp) = &attempt.tcp {
                    insert_tcp_result(attempt, tcp, &client).await?;
                }
                if let Some(tls) = &attempt.tls {
                    insert_tls_result(attempt, tls, &client).await?;
                }
                if let Some(http) = &attempt.http {
                    insert_http_result(attempt, http, &client).await?;
                }
                if let Some(udp) = &attempt.udp {
                    insert_udp_result(attempt, udp, &client).await?;
                }
                if let Some(err) = &attempt.error {
                    insert_error(attempt, err, &client).await?;
                }
                if let Some(st) = &attempt.server_timing {
                    insert_server_timing_result(attempt, st, &client).await?;
                }
            }

            if benchmark_schema_ready {
                if let Some(artifact) = &benchmark_artifact {
                    insert_benchmark_artifact(run.run_id, artifact, &client).await?;
                }
            } else if benchmark_artifact.is_some() {
                tracing::debug!(
                    "Benchmark schema not installed in PostgreSQL; skipping benchmark persistence"
                );
            } else {
                tracing::trace!("Run is not benchmark-mode; skipping benchmark persistence");
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;

        match result {
            Ok(()) => {
                client
                    .batch_execute("COMMIT")
                    .await
                    .context("COMMIT TestRun transaction")?;
                Ok(())
            }
            Err(e) => {
                let _ = client.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    async fn save_url_test(&self, run: &UrlTestRun) -> anyhow::Result<()> {
        let client = self.client.lock().await;
        client
            .batch_execute("BEGIN")
            .await
            .context("BEGIN UrlTest transaction")?;

        let result = async {
            insert_url_test_run(run, &client).await?;
            for resource in &run.resources {
                insert_url_test_resource(run.id, resource, &client).await?;
            }
            for probe in &run.protocol_runs {
                insert_url_test_protocol_run(run.id, probe, &client).await?;
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;

        match result {
            Ok(()) => {
                client
                    .batch_execute("COMMIT")
                    .await
                    .context("COMMIT UrlTest transaction")?;
                Ok(())
            }
            Err(e) => {
                let _ = client.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    async fn ping(&self) -> anyhow::Result<()> {
        let client = self.client.lock().await;
        client
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
    let capture_json: Option<serde_json::Value> = run
        .packet_capture_summary
        .as_ref()
        .and_then(|s| serde_json::to_value(s).ok());

    // Try with packet_capture_json column first (V005+), fall back without it
    c.batch_execute("SAVEPOINT testrun_packet_capture_column")
        .await
        .context("SAVEPOINT TestRun packet_capture_json")?;
    let result = c
        .execute(
            "INSERT INTO TestRun (
                RunId, StartedAt, FinishedAt, TargetUrl, TargetHost,
                Modes, TotalRuns, Concurrency, TimeoutMs,
                ClientOs, ClientVersion, SuccessCount, FailureCount,
                packet_capture_json
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)",
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
                &capture_json,
            ],
        )
        .await;

    match result {
        Ok(_) => {
            c.batch_execute("RELEASE SAVEPOINT testrun_packet_capture_column")
                .await
                .context("RELEASE SAVEPOINT TestRun packet_capture_json")?;
            Ok(())
        }
        Err(err) => {
            c.batch_execute(
                "ROLLBACK TO SAVEPOINT testrun_packet_capture_column;
                 RELEASE SAVEPOINT testrun_packet_capture_column;",
            )
            .await
            .context("ROLLBACK SAVEPOINT TestRun packet_capture_json")?;

            if !is_missing_column_error(&err, "packet_capture_json") {
                return Err(err).context("INSERT TestRun");
            }

            // Fallback: insert without packet_capture_json (older schema)
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
    }
}

async fn insert_benchmark_artifact(
    run_id: uuid::Uuid,
    artifact: &BenchmarkArtifact,
    c: &PgClient,
) -> anyhow::Result<()> {
    insert_benchmark_run(
        run_id,
        &artifact.metadata,
        &artifact.methodology,
        &artifact.diagnostics,
        &artifact.summary,
        c,
    )
    .await?;
    insert_benchmark_environment(run_id, &artifact.environment, c).await?;
    insert_benchmark_data_quality(run_id, &artifact.data_quality, c).await?;
    for launch in &artifact.launches {
        insert_benchmark_launch(run_id, launch, c).await?;
    }

    for case in &artifact.cases {
        insert_benchmark_case(run_id, case, c).await?;
    }
    for sample in &artifact.samples {
        insert_benchmark_sample(run_id, sample, c).await?;
    }
    for summary in &artifact.summaries {
        insert_benchmark_summary(run_id, summary, c).await?;
    }

    Ok(())
}

async fn benchmark_schema_installed(c: &PgClient) -> anyhow::Result<bool> {
    let row = c
        .query_one(
            "SELECT
                to_regclass('public.benchmarkrun') IS NOT NULL
            AND to_regclass('public.benchmarklaunch') IS NOT NULL
            AND to_regclass('public.benchmarkenvironment') IS NOT NULL
            AND to_regclass('public.benchmarkdataquality') IS NOT NULL
            AND to_regclass('public.benchmarkcase') IS NOT NULL
            AND to_regclass('public.benchmarksample') IS NOT NULL
            AND to_regclass('public.benchmarksummary') IS NOT NULL",
            &[],
        )
        .await
        .context("query postgres benchmark schema readiness")?;

    Ok(row.get::<usize, bool>(0))
}

async fn insert_benchmark_launch(
    run_id: uuid::Uuid,
    launch: &BenchmarkLaunch,
    c: &PgClient,
) -> anyhow::Result<()> {
    let phases_json = serde_json::to_value(&launch.phases_present)?;
    c.execute(
        "INSERT INTO BenchmarkLaunch (
            BenchmarkRunId, LaunchIndex, Scenario, PrimaryPhase, StartedAt, FinishedAt,
            SampleCount, PrimarySampleCount, WarmupSampleCount, SuccessCount, FailureCount,
            PhasesJson
         ) VALUES (
            $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12
         )",
        &[
            &run_id,
            &(launch.launch_index as i32),
            &launch.scenario,
            &launch.primary_phase,
            &launch.started_at,
            &launch.finished_at,
            &(launch.sample_count as i64),
            &(launch.primary_sample_count as i64),
            &(launch.warmup_sample_count as i64),
            &(launch.success_count as i64),
            &(launch.failure_count as i64),
            &phases_json,
        ],
    )
    .await
    .context("INSERT BenchmarkLaunch")?;
    Ok(())
}

async fn insert_benchmark_run(
    run_id: uuid::Uuid,
    metadata: &BenchmarkMetadata,
    methodology: &BenchmarkMethodology,
    diagnostics: &BenchmarkDiagnostics,
    aggregate_summary: &BenchmarkSummary,
    c: &PgClient,
) -> anyhow::Result<()> {
    let modes = metadata.modes.join(",");
    let methodology_json = serde_json::to_value(methodology)?;
    let diagnostics_json = serde_json::to_value(diagnostics)?;
    let aggregate_summary_json = serde_json::to_value(aggregate_summary)?;

    c.execute(
        "INSERT INTO BenchmarkRun (
            BenchmarkRunId, ContractVersion, GeneratedAt, Source, TargetUrl, TargetHost,
            Modes, TotalRuns, Concurrency, TimeoutMs, ClientOs, ClientVersion,
            MethodologyJson, DiagnosticsJson, AggregateSummaryJson
         ) VALUES (
            $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15
         )",
        &[
            &run_id,
            &metadata.contract_version,
            &metadata.generated_at,
            &metadata.source,
            &metadata.target_url,
            &metadata.target_host,
            &modes,
            &(metadata.total_runs as i32),
            &(metadata.concurrency as i32),
            &(metadata.timeout_ms as i64),
            &metadata.client_os,
            &metadata.client_version,
            &methodology_json,
            &diagnostics_json,
            &aggregate_summary_json,
        ],
    )
    .await
    .context("INSERT BenchmarkRun")?;
    Ok(())
}

async fn insert_benchmark_environment(
    run_id: uuid::Uuid,
    environment: &BenchmarkEnvironment,
    c: &PgClient,
) -> anyhow::Result<()> {
    let client_info_json = environment
        .client_info
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .context("serialize BenchmarkEnvironment.client_info")?;
    let server_info_json = environment
        .server_info
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .context("serialize BenchmarkEnvironment.server_info")?;
    let network_baseline_json = environment
        .network_baseline
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .context("serialize BenchmarkEnvironment.network_baseline")?;
    let environment_json = serde_json::to_value(environment)?;

    c.execute(
        "INSERT INTO BenchmarkEnvironment (
            BenchmarkRunId, ClientInfoJson, ServerInfoJson, NetworkBaselineJson,
            PacketCaptureEnabled, EnvironmentJson
         ) VALUES ($1,$2,$3,$4,$5,$6)",
        &[
            &run_id,
            &client_info_json,
            &server_info_json,
            &network_baseline_json,
            &environment.packet_capture_enabled,
            &environment_json,
        ],
    )
    .await
    .context("INSERT BenchmarkEnvironment")?;
    Ok(())
}

async fn insert_benchmark_data_quality(
    run_id: uuid::Uuid,
    quality: &BenchmarkDataQuality,
    c: &PgClient,
) -> anyhow::Result<()> {
    let warnings_json = serde_json::to_value(&quality.warnings)?;
    let quality_json = serde_json::to_value(quality)?;
    c.execute(
        "INSERT INTO BenchmarkDataQuality (
            BenchmarkRunId, NoiseLevel, SampleStabilityCv, Sufficiency,
            PublicationReady, WarningsJson, QualityJson
         ) VALUES ($1,$2,$3,$4,$5,$6,$7)",
        &[
            &run_id,
            &quality.noise_level,
            &quality.sample_stability_cv,
            &quality.sufficiency,
            &quality.publication_ready,
            &warnings_json,
            &quality_json,
        ],
    )
    .await
    .context("INSERT BenchmarkDataQuality")?;
    Ok(())
}

async fn insert_benchmark_case(
    run_id: uuid::Uuid,
    case: &BenchmarkCase,
    c: &PgClient,
) -> anyhow::Result<()> {
    let case_json = serde_json::to_value(case)?;
    c.execute(
        "INSERT INTO BenchmarkCase (
            BenchmarkRunId, CaseId, Protocol, PayloadBytes, HttpStack,
            MetricName, MetricUnit, HigherIsBetter, CaseJson
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
        &[
            &run_id,
            &case.id,
            &case.protocol,
            &case.payload_bytes.map(|v| v as i64),
            &case.http_stack,
            &case.metric_name,
            &case.metric_unit,
            &case.higher_is_better,
            &case_json,
        ],
    )
    .await
    .context("INSERT BenchmarkCase")?;
    Ok(())
}

async fn insert_benchmark_sample(
    run_id: uuid::Uuid,
    sample: &BenchmarkSample,
    c: &PgClient,
) -> anyhow::Result<()> {
    let sample_json = serde_json::to_value(sample)?;
    c.execute(
        "INSERT INTO BenchmarkSample (
            AttemptId, BenchmarkRunId, CaseId, LaunchIndex, Phase, IterationIndex,
            Success, RetryCount, InclusionStatus, MetricValue, MetricUnit, StartedAt,
            FinishedAt, TotalDurationMs, TtfbMs, SampleJson
         ) VALUES (
            $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16
         )",
        &[
            &sample.attempt_id,
            &run_id,
            &sample.case_id,
            &(sample.launch_index as i32),
            &sample.phase,
            &(sample.iteration_index as i32),
            &sample.success,
            &(sample.retry_count as i32),
            &sample.inclusion_status,
            &sample.metric_value,
            &sample.metric_unit,
            &sample.started_at,
            &sample.finished_at,
            &sample.total_duration_ms,
            &sample.ttfb_ms,
            &sample_json,
        ],
    )
    .await
    .context("INSERT BenchmarkSample")?;
    Ok(())
}

async fn insert_benchmark_summary(
    run_id: uuid::Uuid,
    summary: &BenchmarkSummary,
    c: &PgClient,
) -> anyhow::Result<()> {
    let summary_json = serde_json::to_value(summary)?;
    c.execute(
        "INSERT INTO BenchmarkSummary (
            BenchmarkRunId, CaseId, Protocol, PayloadBytes, HttpStack, MetricName,
            MetricUnit, HigherIsBetter, SampleCount, IncludedSampleCount,
            ExcludedSampleCount, SuccessCount, FailureCount, TotalRequests, ErrorCount,
            BytesTransferred, WallTimeMs, Rps, Min, Mean, P5, P25, P50, P75, P95, P99,
            P999, Max, Stddev, LatencyMeanMs, LatencyP50Ms, LatencyP99Ms,
            LatencyP999Ms, LatencyMaxMs, SummaryJson
         ) VALUES (
            $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,
            $21,$22,$23,$24,$25,$26,$27,$28,$29,$30,$31,$32,$33,$34,$35
         )",
        &[
            &run_id,
            &summary.case_id,
            &summary.protocol,
            &summary.payload_bytes.map(|v| v as i64),
            &summary.http_stack,
            &summary.metric_name,
            &summary.metric_unit,
            &summary.higher_is_better,
            &(summary.sample_count as i64),
            &(summary.included_sample_count as i64),
            &(summary.excluded_sample_count as i64),
            &(summary.success_count as i64),
            &(summary.failure_count as i64),
            &(summary.total_requests as i64),
            &(summary.error_count as i64),
            &(summary.bytes_transferred as i64),
            &summary.wall_time_ms,
            &summary.rps,
            &summary.min,
            &summary.mean,
            &summary.p5,
            &summary.p25,
            &summary.p50,
            &summary.p75,
            &summary.p95,
            &summary.p99,
            &summary.p999,
            &summary.max,
            &summary.stddev,
            &summary.latency_mean_ms,
            &summary.latency_p50_ms,
            &summary.latency_p99_ms,
            &summary.latency_p999_ms,
            &summary.latency_max_ms,
            &summary_json,
        ],
    )
    .await
    .context("INSERT BenchmarkSummary")?;
    Ok(())
}

async fn insert_url_test_run(run: &UrlTestRun, c: &PgClient) -> anyhow::Result<()> {
    let validated_http_versions = run.validated_http_versions.join(",");
    let capture_errors = if run.capture_errors.is_empty() {
        None
    } else {
        Some(run.capture_errors.join("\n"))
    };
    let pcap_summary_json = run
        .pcap_summary
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .context("serialize UrlPacketCaptureSummary")?;
    let status = run.status.to_string();
    let page_load_strategy = serde_json::to_value(&run.page_load_strategy)?
        .as_str()
        .unwrap_or("browser")
        .to_string();

    c.execute(
        "INSERT INTO UrlTestRun (
            Id, StartedAt, CompletedAt, RequestedUrl, FinalUrl, Status, PageLoadStrategy,
            BrowserEngine, BrowserVersion, UserAgent, PrimaryOrigin, ObservedProtocolPrimaryLoad,
            AdvertisedAltSvc, ValidatedHttpVersions, TlsVersion, CipherSuite, Alpn,
            DnsMs, ConnectMs, HandshakeMs, TtfbMs, DomContentLoadedMs, LoadEventMs,
            NetworkIdleMs, CaptureEndMs, TotalRequests, TotalTransferBytes,
            PeakConcurrentConnections, RedirectCount, FailureCount, HarPath, PcapPath,
            PcapSummaryJson, CaptureErrors, EnvironmentNotes
         ) VALUES (
            $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,
            $18,$19,$20,$21,$22,$23,$24,$25,$26,$27,$28,$29,$30,$31,$32,$33,$34,$35
         )",
        &[
            &run.id,
            &run.started_at,
            &run.completed_at,
            &run.requested_url,
            &run.final_url,
            &status,
            &page_load_strategy,
            &run.browser_engine,
            &run.browser_version,
            &run.user_agent,
            &run.primary_origin,
            &run.observed_protocol_primary_load,
            &run.advertised_alt_svc,
            &validated_http_versions,
            &run.tls_version,
            &run.cipher_suite,
            &run.alpn,
            &run.dns_ms,
            &run.connect_ms,
            &run.handshake_ms,
            &run.ttfb_ms,
            &run.dom_content_loaded_ms,
            &run.load_event_ms,
            &run.network_idle_ms,
            &run.capture_end_ms,
            &(run.total_requests as i32),
            &(run.total_transfer_bytes as i64),
            &run.peak_concurrent_connections.map(|v| v as i32),
            &(run.redirect_count as i32),
            &(run.failure_count as i32),
            &run.har_path,
            &run.pcap_path,
            &pcap_summary_json,
            &capture_errors,
            &run.environment_notes,
        ],
    )
    .await
    .context("INSERT UrlTestRun")?;
    Ok(())
}

async fn insert_url_test_resource(
    run_id: uuid::Uuid,
    r: &UrlTestResource,
    c: &PgClient,
) -> anyhow::Result<()> {
    let id = uuid::Uuid::new_v4();
    c.execute(
        "INSERT INTO UrlTestResource (
            Id, UrlTestRunId, ResourceUrl, Origin, ResourceType, MimeType, StatusCode,
            Protocol, TransferSize, EncodedBodySize, DecodedBodySize, DurationMs,
            ConnectionId, ReusedConnection, InitiatorType, FromCache, Redirected, Failed
        ) VALUES (
            $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18
        )",
        &[
            &id,
            &run_id,
            &r.resource_url,
            &r.origin,
            &r.resource_type,
            &r.mime_type,
            &r.status_code.map(|v| v as i32),
            &r.protocol,
            &r.transfer_size.map(|v| v as i64),
            &r.encoded_body_size.map(|v| v as i64),
            &r.decoded_body_size.map(|v| v as i64),
            &r.duration_ms,
            &r.connection_id,
            &r.reused_connection,
            &r.initiator_type,
            &r.from_cache,
            &r.redirected,
            &r.failed,
        ],
    )
    .await
    .context("INSERT UrlTestResource")?;
    Ok(())
}

async fn insert_url_test_protocol_run(
    run_id: uuid::Uuid,
    p: &UrlTestProtocolRun,
    c: &PgClient,
) -> anyhow::Result<()> {
    let id = uuid::Uuid::new_v4();
    let attempt_type = serde_json::to_value(&p.attempt_type)?
        .as_str()
        .unwrap_or("probe")
        .to_string();
    c.execute(
        "INSERT INTO UrlTestProtocolRun (
            Id, UrlTestRunId, ProtocolMode, RunNumber, AttemptType, ObservedProtocol,
            FallbackOccurred, Succeeded, StatusCode, TtfbMs, TotalMs, FailureReason, Error
        ) VALUES (
            $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13
        )",
        &[
            &id,
            &run_id,
            &p.protocol_mode,
            &(p.run_number as i32),
            &attempt_type,
            &p.observed_protocol,
            &p.fallback_occurred,
            &p.succeeded,
            &p.status_code.map(|v| v as i32),
            &p.ttfb_ms,
            &p.total_ms,
            &p.failure_reason,
            &p.error,
        ],
    )
    .await
    .context("INSERT UrlTestProtocolRun")?;
    Ok(())
}

async fn insert_request_attempt(a: &RequestAttempt, c: &PgClient) -> anyhow::Result<()> {
    let protocol = a.protocol.to_string();
    let err_msg: Option<&str> = a.error.as_ref().map(|e| e.message.as_str());

    // Serialize the full attempt as JSON for rich data (browser, pageload, etc.)
    let extra_json: Option<serde_json::Value> = serde_json::to_value(a).ok();

    // Try with extra_json column first (V004+), fall back to without
    c.batch_execute("SAVEPOINT requestattempt_extra_json_column")
        .await
        .context("SAVEPOINT RequestAttempt extra_json")?;
    let result = c
        .execute(
            "INSERT INTO RequestAttempt (
            AttemptId, RunId, Protocol, SequenceNum,
            StartedAt, FinishedAt, Success, ErrorMessage, RetryCount, extra_json
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
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
                &extra_json,
            ],
        )
        .await;

    match result {
        Ok(_) => {
            c.batch_execute("RELEASE SAVEPOINT requestattempt_extra_json_column")
                .await
                .context("RELEASE SAVEPOINT RequestAttempt extra_json")?;
            Ok(())
        }
        Err(err) => {
            c.batch_execute(
                "ROLLBACK TO SAVEPOINT requestattempt_extra_json_column;
                 RELEASE SAVEPOINT requestattempt_extra_json_column;",
            )
            .await
            .context("ROLLBACK SAVEPOINT RequestAttempt extra_json")?;

            if !is_missing_column_error(&err, "extra_json") {
                return Err(err).context("INSERT RequestAttempt");
            }

            // Fallback: insert without extra_json (older schema)
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
    }
}

fn is_missing_column_error(err: &tokio_postgres::Error, column: &str) -> bool {
    err.as_db_error().is_some_and(|db_err| {
        db_err.code() == &SqlState::UNDEFINED_COLUMN
            && db_err
                .message()
                .to_ascii_lowercase()
                .contains(&column.to_ascii_lowercase())
    })
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
    use crate::output::db::test_fixtures::{
        bare_attempt, full_attempt, make_benchmark_run, make_run,
    };
    use url::Url;
    use uuid::Uuid;

    const DOCUMENTED_V001_SCHEMA: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../sql/postgres/01_CreateSchema.sql"
    ));
    const DOCUMENTED_V003_BENCHMARK_SCHEMA: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../sql/postgres/02_BenchmarkSchema.sql"
    ));

    // ── Migration SQL content tests (no database required) ────────────────────

    /// The migration SQL must declare all 9 tables with CREATE TABLE IF NOT EXISTS.
    #[test]
    fn v001_migration_contains_all_table_definitions() {
        let expected_tables = [
            "TestRun",
            "RequestAttempt",
            "DnsResult",
            "TcpResult",
            "TlsResult",
            "HttpResult",
            "UdpResult",
            "ErrorRecord",
            "ServerTimingResult",
        ];
        for table in &expected_tables {
            assert!(
                V001_MIGRATION.contains(&format!("CREATE TABLE IF NOT EXISTS {table}")),
                "V001 migration missing CREATE TABLE IF NOT EXISTS {table}"
            );
        }
    }

    /// Every table must have a PRIMARY KEY constraint.
    #[test]
    fn v001_migration_contains_primary_keys_for_all_tables() {
        let expected_pks = [
            "PK_TestRun",
            "PK_RequestAttempt",
            "PK_DnsResult",
            "PK_TcpResult",
            "PK_TlsResult",
            "PK_HttpResult",
            "PK_UdpResult",
            "PK_ErrorRecord",
            "PK_ServerTimingResult",
        ];
        for pk in &expected_pks {
            assert!(
                V001_MIGRATION.contains(pk),
                "V001 migration missing PRIMARY KEY constraint: {pk}"
            );
        }
    }

    /// Every child table must have a FOREIGN KEY back to its parent.
    #[test]
    fn v001_migration_contains_foreign_keys() {
        let expected_fks = [
            "FK_Attempt_Run",
            "FK_Dns_Attempt",
            "FK_Tcp_Attempt",
            "FK_Tls_Attempt",
            "FK_Http_Attempt",
            "FK_Udp_Attempt",
            "FK_Error_Attempt",
            "FK_Error_Run",
            "FK_ServerTimingResult_Attempt",
        ];
        for fk in &expected_fks {
            assert!(
                V001_MIGRATION.contains(fk),
                "V001 migration missing FOREIGN KEY constraint: {fk}"
            );
        }
    }

    /// ON DELETE CASCADE must be present so child rows are swept when a run is deleted.
    #[test]
    fn v001_migration_contains_cascade_deletes() {
        assert!(
            V001_MIGRATION.contains("ON DELETE CASCADE"),
            "V001 migration should contain ON DELETE CASCADE"
        );
    }

    /// All expected indexes must be declared with CREATE INDEX IF NOT EXISTS.
    #[test]
    fn v001_migration_contains_expected_indexes() {
        let expected_indexes = [
            "IX_TestRun_StartedAt",
            "IX_TestRun_TargetHost",
            "IX_Attempt_Protocol",
            "IX_Attempt_RunId",
            "IX_HttpResult_Version",
            "IX_HttpResult_Throughput",
            "IX_Error_Category",
            "IX_ServerTimingResult_AttemptId",
        ];
        for idx in &expected_indexes {
            assert!(
                V001_MIGRATION.contains(idx),
                "V001 migration missing index: {idx}"
            );
        }
    }

    /// The migration should use UUID primary keys throughout (not integers).
    #[test]
    fn v001_migration_uses_uuid_primary_keys() {
        // All ID columns are defined as UUID NOT NULL
        assert!(
            V001_MIGRATION.contains("RunId          UUID            NOT NULL"),
            "RunId should be UUID NOT NULL"
        );
        assert!(
            V001_MIGRATION.contains("AttemptId     UUID            NOT NULL"),
            "AttemptId should be UUID NOT NULL"
        );
    }

    /// Timestamps must use TIMESTAMPTZ (timezone-aware) for all date/time columns.
    #[test]
    fn v001_migration_uses_timestamptz_for_dates() {
        // Count occurrences — every table has at least a StartedAt column
        let count = V001_MIGRATION.matches("TIMESTAMPTZ").count();
        assert!(
            count >= 9,
            "expected at least 9 TIMESTAMPTZ columns, found {count}"
        );
    }

    /// The migration text must not be empty and must start with the version comment.
    #[test]
    fn v001_migration_starts_with_version_comment() {
        let trimmed = V001_MIGRATION.trim();
        assert!(
            trimmed.starts_with("-- V001:"),
            "migration should start with -- V001: comment"
        );
    }

    /// Sanity-check: the SQL string is non-trivially long (guards against accidental truncation).
    #[test]
    fn v001_migration_is_substantial() {
        assert!(
            V001_MIGRATION.len() > 2000,
            "V001 migration SQL seems too short ({}B) — possible truncation",
            V001_MIGRATION.len()
        );
    }

    #[test]
    fn v002_migration_contains_url_diagnostic_tables() {
        for table in ["UrlTestRun", "UrlTestResource", "UrlTestProtocolRun"] {
            assert!(
                V002_MIGRATION.contains(&format!("CREATE TABLE IF NOT EXISTS {table}")),
                "V002 migration missing CREATE TABLE IF NOT EXISTS {table}"
            );
        }
    }

    #[test]
    fn v002_migration_contains_url_diagnostic_indexes() {
        for idx in [
            "IX_UrlTestRun_StartedAt",
            "IX_UrlTestRun_Status",
            "IX_UrlTestResource_RunId",
            "IX_UrlTestProtocolRun_RunId",
        ] {
            assert!(
                V002_MIGRATION.contains(idx),
                "V002 migration missing index: {idx}"
            );
        }
    }

    #[test]
    fn v002_migration_starts_with_version_comment() {
        let trimmed = V002_MIGRATION.trim();
        assert!(
            trimmed.starts_with("-- V002:"),
            "migration should start with -- V002: comment"
        );
    }

    #[test]
    fn v003_migration_contains_benchmark_tables() {
        for table in [
            "BenchmarkRun",
            "BenchmarkEnvironment",
            "BenchmarkDataQuality",
            "BenchmarkCase",
            "BenchmarkSample",
            "BenchmarkSummary",
        ] {
            assert!(
                V003_MIGRATION.contains(&format!("CREATE TABLE IF NOT EXISTS {table}")),
                "V003 migration missing CREATE TABLE IF NOT EXISTS {table}"
            );
        }
    }

    #[test]
    fn v003_migration_contains_benchmark_indexes() {
        for idx in [
            "IX_BenchmarkRun_GeneratedAt",
            "IX_BenchmarkCase_Protocol",
            "IX_BenchmarkSample_RunCase",
            "IX_BenchmarkSummary_RunProtocol",
            "IX_BenchmarkDataQuality_PublicationReady",
        ] {
            assert!(
                V003_MIGRATION.contains(idx),
                "V003 migration missing index: {idx}"
            );
        }
    }

    #[test]
    fn v003_migration_starts_with_version_comment() {
        let trimmed = V003_MIGRATION.trim();
        assert!(
            trimmed.starts_with("-- V003:"),
            "migration should start with -- V003: comment"
        );
    }

    #[test]
    fn v004_migration_contains_benchmark_launch_table() {
        assert!(
            V004_MIGRATION.contains("CREATE TABLE IF NOT EXISTS BenchmarkLaunch"),
            "V004 migration missing BenchmarkLaunch table"
        );
    }

    #[test]
    fn v004_migration_contains_benchmark_launch_index() {
        assert!(
            V004_MIGRATION.contains("IX_BenchmarkLaunch_Phase"),
            "V004 migration missing benchmark launch index"
        );
    }

    #[test]
    fn v004_migration_starts_with_version_comment() {
        let trimmed = V004_MIGRATION.trim();
        assert!(
            trimmed.starts_with("-- V004:"),
            "migration should start with -- V004: comment"
        );
    }

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

    fn database_url_with_name(base_url: &str, db_name: &str) -> String {
        let mut parsed = Url::parse(base_url).expect("NETWORKER_DB_URL should be a valid URL");
        parsed.set_path(&format!("/{db_name}"));
        parsed.to_string()
    }

    fn quote_identifier(name: &str) -> String {
        format!("\"{}\"", name.replace('"', "\"\""))
    }

    async fn create_isolated_database(base_url: &str, prefix: &str) -> String {
        let admin = raw_client(base_url).await;
        let db_name = format!("networker_{}_{}", prefix, Uuid::new_v4().simple());
        let quoted_name = quote_identifier(&db_name);

        admin
            .execute(&format!("DROP DATABASE IF EXISTS {quoted_name}"), &[])
            .await
            .unwrap();
        admin
            .execute(&format!("CREATE DATABASE {quoted_name}"), &[])
            .await
            .unwrap();

        database_url_with_name(base_url, &db_name)
    }

    async fn install_documented_schema(url: &str, benchmark_schema: bool) {
        let client = raw_client(url).await;
        client.batch_execute(DOCUMENTED_V001_SCHEMA).await.unwrap();
        if benchmark_schema {
            client
                .batch_execute(DOCUMENTED_V003_BENCHMARK_SCHEMA)
                .await
                .unwrap();
        }
    }

    async fn isolated_backend_with_documented_schema(
        base_url: &str,
        prefix: &str,
        benchmark_schema: bool,
    ) -> (String, PostgresBackend) {
        let db_url = create_isolated_database(base_url, prefix).await;
        install_documented_schema(&db_url, benchmark_schema).await;
        let backend = PostgresBackend::connect(&db_url).await.unwrap();
        (db_url, backend)
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

    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn db_postgres_persists_benchmark_rows() {
        let url = match pg_url() {
            Some(u) => u,
            None => return,
        };
        let backend = setup(&url).await;
        let run_id = Uuid::new_v4();
        let run = make_benchmark_run(run_id, vec![bare_attempt(run_id)]);
        backend.save(&run).await.unwrap();

        let c = raw_client(&url).await;

        let benchmark_run = c
            .query_one(
                "SELECT ContractVersion, TargetHost
                 FROM BenchmarkRun WHERE BenchmarkRunId = $1",
                &[&run_id],
            )
            .await
            .expect("BenchmarkRun row must exist");
        let contract_version: &str = benchmark_run.get(0);
        assert_eq!(contract_version, "1.2");
        let target_host: &str = benchmark_run.get(1);
        assert_eq!(target_host, "localhost");

        let env_rows = c
            .query(
                "SELECT 1 FROM BenchmarkEnvironment WHERE BenchmarkRunId = $1",
                &[&run_id],
            )
            .await
            .unwrap();
        assert_eq!(env_rows.len(), 1);

        let quality_rows = c
            .query(
                "SELECT 1 FROM BenchmarkDataQuality WHERE BenchmarkRunId = $1",
                &[&run_id],
            )
            .await
            .unwrap();
        assert_eq!(quality_rows.len(), 1);

        let launch_row = c
            .query_one(
                "SELECT LaunchIndex, Scenario, PrimaryPhase, WarmupSampleCount
                 FROM BenchmarkLaunch WHERE BenchmarkRunId = $1",
                &[&run_id],
            )
            .await
            .expect("BenchmarkLaunch row");
        let launch_index: i32 = launch_row.get(0);
        assert_eq!(launch_index, 0);
        let scenario: &str = launch_row.get(1);
        assert_eq!(scenario, "warm");
        let primary_phase: &str = launch_row.get(2);
        assert_eq!(primary_phase, "measured");
        let warmup_sample_count: i64 = launch_row.get(3);
        assert_eq!(warmup_sample_count, 0);

        let case_row = c
            .query_one(
                "SELECT CaseId, Protocol, MetricUnit
                 FROM BenchmarkCase WHERE BenchmarkRunId = $1",
                &[&run_id],
            )
            .await
            .expect("BenchmarkCase row");
        let case_id: &str = case_row.get(0);
        assert_eq!(case_id, "http1:default:default");
        let protocol: &str = case_row.get(1);
        assert_eq!(protocol, "http1");
        let metric_unit: &str = case_row.get(2);
        assert_eq!(metric_unit, "ms");

        let sample_row = c
            .query_one(
                "SELECT InclusionStatus, MetricUnit
                 FROM BenchmarkSample WHERE BenchmarkRunId = $1",
                &[&run_id],
            )
            .await
            .expect("BenchmarkSample row");
        let inclusion_status: &str = sample_row.get(0);
        assert_eq!(inclusion_status, "excluded_missing_metric");
        let sample_metric_unit: &str = sample_row.get(1);
        assert_eq!(sample_metric_unit, "ms");

        let summary_row = c
            .query_one(
                "SELECT SampleCount, IncludedSampleCount, FailureCount
                 FROM BenchmarkSummary WHERE BenchmarkRunId = $1 AND CaseId = $2",
                &[&run_id, &case_id],
            )
            .await
            .expect("BenchmarkSummary row");
        let sample_count: i64 = summary_row.get(0);
        assert_eq!(sample_count, 1);
        let included_sample_count: i64 = summary_row.get(1);
        assert_eq!(included_sample_count, 0);
        let failure_count: i64 = summary_row.get(2);
        assert_eq!(failure_count, 0);
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn db_postgres_plain_run_succeeds_with_documented_legacy_schema() {
        let base_url = match pg_url() {
            Some(u) => u,
            None => return,
        };
        let (db_url, backend) =
            isolated_backend_with_documented_schema(&base_url, "legacy_plain", false).await;
        let run_id = Uuid::new_v4();
        let run = make_run(run_id, vec![bare_attempt(run_id)]);
        backend.save(&run).await.unwrap();

        let client = raw_client(&db_url).await;
        let test_run_count: i64 = client
            .query_one("SELECT COUNT(*) FROM TestRun WHERE RunId = $1", &[&run_id])
            .await
            .unwrap()
            .get(0);
        assert_eq!(test_run_count, 1);

        let attempt_count: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM RequestAttempt WHERE RunId = $1",
                &[&run_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(attempt_count, 1);

        let benchmark_tables_present: bool = client
            .query_one("SELECT to_regclass('public.benchmarkrun') IS NOT NULL", &[])
            .await
            .unwrap()
            .get(0);
        assert!(!benchmark_tables_present);
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn db_postgres_benchmark_run_skips_benchmark_rows_with_documented_legacy_schema() {
        let base_url = match pg_url() {
            Some(u) => u,
            None => return,
        };
        let (db_url, backend) =
            isolated_backend_with_documented_schema(&base_url, "legacy_bench", false).await;
        let run_id = Uuid::new_v4();
        let run = make_benchmark_run(run_id, vec![bare_attempt(run_id)]);
        backend.save(&run).await.unwrap();

        let client = raw_client(&db_url).await;
        let test_run_count: i64 = client
            .query_one("SELECT COUNT(*) FROM TestRun WHERE RunId = $1", &[&run_id])
            .await
            .unwrap()
            .get(0);
        assert_eq!(test_run_count, 1);

        let benchmark_tables_present: bool = client
            .query_one("SELECT to_regclass('public.benchmarkrun') IS NOT NULL", &[])
            .await
            .unwrap()
            .get(0);
        assert!(!benchmark_tables_present);
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn db_postgres_plain_run_does_not_create_benchmark_rows_when_schema_exists() {
        let base_url = match pg_url() {
            Some(u) => u,
            None => return,
        };
        let (db_url, backend) =
            isolated_backend_with_documented_schema(&base_url, "migrated_plain", true).await;
        let run_id = Uuid::new_v4();
        let run = make_run(run_id, vec![bare_attempt(run_id)]);
        backend.save(&run).await.unwrap();

        let client = raw_client(&db_url).await;
        let benchmark_run_count: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM BenchmarkRun WHERE BenchmarkRunId = $1",
                &[&run_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(benchmark_run_count, 0);
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn db_postgres_benchmark_run_persists_rows_with_documented_benchmark_schema() {
        let base_url = match pg_url() {
            Some(u) => u,
            None => return,
        };
        let (db_url, backend) =
            isolated_backend_with_documented_schema(&base_url, "migrated_bench", true).await;
        let run_id = Uuid::new_v4();
        let run = make_benchmark_run(run_id, vec![bare_attempt(run_id)]);
        backend.save(&run).await.unwrap();

        let client = raw_client(&db_url).await;
        let benchmark_run = client
            .query_one(
                "SELECT ContractVersion, TargetHost
                 FROM BenchmarkRun WHERE BenchmarkRunId = $1",
                &[&run_id],
            )
            .await
            .expect("BenchmarkRun row must exist");
        let contract_version: &str = benchmark_run.get(0);
        assert_eq!(contract_version, "1.2");
        let target_host: &str = benchmark_run.get(1);
        assert_eq!(target_host, "localhost");

        let launch_count: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM BenchmarkLaunch WHERE BenchmarkRunId = $1",
                &[&run_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(launch_count, 1);
    }
}
