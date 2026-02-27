-- =============================================================================
-- Migration: Add throughput columns to HttpResult
-- Run this against an existing NetworkDiagnostics database to add the
-- PayloadBytes and ThroughputMbps columns introduced in v0.2.
-- Idempotent: safe to run more than once.
-- =============================================================================

USE NetworkDiagnostics;
GO

IF NOT EXISTS (
    SELECT 1 FROM sys.columns
    WHERE object_id = OBJECT_ID(N'dbo.HttpResult') AND name = N'PayloadBytes'
)
    ALTER TABLE dbo.HttpResult ADD PayloadBytes BIGINT NULL;
GO

IF NOT EXISTS (
    SELECT 1 FROM sys.columns
    WHERE object_id = OBJECT_ID(N'dbo.HttpResult') AND name = N'ThroughputMbps'
)
    ALTER TABLE dbo.HttpResult ADD ThroughputMbps FLOAT NULL;
GO

-- Filtered index speeds up throughput-specific queries while keeping it small.
IF NOT EXISTS (
    SELECT 1 FROM sys.indexes
    WHERE object_id = OBJECT_ID(N'dbo.HttpResult') AND name = N'IX_HttpResult_Throughput'
)
    CREATE INDEX IX_HttpResult_Throughput
        ON dbo.HttpResult (ThroughputMbps)
        WHERE ThroughputMbps IS NOT NULL;
GO
