-- =============================================================================
-- Networker Tester – Sample / Diagnostic Queries
-- =============================================================================

USE NetworkDiagnostics;
GO

-- ── 1. Latest 10 runs ────────────────────────────────────────────────────────
SELECT TOP 10
    r.RunId,
    r.StartedAt,
    r.TargetHost,
    r.Modes,
    r.SuccessCount,
    r.FailureCount,
    r.ClientOs
FROM dbo.TestRun r
ORDER BY r.StartedAt DESC;
GO

-- ── 2. Average latency by protocol for the last 24 hours ────────────────────
SELECT
    a.Protocol,
    COUNT(*)                             AS Attempts,
    AVG(h.TtfbMs)                        AS AvgTtfbMs,
    AVG(h.TotalDurationMs)               AS AvgTotalMs,
    AVG(t.ConnectDurationMs)             AS AvgTcpMs,
    AVG(tls.HandshakeDurationMs)         AS AvgTlsMs,
    SUM(CASE WHEN a.Success = 1 THEN 1 ELSE 0 END) AS Successes
FROM dbo.RequestAttempt a
LEFT JOIN dbo.HttpResult  h   ON h.AttemptId = a.AttemptId
LEFT JOIN dbo.TcpResult   t   ON t.AttemptId = a.AttemptId
LEFT JOIN dbo.TlsResult   tls ON tls.AttemptId = a.AttemptId
WHERE a.StartedAt >= DATEADD(HOUR, -24, GETUTCDATE())
GROUP BY a.Protocol
ORDER BY a.Protocol;
GO

-- ── 3. HTTP version distribution ─────────────────────────────────────────────
SELECT
    h.NegotiatedVersion,
    COUNT(*)                 AS Count,
    AVG(h.TtfbMs)            AS AvgTtfbMs,
    AVG(h.TotalDurationMs)   AS AvgTotalMs,
    MIN(h.TtfbMs)            AS MinTtfbMs,
    MAX(h.TtfbMs)            AS MaxTtfbMs
FROM dbo.HttpResult h
GROUP BY h.NegotiatedVersion
ORDER BY h.NegotiatedVersion;
GO

-- ── 4. TLS protocol and cipher breakdown ─────────────────────────────────────
SELECT
    tls.ProtocolVersion,
    tls.CipherSuite,
    tls.AlpnNegotiated,
    COUNT(*)                      AS Count,
    AVG(tls.HandshakeDurationMs)  AS AvgHandshakeMs
FROM dbo.TlsResult tls
GROUP BY tls.ProtocolVersion, tls.CipherSuite, tls.AlpnNegotiated
ORDER BY Count DESC;
GO

-- ── 5. UDP packet loss by target ─────────────────────────────────────────────
SELECT
    u.RemoteAddr,
    COUNT(*)          AS ProbeRuns,
    AVG(u.LossPercent) AS AvgLossPct,
    AVG(u.RttAvgMs)    AS AvgRttMs,
    AVG(u.RttP95Ms)    AS AvgP95Ms,
    AVG(u.JitterMs)    AS AvgJitterMs
FROM dbo.UdpResult u
GROUP BY u.RemoteAddr
ORDER BY AvgLossPct DESC;
GO

-- ── 6. Error summary by category ─────────────────────────────────────────────
SELECT
    e.ErrorCategory,
    COUNT(*)                            AS ErrorCount,
    MAX(e.OccurredAt)                   AS LastSeen,
    MIN(e.OccurredAt)                   AS FirstSeen
FROM dbo.ErrorRecord e
GROUP BY e.ErrorCategory
ORDER BY ErrorCount DESC;
GO

-- ── 7. Detailed view of a specific run ───────────────────────────────────────
-- Replace the RunId with a real value from query #1.
DECLARE @RunId NVARCHAR(36) = 'YOUR-RUN-ID-HERE';

SELECT
    a.SequenceNum,
    a.Protocol,
    a.Success,
    d.DurationMs       AS DnsMs,
    d.ResolvedIPs,
    t.ConnectDurationMs AS TcpMs,
    t.LocalAddr,
    t.RemoteAddr,
    t.MssBytesEstimate,
    t.RttEstimateMs,
    tls.ProtocolVersion,
    tls.AlpnNegotiated,
    tls.HandshakeDurationMs AS TlsMs,
    tls.CertSubject,
    tls.CertExpiry,
    h.NegotiatedVersion,
    h.StatusCode,
    h.TtfbMs,
    h.TotalDurationMs,
    h.BodySizeBytes,
    u.LossPercent,
    u.RttAvgMs,
    u.RttP95Ms,
    e.ErrorCategory,
    e.ErrorMessage
