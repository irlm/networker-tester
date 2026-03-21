-- =============================================================================
-- Networker Tester – URL Page Load Diagnostic foundation schema
-- SQL Server 2017+ / Azure SQL Database
--
-- Run after: 01_CreateDatabase.sql through 07_MoreTcpStats.sql
-- =============================================================================

USE NetworkDiagnostics;
GO

IF NOT EXISTS (SELECT * FROM sys.objects WHERE object_id = OBJECT_ID(N'dbo.UrlTestRun') AND type = 'U')
CREATE TABLE dbo.UrlTestRun (
    Id                          NVARCHAR(36)    NOT NULL,
    StartedAt                   DATETIME2(3)    NOT NULL,
    CompletedAt                 DATETIME2(3)    NULL,
    RequestedUrl                NVARCHAR(2048)  NOT NULL,
    FinalUrl                    NVARCHAR(2048)  NULL,
    Status                      NVARCHAR(32)    NOT NULL,
    PageLoadStrategy            NVARCHAR(32)    NOT NULL,
    BrowserEngine               NVARCHAR(64)    NULL,
    BrowserVersion              NVARCHAR(64)    NULL,
    UserAgent                   NVARCHAR(MAX)   NULL,
    PrimaryOrigin               NVARCHAR(1024)  NULL,
    ObservedProtocolPrimaryLoad NVARCHAR(32)    NULL,
    AdvertisedAltSvc            NVARCHAR(MAX)   NULL,
    ValidatedHttpVersions       NVARCHAR(128)   NOT NULL DEFAULT N'',
    TlsVersion                  NVARCHAR(32)    NULL,
    CipherSuite                 NVARCHAR(128)   NULL,
    Alpn                        NVARCHAR(32)    NULL,
    DnsMs                       FLOAT           NULL,
    ConnectMs                   FLOAT           NULL,
    HandshakeMs                 FLOAT           NULL,
    TtfbMs                      FLOAT           NULL,
    DomContentLoadedMs          FLOAT           NULL,
    LoadEventMs                 FLOAT           NULL,
    NetworkIdleMs               FLOAT           NULL,
    CaptureEndMs                FLOAT           NULL,
    TotalRequests               INT             NOT NULL DEFAULT 0,
    TotalTransferBytes          BIGINT          NOT NULL DEFAULT 0,
    PeakConcurrentConnections   INT             NULL,
    RedirectCount               INT             NOT NULL DEFAULT 0,
    FailureCount                INT             NOT NULL DEFAULT 0,
    HarPath                     NVARCHAR(MAX)   NULL,
    PcapPath                    NVARCHAR(MAX)   NULL,
    PcapSummaryJson             NVARCHAR(MAX)   NULL,
    CaptureErrors               NVARCHAR(MAX)   NULL,
    EnvironmentNotes            NVARCHAR(MAX)   NULL,
    CONSTRAINT PK_UrlTestRun PRIMARY KEY (Id)
);
GO

IF NOT EXISTS (SELECT * FROM sys.objects WHERE object_id = OBJECT_ID(N'dbo.UrlTestResource') AND type = 'U')
CREATE TABLE dbo.UrlTestResource (
    Id               NVARCHAR(36)    NOT NULL,
    UrlTestRunId     NVARCHAR(36)    NOT NULL,
    ResourceUrl      NVARCHAR(2048)  NOT NULL,
    Origin           NVARCHAR(1024)  NOT NULL,
    ResourceType     NVARCHAR(64)    NOT NULL,
    MimeType         NVARCHAR(255)   NULL,
    StatusCode       INT             NULL,
    Protocol         NVARCHAR(32)    NULL,
    TransferSize     BIGINT          NULL,
    EncodedBodySize  BIGINT          NULL,
    DecodedBodySize  BIGINT          NULL,
    DurationMs       FLOAT           NULL,
    ConnectionId     NVARCHAR(128)   NULL,
    ReusedConnection BIT             NULL,
    InitiatorType    NVARCHAR(64)    NULL,
    FromCache        BIT             NULL,
    Redirected       BIT             NULL,
    Failed           BIT             NOT NULL DEFAULT 0,
    CONSTRAINT PK_UrlTestResource PRIMARY KEY (Id),
    CONSTRAINT FK_UrlTestResource_Run FOREIGN KEY (UrlTestRunId)
        REFERENCES dbo.UrlTestRun (Id)
        ON DELETE CASCADE
);
GO

IF NOT EXISTS (SELECT * FROM sys.objects WHERE object_id = OBJECT_ID(N'dbo.UrlTestProtocolRun') AND type = 'U')
CREATE TABLE dbo.UrlTestProtocolRun (
    Id               NVARCHAR(36)    NOT NULL,
    UrlTestRunId     NVARCHAR(36)    NOT NULL,
    ProtocolMode     NVARCHAR(16)    NOT NULL,
    RunNumber        INT             NOT NULL,
    AttemptType      NVARCHAR(16)    NOT NULL,
    ObservedProtocol NVARCHAR(32)    NULL,
    FallbackOccurred BIT             NULL,
    Succeeded        BIT             NOT NULL DEFAULT 0,
    StatusCode       INT             NULL,
    TtfbMs           FLOAT           NULL,
    TotalMs          FLOAT           NULL,
    FailureReason    NVARCHAR(MAX)   NULL,
    Error            NVARCHAR(MAX)   NULL,
    CONSTRAINT PK_UrlTestProtocolRun PRIMARY KEY (Id),
    CONSTRAINT FK_UrlTestProtocolRun_Run FOREIGN KEY (UrlTestRunId)
        REFERENCES dbo.UrlTestRun (Id)
        ON DELETE CASCADE
);
GO

IF NOT EXISTS (SELECT 1 FROM sys.indexes WHERE name = N'IX_UrlTestRun_StartedAt' AND object_id = OBJECT_ID(N'dbo.UrlTestRun'))
    CREATE INDEX IX_UrlTestRun_StartedAt ON dbo.UrlTestRun (StartedAt DESC);
IF NOT EXISTS (SELECT 1 FROM sys.indexes WHERE name = N'IX_UrlTestRun_Status' AND object_id = OBJECT_ID(N'dbo.UrlTestRun'))
    CREATE INDEX IX_UrlTestRun_Status ON dbo.UrlTestRun (Status, StartedAt DESC);
IF NOT EXISTS (SELECT 1 FROM sys.indexes WHERE name = N'IX_UrlTestResource_RunId' AND object_id = OBJECT_ID(N'dbo.UrlTestResource'))
    CREATE INDEX IX_UrlTestResource_RunId ON dbo.UrlTestResource (UrlTestRunId);
IF NOT EXISTS (SELECT 1 FROM sys.indexes WHERE name = N'IX_UrlTestProtocolRun_RunId' AND object_id = OBJECT_ID(N'dbo.UrlTestProtocolRun'))
    CREATE INDEX IX_UrlTestProtocolRun_RunId ON dbo.UrlTestProtocolRun (UrlTestRunId, ProtocolMode, RunNumber);
GO

PRINT 'URL page-load diagnostic foundation schema created / verified successfully.';
GO
