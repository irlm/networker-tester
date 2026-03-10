-- =============================================================================
-- Networker Tester – PostgreSQL Schema
-- PostgreSQL 14+
--
-- This file is for documentation; the actual migration is embedded in
-- crates/networker-tester/src/output/db/postgres.rs and applied automatically
-- via `--db-migrate`.
--
-- Quick start (Docker):
--   docker compose -f docker-compose.db.yml up -d postgres
--   NETWORKER_DB_URL=postgres://networker:networker@localhost/network_diagnostics \
--     cargo run -- --save-to-db --db-migrate --target http://localhost:8080/health
-- =============================================================================

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
    ErrorId        UUID         NOT NULL,
    AttemptId      UUID         NULL,
    RunId          UUID         NOT NULL,
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