FROM dbo.RequestAttempt a
LEFT JOIN dbo.DnsResult  d   ON d.AttemptId  = a.AttemptId
LEFT JOIN dbo.TcpResult  t   ON t.AttemptId  = a.AttemptId
LEFT JOIN dbo.TlsResult  tls ON tls.AttemptId = a.AttemptId
LEFT JOIN dbo.HttpResult h   ON h.AttemptId  = a.AttemptId
LEFT JOIN dbo.UdpResult  u   ON u.AttemptId  = a.AttemptId
LEFT JOIN dbo.ErrorRecord e  ON e.AttemptId  = a.AttemptId
WHERE a.RunId = @RunId
ORDER BY a.SequenceNum;
GO

-- ── 8. Performance regression: compare two runs ──────────────────────────────
-- Useful to see if a deploy changed latency.
DECLARE @RunA NVARCHAR(36) = 'RUN-A-ID';
DECLARE @RunB NVARCHAR(36) = 'RUN-B-ID';

SELECT
    a.Protocol,
    AVG(CASE WHEN a.RunId = @RunA THEN h.TtfbMs END)          AS RunA_AvgTtfbMs,
    AVG(CASE WHEN a.RunId = @RunB THEN h.TtfbMs END)          AS RunB_AvgTtfbMs,
    AVG(CASE WHEN a.RunId = @RunA THEN h.TotalDurationMs END)  AS RunA_AvgTotalMs,
    AVG(CASE WHEN a.RunId = @RunB THEN h.TotalDurationMs END)  AS RunB_AvgTotalMs
FROM dbo.RequestAttempt a
JOIN dbo.HttpResult h ON h.AttemptId = a.AttemptId
WHERE a.RunId IN (@RunA, @RunB)
GROUP BY a.Protocol;
GO

-- ── 9. Slowest requests (p99 equivalent) ─────────────────────────────────────
SELECT TOP 20
    a.Protocol,
    h.NegotiatedVersion,
    h.StatusCode,
    h.TtfbMs,
    h.TotalDurationMs,
    a.StartedAt,
    r.TargetHost
FROM dbo.HttpResult h
JOIN dbo.RequestAttempt a ON a.AttemptId = h.AttemptId
JOIN dbo.TestRun        r ON r.RunId     = a.RunId
ORDER BY h.TotalDurationMs DESC;
GO

-- ── 10. Verify inserts after a run ───────────────────────────────────────────
SELECT
    (SELECT COUNT(*) FROM dbo.TestRun)        AS TestRuns,
    (SELECT COUNT(*) FROM dbo.RequestAttempt) AS Attempts,
    (SELECT COUNT(*) FROM dbo.DnsResult)      AS DnsResults,
    (SELECT COUNT(*) FROM dbo.TcpResult)      AS TcpResults,
    (SELECT COUNT(*) FROM dbo.TlsResult)      AS TlsResults,
    (SELECT COUNT(*) FROM dbo.HttpResult)     AS HttpResults,
    (SELECT COUNT(*) FROM dbo.UdpResult)      AS UdpResults,
    (SELECT COUNT(*) FROM dbo.ErrorRecord)    AS Errors;
GO

-- ── 11. Throughput summary by mode and payload size ───────────────────────────
SELECT
    a.Protocol,
    h.PayloadBytes,
    COUNT(*)                AS Attempts,
    AVG(h.ThroughputMbps)  AS AvgMbps,
    MIN(h.ThroughputMbps)  AS MinMbps,
    MAX(h.ThroughputMbps)  AS MaxMbps
FROM dbo.RequestAttempt a
JOIN dbo.HttpResult h ON h.AttemptId = a.AttemptId
WHERE a.Protocol IN ('download', 'upload')
  AND h.ThroughputMbps IS NOT NULL
GROUP BY a.Protocol, h.PayloadBytes
ORDER BY a.Protocol, h.PayloadBytes;
GO
