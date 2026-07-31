-- =============================================================================
-- Networker Tester – Migration 10: MthroughputResult table + SrvCpuMs column
--
-- Persists the multi-connection capacity probe (mthroughput mode) so a
-- completed run's attempts carry the multi-stream capacity numbers the
-- dashboard's infrastructure envelope uses as its EMPIRICAL ceiling — they
-- previously existed only in the tester's JSON output / live attempt stream.
-- Column names mirror the Rust MthroughputResult struct fields; note
-- Capacity*Mbps carries MB/s (decimal), matching the struct.
--
-- Also adds ServerTimingResult.SrvCpuMs: endpoint process-CPU milliseconds
-- across the upload drain window (Server-Timing `cpu;dur`), the server-side
-- CPU-bound evidence. The tester probes for this column and degrades to the
-- old 8-column insert when absent, so applying this script is optional but
-- required to capture the new field.
--
-- Run after 06_ServerTiming.sql. (Mirrors the tester-managed PostgreSQL V005
-- migration in crates/networker-tester/src/output/db/postgres.rs.)
-- =============================================================================

USE NetworkDiagnostics;
GO

IF NOT EXISTS (
    SELECT 1 FROM sys.tables WHERE name = 'MthroughputResult' AND schema_id = SCHEMA_ID('dbo')
)
BEGIN
    CREATE TABLE dbo.MthroughputResult (
        ServerId               NVARCHAR(36)   NOT NULL
                                   CONSTRAINT PK_MthroughputResult PRIMARY KEY,
        AttemptId              NVARCHAR(36)   NOT NULL
                                   CONSTRAINT FK_MthroughputResult_Attempt
                                   REFERENCES dbo.RequestAttempt(AttemptId),
        -- Endpoint base URL the load ran against
        RemoteAddr             NVARCHAR(256)  NOT NULL,
        -- Aggregate capacity at saturation, per direction (MB/s decimal)
        CapacityDownMbps       FLOAT          NULL,
        CapacityUpMbps         FLOAT          NULL,
        -- Parallel connections open at saturation
        ConnsDown              INT            NOT NULL,
        ConnsUp                INT            NULL,
        -- Per-connection fair-share spread (%)
        FairShareSpreadDownPct FLOAT          NULL,
        FairShareSpreadUpPct   FLOAT          NULL
    );

    CREATE INDEX IX_MthroughputResult_AttemptId
        ON dbo.MthroughputResult (AttemptId);
END
GO

IF COL_LENGTH(N'dbo.ServerTimingResult', N'SrvCpuMs') IS NULL
BEGIN
    ALTER TABLE dbo.ServerTimingResult ADD SrvCpuMs FLOAT NULL;
END
GO
