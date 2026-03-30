-- =============================================================================
-- Networker Tester – PostgreSQL Benchmark Schema
-- PostgreSQL 14+
--
-- This file documents the benchmark-oriented schema added in V003.
-- The runtime migration source of truth remains:
-- crates/networker-tester/src/output/db/postgres.rs
-- =============================================================================

-- V003: Add normalized benchmark storage tables

CREATE TABLE IF NOT EXISTS BenchmarkRun (
    BenchmarkRunId       UUID            NOT NULL,
    ContractVersion      VARCHAR(20)     NOT NULL,
    GeneratedAt          TIMESTAMPTZ     NOT NULL,
    Source               VARCHAR(64)     NOT NULL,
    TargetUrl            VARCHAR(2048)   NOT NULL,
    TargetHost           VARCHAR(255)    NOT NULL,
    Modes                VARCHAR(200)    NOT NULL,
    TotalRuns            INT             NOT NULL,
    Concurrency          INT             NOT NULL,
    TimeoutMs            BIGINT          NOT NULL,
    ClientOs             VARCHAR(50)     NOT NULL,
    ClientVersion        VARCHAR(50)     NOT NULL,
    MethodologyJson      JSONB           NOT NULL,
    DiagnosticsJson      JSONB           NOT NULL,
    AggregateSummaryJson JSONB           NOT NULL,
    CONSTRAINT PK_BenchmarkRun PRIMARY KEY (BenchmarkRunId),
    CONSTRAINT FK_BenchmarkRun_TestRun FOREIGN KEY (BenchmarkRunId)
        REFERENCES TestRun (RunId) ON DELETE CASCADE
);

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
CREATE INDEX IF NOT EXISTS IX_BenchmarkLaunch_Phase
    ON BenchmarkLaunch (PrimaryPhase, Scenario, BenchmarkRunId);
CREATE INDEX IF NOT EXISTS IX_BenchmarkCase_Protocol
    ON BenchmarkCase (Protocol, BenchmarkRunId);
CREATE INDEX IF NOT EXISTS IX_BenchmarkSample_RunCase
    ON BenchmarkSample (BenchmarkRunId, CaseId, Phase, Success);
CREATE INDEX IF NOT EXISTS IX_BenchmarkSummary_RunProtocol
    ON BenchmarkSummary (BenchmarkRunId, Protocol);
CREATE INDEX IF NOT EXISTS IX_BenchmarkDataQuality_PublicationReady
    ON BenchmarkDataQuality (PublicationReady, NoiseLevel);
