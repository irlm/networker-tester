-- =============================================================================
-- Networker Tester – Database Schema
-- SQL Server 2017+ / Azure SQL Database
--
-- Run order: 01_CreateDatabase.sql → 02_StoredProcedures.sql → 03_SampleQueries.sql
--
-- Quick start (Docker):
--   docker run -e ACCEPT_EULA=Y -e SA_PASSWORD="YourPass1!" \
--              -p 1433:1433 --name sql1 -d mcr.microsoft.com/mssql/server:2022-latest
--   sqlcmd -S localhost -U sa -P "YourPass1!" -i sql/01_CreateDatabase.sql
-- =============================================================================

-- ── 1. Database ------------------------------------------------------------------
IF NOT EXISTS (SELECT name FROM sys.databases WHERE name = N'NetworkDiagnostics')
BEGIN
    CREATE DATABASE NetworkDiagnostics
    COLLATE SQL_Latin1_General_CP1_CI_AS;
END
GO

USE NetworkDiagnostics;
GO

-- ── 2. Tables -------------------------------------------------------------------

-- TestRun: one row per `networker-tester` invocation.
IF NOT EXISTS (SELECT * FROM sys.objects WHERE object_id = OBJECT_ID(N'dbo.TestRun') AND type = 'U')
CREATE TABLE dbo.TestRun (
    RunId          NVARCHAR(36)    NOT NULL,   -- UUID as string
    StartedAt      DATETIME2(3)    NOT NULL,
    FinishedAt     DATETIME2(3)    NULL,
    TargetUrl      NVARCHAR(2048)  NOT NULL,
    TargetHost     NVARCHAR(255)   NOT NULL,
    Modes          NVARCHAR(200)   NOT NULL,   -- comma-separated: "http1,http2,udp"
    TotalRuns      INT             NOT NULL DEFAULT 1,
    Concurrency    INT             NOT NULL DEFAULT 1,
    TimeoutMs      BIGINT          NOT NULL DEFAULT 30000,
    ClientOs       NVARCHAR(50)    NOT NULL,
    ClientVersion  NVARCHAR(50)    NOT NULL,
    SuccessCount   INT             NOT NULL DEFAULT 0,
    FailureCount   INT             NOT NULL DEFAULT 0,
    CONSTRAINT PK_TestRun PRIMARY KEY (RunId)
);
GO

-- RequestAttempt: one row per protocol probe within a run.
IF NOT EXISTS (SELECT * FROM sys.objects WHERE object_id = OBJECT_ID(N'dbo.RequestAttempt') AND type = 'U')
CREATE TABLE dbo.RequestAttempt (
    AttemptId     NVARCHAR(36)    NOT NULL,
    RunId         NVARCHAR(36)    NOT NULL,
    Protocol      NVARCHAR(20)    NOT NULL,   -- tcp | http1 | http2 | http3 | udp
    SequenceNum   INT             NOT NULL,
    StartedAt     DATETIME2(3)    NOT NULL,
    FinishedAt    DATETIME2(3)    NULL,
    Success       BIT             NOT NULL DEFAULT 0,
    ErrorMessage  NVARCHAR(MAX)   NULL,
    CONSTRAINT PK_RequestAttempt  PRIMARY KEY (AttemptId),
    CONSTRAINT FK_Attempt_Run     FOREIGN KEY (RunId)
        REFERENCES dbo.TestRun (RunId)
        ON DELETE CASCADE
);
GO

-- DnsResult
IF NOT EXISTS (SELECT * FROM sys.objects WHERE object_id = OBJECT_ID(N'dbo.DnsResult') AND type = 'U')
CREATE TABLE dbo.DnsResult (
    DnsId       NVARCHAR(36)    NOT NULL,
    AttemptId   NVARCHAR(36)    NOT NULL,
    QueryName   NVARCHAR(255)   NOT NULL,
    ResolvedIPs NVARCHAR(1024)  NOT NULL,   -- comma-separated IPs
    DurationMs  FLOAT           NOT NULL,
    StartedAt   DATETIME2(3)    NOT NULL,
    Success     BIT             NOT NULL,
    CONSTRAINT PK_DnsResult      PRIMARY KEY (DnsId),
    CONSTRAINT FK_Dns_Attempt    FOREIGN KEY (AttemptId)
        REFERENCES dbo.RequestAttempt (AttemptId)
        ON DELETE CASCADE
);
GO

