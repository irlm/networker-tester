-- =============================================================================
-- Networker Tester – Migration 05: Extended TCP kernel stats + RetryCount
--
-- Idempotent: each ALTER TABLE is guarded by an IF NOT EXISTS check.
-- Run after 01_CreateDatabase.sql (and optionally 04_AddThroughput.sql).
-- =============================================================================

USE NetworkDiagnostics;
GO

-- ── dbo.TcpResult – 8 new nullable kernel-stats columns ──────────────────────

IF NOT EXISTS (
    SELECT 1 FROM sys.columns
    WHERE object_id = OBJECT_ID('dbo.TcpResult') AND name = 'Retransmits'
)
    ALTER TABLE dbo.TcpResult ADD Retransmits INT NULL;
GO

IF NOT EXISTS (
    SELECT 1 FROM sys.columns
    WHERE object_id = OBJECT_ID('dbo.TcpResult') AND name = 'TotalRetrans'
)
    ALTER TABLE dbo.TcpResult ADD TotalRetrans INT NULL;
GO

IF NOT EXISTS (
    SELECT 1 FROM sys.columns
    WHERE object_id = OBJECT_ID('dbo.TcpResult') AND name = 'SndCwnd'
)
    ALTER TABLE dbo.TcpResult ADD SndCwnd INT NULL;
GO

IF NOT EXISTS (
    SELECT 1 FROM sys.columns
    WHERE object_id = OBJECT_ID('dbo.TcpResult') AND name = 'SndSsthresh'
)
    ALTER TABLE dbo.TcpResult ADD SndSsthresh INT NULL;
GO

IF NOT EXISTS (
    SELECT 1 FROM sys.columns
    WHERE object_id = OBJECT_ID('dbo.TcpResult') AND name = 'RttVarianceMs'
)
    ALTER TABLE dbo.TcpResult ADD RttVarianceMs FLOAT NULL;
GO

IF NOT EXISTS (
    SELECT 1 FROM sys.columns
    WHERE object_id = OBJECT_ID('dbo.TcpResult') AND name = 'RcvSpace'
)
    ALTER TABLE dbo.TcpResult ADD RcvSpace INT NULL;
GO

IF NOT EXISTS (
    SELECT 1 FROM sys.columns
    WHERE object_id = OBJECT_ID('dbo.TcpResult') AND name = 'SegsOut'
)
    ALTER TABLE dbo.TcpResult ADD SegsOut INT NULL;
GO

IF NOT EXISTS (
    SELECT 1 FROM sys.columns
    WHERE object_id = OBJECT_ID('dbo.TcpResult') AND name = 'SegsIn'
)
    ALTER TABLE dbo.TcpResult ADD SegsIn INT NULL;
GO

-- ── dbo.RequestAttempt – add RetryCount ──────────────────────────────────────

IF NOT EXISTS (
    SELECT 1 FROM sys.columns
    WHERE object_id = OBJECT_ID('dbo.RequestAttempt') AND name = 'RetryCount'
)
    ALTER TABLE dbo.RequestAttempt ADD RetryCount INT NOT NULL DEFAULT 0;
GO

-- ── dbo.HttpResult – idempotent throughput migration (04_AddThroughput.sql) ──

IF NOT EXISTS (
    SELECT 1 FROM sys.columns
    WHERE object_id = OBJECT_ID('dbo.HttpResult') AND name = 'PayloadBytes'
)
    ALTER TABLE dbo.HttpResult ADD PayloadBytes BIGINT NULL;
GO

IF NOT EXISTS (
    SELECT 1 FROM sys.columns
    WHERE object_id = OBJECT_ID('dbo.HttpResult') AND name = 'ThroughputMbps'
)
    ALTER TABLE dbo.HttpResult ADD ThroughputMbps FLOAT NULL;
GO

PRINT 'Migration 05_ExtendedTcpStats completed successfully.';
GO
