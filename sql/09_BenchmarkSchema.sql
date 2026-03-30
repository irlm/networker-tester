-- =============================================================================
-- Networker Tester – Benchmark measurement and reporting schema
-- SQL Server 2017+ / Azure SQL Database
--
-- Run after: 01_CreateDatabase.sql through 08_UrlDiagnostics.sql
-- =============================================================================

USE NetworkDiagnostics;
GO

IF NOT EXISTS (SELECT * FROM sys.objects WHERE object_id = OBJECT_ID(N'dbo.BenchmarkRun') AND type = 'U')
CREATE TABLE dbo.BenchmarkRun (
    BenchmarkRunId       NVARCHAR(36)    NOT NULL,
    ContractVersion      NVARCHAR(20)    NOT NULL,
    GeneratedAt          DATETIME2(3)    NOT NULL,
    Source               NVARCHAR(64)    NOT NULL,
    TargetUrl            NVARCHAR(2048)  NOT NULL,
    TargetHost           NVARCHAR(255)   NOT NULL,
    Modes                NVARCHAR(200)   NOT NULL,
    TotalRuns            INT             NOT NULL,
    Concurrency          INT             NOT NULL,
    TimeoutMs            BIGINT          NOT NULL,
    ClientOs             NVARCHAR(50)    NOT NULL,
    ClientVersion        NVARCHAR(50)    NOT NULL,
    MethodologyJson      NVARCHAR(MAX)   NOT NULL,
    DiagnosticsJson      NVARCHAR(MAX)   NOT NULL,
    AggregateSummaryJson NVARCHAR(MAX)   NOT NULL,
    CONSTRAINT PK_BenchmarkRun PRIMARY KEY (BenchmarkRunId),
    CONSTRAINT FK_BenchmarkRun_TestRun FOREIGN KEY (BenchmarkRunId)
        REFERENCES dbo.TestRun (RunId)
        ON DELETE CASCADE
);
GO

IF NOT EXISTS (SELECT * FROM sys.objects WHERE object_id = OBJECT_ID(N'dbo.BenchmarkLaunch') AND type = 'U')
CREATE TABLE dbo.BenchmarkLaunch (
    BenchmarkRunId      NVARCHAR(36)   NOT NULL,
    LaunchIndex         INT            NOT NULL,
    Scenario            NVARCHAR(64)   NOT NULL,
    PrimaryPhase        NVARCHAR(32)   NOT NULL,
    StartedAt           DATETIME2(3)   NOT NULL,
    FinishedAt          DATETIME2(3)   NULL,
    SampleCount         BIGINT         NOT NULL,
    PrimarySampleCount  BIGINT         NOT NULL,
    WarmupSampleCount   BIGINT         NOT NULL,
    SuccessCount        BIGINT         NOT NULL,
    FailureCount        BIGINT         NOT NULL,
    PhasesJson          NVARCHAR(MAX)  NOT NULL,
    CONSTRAINT PK_BenchmarkLaunch PRIMARY KEY (BenchmarkRunId, LaunchIndex),
    CONSTRAINT FK_BenchmarkLaunch_Run FOREIGN KEY (BenchmarkRunId)
        REFERENCES dbo.BenchmarkRun (BenchmarkRunId)
        ON DELETE CASCADE
);
GO

IF NOT EXISTS (SELECT * FROM sys.objects WHERE object_id = OBJECT_ID(N'dbo.BenchmarkEnvironment') AND type = 'U')
CREATE TABLE dbo.BenchmarkEnvironment (
    BenchmarkRunId       NVARCHAR(36)   NOT NULL,
    ClientInfoJson       NVARCHAR(MAX)  NULL,
    ServerInfoJson       NVARCHAR(MAX)  NULL,
    NetworkBaselineJson  NVARCHAR(MAX)  NULL,
    PacketCaptureEnabled BIT            NOT NULL DEFAULT 0,
    EnvironmentJson      NVARCHAR(MAX)  NOT NULL,
    CONSTRAINT PK_BenchmarkEnvironment PRIMARY KEY (BenchmarkRunId),
    CONSTRAINT FK_BenchmarkEnvironment_Run FOREIGN KEY (BenchmarkRunId)
        REFERENCES dbo.BenchmarkRun (BenchmarkRunId)
        ON DELETE CASCADE
);
GO