-- TcpResult
IF NOT EXISTS (SELECT * FROM sys.objects WHERE object_id = OBJECT_ID(N'dbo.TcpResult') AND type = 'U')
CREATE TABLE dbo.TcpResult (
    TcpId              NVARCHAR(36)    NOT NULL,
    AttemptId          NVARCHAR(36)    NOT NULL,
    LocalAddr          NVARCHAR(50)    NULL,
    RemoteAddr         NVARCHAR(50)    NOT NULL,
    ConnectDurationMs  FLOAT           NOT NULL,
    AttemptCount       INT             NOT NULL DEFAULT 1,
    StartedAt          DATETIME2(3)    NOT NULL,
    Success            BIT             NOT NULL,
    -- OS-level socket info (best-effort; NULL when not available)
    MssBytesEstimate   INT             NULL,   -- TCP_MAXSEG (Unix only)
    RttEstimateMs      FLOAT           NULL,   -- tcpi_rtt   (Linux only)
    CONSTRAINT PK_TcpResult     PRIMARY KEY (TcpId),
    CONSTRAINT FK_Tcp_Attempt   FOREIGN KEY (AttemptId)
        REFERENCES dbo.RequestAttempt (AttemptId)
        ON DELETE CASCADE
);
GO

-- TlsResult
IF NOT EXISTS (SELECT * FROM sys.objects WHERE object_id = OBJECT_ID(N'dbo.TlsResult') AND type = 'U')
CREATE TABLE dbo.TlsResult (
    TlsId                NVARCHAR(36)    NOT NULL,
    AttemptId            NVARCHAR(36)    NOT NULL,
    ProtocolVersion      NVARCHAR(20)    NOT NULL,   -- TLSv1.3, TLSv1.2 …
    CipherSuite          NVARCHAR(100)   NOT NULL,
    AlpnNegotiated       NVARCHAR(50)    NULL,       -- h2, http/1.1, h3
    CertSubject          NVARCHAR(500)   NULL,
    CertIssuer           NVARCHAR(500)   NULL,
    CertExpiry           DATETIME2(3)    NULL,
    HandshakeDurationMs  FLOAT           NOT NULL,
    StartedAt            DATETIME2(3)    NOT NULL,
    Success              BIT             NOT NULL,
    CONSTRAINT PK_TlsResult     PRIMARY KEY (TlsId),
    CONSTRAINT FK_Tls_Attempt   FOREIGN KEY (AttemptId)
        REFERENCES dbo.RequestAttempt (AttemptId)
        ON DELETE CASCADE
);
GO

-- HttpResult
IF NOT EXISTS (SELECT * FROM sys.objects WHERE object_id = OBJECT_ID(N'dbo.HttpResult') AND type = 'U')
CREATE TABLE dbo.HttpResult (
    HttpId              NVARCHAR(36)    NOT NULL,
    AttemptId           NVARCHAR(36)    NOT NULL,
    NegotiatedVersion   NVARCHAR(20)    NOT NULL,   -- HTTP/1.1, HTTP/2, HTTP/3
    StatusCode          INT             NOT NULL,
    HeadersSizeBytes    INT             NOT NULL DEFAULT 0,
    BodySizeBytes       INT             NOT NULL DEFAULT 0,
    TtfbMs              FLOAT           NOT NULL,   -- Time to first byte
    TotalDurationMs     FLOAT           NOT NULL,
    RedirectCount       INT             NOT NULL DEFAULT 0,
    StartedAt           DATETIME2(3)    NOT NULL,
    CONSTRAINT PK_HttpResult    PRIMARY KEY (HttpId),
    CONSTRAINT FK_Http_Attempt  FOREIGN KEY (AttemptId)
        REFERENCES dbo.RequestAttempt (AttemptId)
        ON DELETE CASCADE
);
GO

