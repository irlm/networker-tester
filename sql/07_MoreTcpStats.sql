-- ============================================================================
-- 07_MoreTcpStats.sql
-- Adds three new TCP kernel stat columns to dbo.TcpResult:
--   CongestionAlgorithm  – TCP_CONGESTION algorithm name (e.g. "cubic", "bbr")
--   DeliveryRateBps      – tcpi_delivery_rate (Linux ≥ 4.9), bytes/sec
--   MinRttMs             – tcpi_min_rtt (Linux ≥ 4.9), milliseconds
--
-- All additions are idempotent (IF NOT EXISTS guard).
-- Run after 05_ExtendedTcpStats.sql.
-- ============================================================================
USE NetworkDiagnostics;
GO

IF NOT EXISTS (
    SELECT 1 FROM sys.columns
    WHERE object_id = OBJECT_ID('dbo.TcpResult') AND name = 'CongestionAlgorithm'
)
    ALTER TABLE dbo.TcpResult ADD CongestionAlgorithm NVARCHAR(32) NULL;
GO

IF NOT EXISTS (
    SELECT 1 FROM sys.columns
    WHERE object_id = OBJECT_ID('dbo.TcpResult') AND name = 'DeliveryRateBps'
)
    ALTER TABLE dbo.TcpResult ADD DeliveryRateBps BIGINT NULL;
GO

IF NOT EXISTS (
    SELECT 1 FROM sys.columns
    WHERE object_id = OBJECT_ID('dbo.TcpResult') AND name = 'MinRttMs'
)
    ALTER TABLE dbo.TcpResult ADD MinRttMs FLOAT NULL;
GO