IF NOT EXISTS (SELECT * FROM sys.objects WHERE object_id = OBJECT_ID(N'dbo.BenchmarkDataQuality') AND type = 'U')
CREATE TABLE dbo.BenchmarkDataQuality (
    BenchmarkRunId     NVARCHAR(36)   NOT NULL,
    NoiseLevel         NVARCHAR(16)   NOT NULL,
    SampleStabilityCv  FLOAT          NOT NULL,
    Sufficiency        NVARCHAR(16)   NOT NULL,
    PublicationReady   BIT            NOT NULL DEFAULT 0,
    WarningsJson       NVARCHAR(MAX)  NOT NULL,
    QualityJson        NVARCHAR(MAX)  NOT NULL,
    CONSTRAINT PK_BenchmarkDataQuality PRIMARY KEY (BenchmarkRunId),
    CONSTRAINT FK_BenchmarkDataQuality_Run FOREIGN KEY (BenchmarkRunId)
        REFERENCES dbo.BenchmarkRun (BenchmarkRunId)
        ON DELETE CASCADE
);
GO

IF NOT EXISTS (SELECT * FROM sys.objects WHERE object_id = OBJECT_ID(N'dbo.BenchmarkCase') AND type = 'U')
CREATE TABLE dbo.BenchmarkCase (
    BenchmarkRunId   NVARCHAR(36)   NOT NULL,
    CaseId           NVARCHAR(255)  NOT NULL,
    Protocol         NVARCHAR(32)   NOT NULL,
    PayloadBytes     BIGINT         NULL,
    HttpStack        NVARCHAR(128)  NULL,
    MetricName       NVARCHAR(64)   NOT NULL,
    MetricUnit       NVARCHAR(32)   NOT NULL,
    HigherIsBetter   BIT            NOT NULL,
    CaseJson         NVARCHAR(MAX)  NOT NULL,
    CONSTRAINT PK_BenchmarkCase PRIMARY KEY (BenchmarkRunId, CaseId),
    CONSTRAINT FK_BenchmarkCase_Run FOREIGN KEY (BenchmarkRunId)
        REFERENCES dbo.BenchmarkRun (BenchmarkRunId)
        ON DELETE CASCADE
);
GO

IF NOT EXISTS (SELECT * FROM sys.objects WHERE object_id = OBJECT_ID(N'dbo.BenchmarkSample') AND type = 'U')
CREATE TABLE dbo.BenchmarkSample (
    AttemptId        NVARCHAR(36)   NOT NULL,
    BenchmarkRunId   NVARCHAR(36)   NOT NULL,
    CaseId           NVARCHAR(255)  NOT NULL,
    LaunchIndex      INT            NOT NULL DEFAULT 0,
    Phase            NVARCHAR(32)   NOT NULL,
    IterationIndex   INT            NOT NULL,
    Success          BIT            NOT NULL DEFAULT 0,
    RetryCount       INT            NOT NULL DEFAULT 0,
    InclusionStatus  NVARCHAR(64)   NOT NULL,
    MetricValue      FLOAT          NULL,
    MetricUnit       NVARCHAR(32)   NOT NULL,
    StartedAt        DATETIME2(3)   NOT NULL,
    FinishedAt       DATETIME2(3)   NULL,
    TotalDurationMs  FLOAT          NULL,
    TtfbMs           FLOAT          NULL,
    SampleJson       NVARCHAR(MAX)  NOT NULL,
    CONSTRAINT PK_BenchmarkSample PRIMARY KEY (AttemptId),
    CONSTRAINT FK_BenchmarkSample_Run FOREIGN KEY (BenchmarkRunId)
        REFERENCES dbo.BenchmarkRun (BenchmarkRunId)
        ON DELETE CASCADE,
    CONSTRAINT FK_BenchmarkSample_Case FOREIGN KEY (BenchmarkRunId, CaseId)
        REFERENCES dbo.BenchmarkCase (BenchmarkRunId, CaseId),
    CONSTRAINT FK_BenchmarkSample_Attempt FOREIGN KEY (AttemptId)
        REFERENCES dbo.RequestAttempt (AttemptId)
);
GO