-- UdpResult
IF NOT EXISTS (SELECT * FROM sys.objects WHERE object_id = OBJECT_ID(N'dbo.UdpResult') AND type = 'U')
CREATE TABLE dbo.UdpResult (
    UdpId         NVARCHAR(36)    NOT NULL,
    AttemptId     NVARCHAR(36)    NOT NULL,
    RemoteAddr    NVARCHAR(50)    NOT NULL,
    ProbeCount    INT             NOT NULL,
    SuccessCount  INT             NOT NULL,
    LossPercent   FLOAT           NOT NULL,
    RttMinMs      FLOAT           NOT NULL,
    RttAvgMs      FLOAT           NOT NULL,
    RttP95Ms      FLOAT           NOT NULL,
    JitterMs      FLOAT           NOT NULL,
    StartedAt     DATETIME2(3)    NOT NULL,
    CONSTRAINT PK_UdpResult     PRIMARY KEY (UdpId),
    CONSTRAINT FK_Udp_Attempt   FOREIGN KEY (AttemptId)
        REFERENCES dbo.RequestAttempt (AttemptId)
        ON DELETE CASCADE
);
GO

-- ErrorRecord
IF NOT EXISTS (SELECT * FROM sys.objects WHERE object_id = OBJECT_ID(N'dbo.ErrorRecord') AND type = 'U')
CREATE TABLE dbo.ErrorRecord (
    ErrorId        NVARCHAR(36)    NOT NULL,
    AttemptId      NVARCHAR(36)    NULL,
    RunId          NVARCHAR(36)    NOT NULL,
    ErrorCategory  NVARCHAR(50)    NOT NULL,   -- dns | tcp | tls | http | udp | timeout | config | other
    ErrorMessage   NVARCHAR(MAX)   NOT NULL,
    ErrorDetail    NVARCHAR(MAX)   NULL,
    OccurredAt     DATETIME2(3)    NOT NULL,
    CONSTRAINT PK_ErrorRecord       PRIMARY KEY (ErrorId),
    CONSTRAINT FK_Error_Attempt     FOREIGN KEY (AttemptId)
        REFERENCES dbo.RequestAttempt (AttemptId)
        ON DELETE NO ACTION,   -- attempt may not exist yet on run-level errors
    CONSTRAINT FK_Error_Run         FOREIGN KEY (RunId)
        REFERENCES dbo.TestRun (RunId)
        ON DELETE NO ACTION
);
GO

-- ── 3. Indexes ------------------------------------------------------------------

-- Filter by time range (most common dashboard query)
IF NOT EXISTS (
    SELECT 1 FROM sys.indexes
    WHERE object_id = OBJECT_ID(N'dbo.TestRun') AND name = N'IX_TestRun_StartedAt'
)
    CREATE INDEX IX_TestRun_StartedAt
        ON dbo.TestRun (StartedAt DESC);
GO

-- Filter by target host
IF NOT EXISTS (
    SELECT 1 FROM sys.indexes
    WHERE object_id = OBJECT_ID(N'dbo.TestRun') AND name = N'IX_TestRun_TargetHost'
)
    CREATE INDEX IX_TestRun_TargetHost
        ON dbo.TestRun (TargetHost);
GO

-- Filter by protocol and HTTP version
IF NOT EXISTS (
    SELECT 1 FROM sys.indexes
    WHERE object_id = OBJECT_ID(N'dbo.RequestAttempt') AND name = N'IX_Attempt_Protocol'
)
    CREATE INDEX IX_Attempt_Protocol
        ON dbo.RequestAttempt (Protocol, Success);
GO

IF NOT EXISTS (
    SELECT 1 FROM sys.indexes
    WHERE object_id = OBJECT_ID(N'dbo.HttpResult') AND name = N'IX_HttpResult_Version'
)
    CREATE INDEX IX_HttpResult_Version
        ON dbo.HttpResult (NegotiatedVersion, StatusCode);
GO

-- Filter errors by category
IF NOT EXISTS (
    SELECT 1 FROM sys.indexes
    WHERE object_id = OBJECT_ID(N'dbo.ErrorRecord') AND name = N'IX_Error_Category'
)
    CREATE INDEX IX_Error_Category
        ON dbo.ErrorRecord (ErrorCategory, OccurredAt DESC);
GO

-- Lookup all attempts for a run
IF NOT EXISTS (
    SELECT 1 FROM sys.indexes
    WHERE object_id = OBJECT_ID(N'dbo.RequestAttempt') AND name = N'IX_Attempt_RunId'
)
    CREATE INDEX IX_Attempt_RunId
        ON dbo.RequestAttempt (RunId, SequenceNum);
GO

PRINT 'NetworkDiagnostics schema created / verified successfully.';
GO
