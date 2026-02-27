-- =============================================================================
-- Networker Tester – Migration 06: ServerTimingResult table
--
-- Stores X-Networker-* and Server-Timing header data captured per attempt.
-- Run after 01_CreateDatabase.sql and 05_ExtendedTcpStats.sql.
-- =============================================================================

USE NetworkDiagnostics;
GO

IF NOT EXISTS (
    SELECT 1 FROM sys.tables WHERE name = 'ServerTimingResult' AND schema_id = SCHEMA_ID('dbo')
)
BEGIN
    CREATE TABLE dbo.ServerTimingResult (
        ServerId        UNIQUEIDENTIFIER NOT NULL
                            CONSTRAINT PK_ServerTimingResult PRIMARY KEY
                            DEFAULT NEWSEQUENTIALID(),
        AttemptId       UNIQUEIDENTIFIER NOT NULL
                            CONSTRAINT FK_ServerTimingResult_Attempt
                            REFERENCES dbo.RequestAttempt(AttemptId),
        -- X-Networker-Request-Id echoed from the response
        RequestId       NVARCHAR(128)    NULL,
        -- X-Networker-Server-Timestamp parsed to UTC
        ServerTimestamp DATETIME2        NULL,
        -- Estimated one-way clock skew: (server_ts − client_send_at) − ttfb_ms/2
        ClockSkewMs     FLOAT            NULL,
        -- Server-Timing: recv;dur=X  (upload body drain time)
        RecvBodyMs      FLOAT            NULL,
        -- Server-Timing: proc;dur=X  (download allocation / processing time)
        ProcessingMs    FLOAT            NULL,
        -- Server-Timing: total;dur=X
        TotalServerMs   FLOAT            NULL
    );
END
GO

-- Index for fast lookup by attempt
IF NOT EXISTS (
    SELECT 1 FROM sys.indexes
    WHERE object_id = OBJECT_ID('dbo.ServerTimingResult')
      AND name = 'IX_ServerTimingResult_AttemptId'
)
    CREATE NONCLUSTERED INDEX IX_ServerTimingResult_AttemptId
        ON dbo.ServerTimingResult (AttemptId);
GO

PRINT 'Migration 06_ServerTiming completed successfully.';
GO