IF NOT EXISTS (SELECT * FROM sys.objects WHERE object_id = OBJECT_ID(N'dbo.BenchmarkSummary') AND type = 'U')
CREATE TABLE dbo.BenchmarkSummary (
    BenchmarkRunId       NVARCHAR(36)   NOT NULL,
    CaseId               NVARCHAR(255)  NOT NULL,
    Protocol             NVARCHAR(32)   NOT NULL,
    PayloadBytes         BIGINT         NULL,
    HttpStack            NVARCHAR(128)  NULL,
    MetricName           NVARCHAR(64)   NOT NULL,
    MetricUnit           NVARCHAR(32)   NOT NULL,
    HigherIsBetter       BIT            NOT NULL,
    SampleCount          BIGINT         NOT NULL,
    IncludedSampleCount  BIGINT         NOT NULL,
    ExcludedSampleCount  BIGINT         NOT NULL,
    SuccessCount         BIGINT         NOT NULL,
    FailureCount         BIGINT         NOT NULL,
    TotalRequests        BIGINT         NOT NULL,
    ErrorCount           BIGINT         NOT NULL,
    BytesTransferred     BIGINT         NOT NULL,
    WallTimeMs           FLOAT          NOT NULL,
    Rps                  FLOAT          NOT NULL,
    Min                  FLOAT          NOT NULL,
    Mean                 FLOAT          NOT NULL,
    P5                   FLOAT          NOT NULL,
    P25                  FLOAT          NOT NULL,
    P50                  FLOAT          NOT NULL,
    P75                  FLOAT          NOT NULL,
    P95                  FLOAT          NOT NULL,
    P99                  FLOAT          NOT NULL,
    P999                 FLOAT          NOT NULL,
    Max                  FLOAT          NOT NULL,
    Stddev               FLOAT          NOT NULL,
    LatencyMeanMs        FLOAT          NULL,
    LatencyP50Ms         FLOAT          NULL,
    LatencyP99Ms         FLOAT          NULL,
    LatencyP999Ms        FLOAT          NULL,
    LatencyMaxMs         FLOAT          NULL,
    SummaryJson          NVARCHAR(MAX)  NOT NULL,
    CONSTRAINT PK_BenchmarkSummary PRIMARY KEY (BenchmarkRunId, CaseId),
    CONSTRAINT FK_BenchmarkSummary_Run FOREIGN KEY (BenchmarkRunId)
        REFERENCES dbo.BenchmarkRun (BenchmarkRunId)
        ON DELETE CASCADE
);
GO

IF NOT EXISTS (SELECT 1 FROM sys.indexes WHERE name = N'IX_BenchmarkRun_GeneratedAt' AND object_id = OBJECT_ID(N'dbo.BenchmarkRun'))
    CREATE INDEX IX_BenchmarkRun_GeneratedAt ON dbo.BenchmarkRun (GeneratedAt DESC);
IF NOT EXISTS (SELECT 1 FROM sys.indexes WHERE name = N'IX_BenchmarkLaunch_Phase' AND object_id = OBJECT_ID(N'dbo.BenchmarkLaunch'))
    CREATE INDEX IX_BenchmarkLaunch_Phase ON dbo.BenchmarkLaunch (PrimaryPhase, Scenario, BenchmarkRunId);
IF NOT EXISTS (SELECT 1 FROM sys.indexes WHERE name = N'IX_BenchmarkCase_Protocol' AND object_id = OBJECT_ID(N'dbo.BenchmarkCase'))
    CREATE INDEX IX_BenchmarkCase_Protocol ON dbo.BenchmarkCase (Protocol, BenchmarkRunId);
IF NOT EXISTS (SELECT 1 FROM sys.indexes WHERE name = N'IX_BenchmarkSample_RunCase' AND object_id = OBJECT_ID(N'dbo.BenchmarkSample'))
    CREATE INDEX IX_BenchmarkSample_RunCase ON dbo.BenchmarkSample (BenchmarkRunId, CaseId, Phase, Success);
IF NOT EXISTS (SELECT 1 FROM sys.indexes WHERE name = N'IX_BenchmarkSummary_RunProtocol' AND object_id = OBJECT_ID(N'dbo.BenchmarkSummary'))
    CREATE INDEX IX_BenchmarkSummary_RunProtocol ON dbo.BenchmarkSummary (BenchmarkRunId, Protocol);
IF NOT EXISTS (SELECT 1 FROM sys.indexes WHERE name = N'IX_BenchmarkDataQuality_PublicationReady' AND object_id = OBJECT_ID(N'dbo.BenchmarkDataQuality'))
    CREATE INDEX IX_BenchmarkDataQuality_PublicationReady ON dbo.BenchmarkDataQuality (PublicationReady, NoiseLevel);
GO

PRINT 'Benchmark measurement and reporting schema created / verified successfully.';
GO
