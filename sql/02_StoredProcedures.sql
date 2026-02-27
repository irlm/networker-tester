-- =============================================================================
-- Networker Tester – Stored Procedures (optional)
--
-- These are provided as an alternative to the parameterized inserts in
-- output/sql.rs.  Use them if your security policy requires all writes to
-- go through SPs rather than direct table inserts.
--
-- Usage in Rust (tiberius):
--   let mut q = Query::new("EXEC dbo.usp_InsertTestRun @RunId=@P1, ...");
-- =============================================================================

USE NetworkDiagnostics;
GO

-- ── usp_InsertTestRun ──────────────────────────────────────────────────────

CREATE OR ALTER PROCEDURE dbo.usp_InsertTestRun
    @RunId         NVARCHAR(36),
    @StartedAt     DATETIME2(3),
    @FinishedAt    DATETIME2(3)   = NULL,
    @TargetUrl     NVARCHAR(2048),
    @TargetHost    NVARCHAR(255),
    @Modes         NVARCHAR(200),
    @TotalRuns     INT            = 1,
    @Concurrency   INT            = 1,
    @TimeoutMs     BIGINT         = 30000,
    @ClientOs      NVARCHAR(50),
    @ClientVersion NVARCHAR(50),
    @SuccessCount  INT            = 0,
    @FailureCount  INT            = 0
AS
BEGIN
    SET NOCOUNT ON;

    IF EXISTS (SELECT 1 FROM dbo.TestRun WHERE RunId = @RunId)
        -- Idempotent: update FinishedAt and counts if run already exists
        UPDATE dbo.TestRun
        SET FinishedAt   = ISNULL(@FinishedAt, FinishedAt),
            SuccessCount = @SuccessCount,
            FailureCount = @FailureCount
        WHERE RunId = @RunId;
    ELSE
        INSERT INTO dbo.TestRun (
            RunId, StartedAt, FinishedAt, TargetUrl, TargetHost,
            Modes, TotalRuns, Concurrency, TimeoutMs,
            ClientOs, ClientVersion, SuccessCount, FailureCount
        ) VALUES (
            @RunId, @StartedAt, @FinishedAt, @TargetUrl, @TargetHost,
            @Modes, @TotalRuns, @Concurrency, @TimeoutMs,
            @ClientOs, @ClientVersion, @SuccessCount, @FailureCount
        );
END;
GO

-- ── usp_InsertRequestAttempt ───────────────────────────────────────────────

CREATE OR ALTER PROCEDURE dbo.usp_InsertRequestAttempt
    @AttemptId    NVARCHAR(36),
    @RunId        NVARCHAR(36),
    @Protocol     NVARCHAR(20),
    @SequenceNum  INT,
    @StartedAt    DATETIME2(3),
    @FinishedAt   DATETIME2(3)  = NULL,
    @Success      BIT           = 0,
    @ErrorMessage NVARCHAR(MAX) = NULL
AS
BEGIN
    SET NOCOUNT ON;
    INSERT INTO dbo.RequestAttempt (
        AttemptId, RunId, Protocol, SequenceNum,
        StartedAt, FinishedAt, Success, ErrorMessage
    ) VALUES (
        @AttemptId, @RunId, @Protocol, @SequenceNum,
        @StartedAt, @FinishedAt, @Success, @ErrorMessage
    );
END;
GO

-- ── usp_InsertDnsResult ────────────────────────────────────────────────────

CREATE OR ALTER PROCEDURE dbo.usp_InsertDnsResult
    @DnsId       NVARCHAR(36),
    @AttemptId   NVARCHAR(36),
    @QueryName   NVARCHAR(255),
    @ResolvedIPs NVARCHAR(1024),
    @DurationMs  FLOAT,
    @StartedAt   DATETIME2(3),
    @Success     BIT
AS
BEGIN
    SET NOCOUNT ON;
    INSERT INTO dbo.DnsResult (DnsId, AttemptId, QueryName, ResolvedIPs, DurationMs, StartedAt, Success)
    VALUES (@DnsId, @AttemptId, @QueryName, @ResolvedIPs, @DurationMs, @StartedAt, @Success);
END;
GO

-- ── usp_InsertTcpResult ────────────────────────────────────────────────────

CREATE OR ALTER PROCEDURE dbo.usp_InsertTcpResult
    @TcpId             NVARCHAR(36),
    @AttemptId         NVARCHAR(36),
    @LocalAddr         NVARCHAR(50)  = NULL,
    @RemoteAddr        NVARCHAR(50),
    @ConnectDurationMs FLOAT,
    @AttemptCount      INT           = 1,
    @StartedAt         DATETIME2(3),
    @Success           BIT,
    @MssBytesEstimate  INT           = NULL,
    @RttEstimateMs     FLOAT         = NULL
AS
BEGIN
    SET NOCOUNT ON;
    INSERT INTO dbo.TcpResult (
        TcpId, AttemptId, LocalAddr, RemoteAddr,
        ConnectDurationMs, AttemptCount, StartedAt, Success,
        MssBytesEstimate, RttEstimateMs
    ) VALUES (
        @TcpId, @AttemptId, @LocalAddr, @RemoteAddr,
        @ConnectDurationMs, @AttemptCount, @StartedAt, @Success,
        @MssBytesEstimate, @RttEstimateMs
    );
END;
GO

-- ── usp_InsertTlsResult ────────────────────────────────────────────────────

CREATE OR ALTER PROCEDURE dbo.usp_InsertTlsResult
    @TlsId               NVARCHAR(36),
    @AttemptId           NVARCHAR(36),
    @ProtocolVersion     NVARCHAR(20),
    @CipherSuite         NVARCHAR(100),
    @AlpnNegotiated      NVARCHAR(50)   = NULL,
    @CertSubject         NVARCHAR(500)  = NULL,
    @CertIssuer          NVARCHAR(500)  = NULL,
    @CertExpiry          DATETIME2(3)   = NULL,
    @HandshakeDurationMs FLOAT,
    @StartedAt           DATETIME2(3),
    @Success             BIT
AS
BEGIN
    SET NOCOUNT ON;
    INSERT INTO dbo.TlsResult (
        TlsId, AttemptId, ProtocolVersion, CipherSuite,
        AlpnNegotiated, CertSubject, CertIssuer, CertExpiry,
        HandshakeDurationMs, StartedAt, Success
    ) VALUES (
        @TlsId, @AttemptId, @ProtocolVersion, @CipherSuite,
        @AlpnNegotiated, @CertSubject, @CertIssuer, @CertExpiry,
        @HandshakeDurationMs, @StartedAt, @Success
    );
END;
GO

-- ── usp_InsertHttpResult ───────────────────────────────────────────────────

CREATE OR ALTER PROCEDURE dbo.usp_InsertHttpResult
    @HttpId            NVARCHAR(36),
    @AttemptId         NVARCHAR(36),
    @NegotiatedVersion NVARCHAR(20),
    @StatusCode        INT,
    @HeadersSizeBytes  INT,
    @BodySizeBytes     INT,
    @TtfbMs            FLOAT,
    @TotalDurationMs   FLOAT,
    @RedirectCount     INT  = 0,
    @StartedAt         DATETIME2(3)
AS
BEGIN
    SET NOCOUNT ON;
    INSERT INTO dbo.HttpResult (
        HttpId, AttemptId, NegotiatedVersion, StatusCode,
        HeadersSizeBytes, BodySizeBytes, TtfbMs,
        TotalDurationMs, RedirectCount, StartedAt
    ) VALUES (
        @HttpId, @AttemptId, @NegotiatedVersion, @StatusCode,
        @HeadersSizeBytes, @BodySizeBytes, @TtfbMs,
        @TotalDurationMs, @RedirectCount, @StartedAt
    );
END;
GO

-- ── usp_InsertUdpResult ────────────────────────────────────────────────────

CREATE OR ALTER PROCEDURE dbo.usp_InsertUdpResult
    @UdpId        NVARCHAR(36),
    @AttemptId    NVARCHAR(36),
    @RemoteAddr   NVARCHAR(50),
    @ProbeCount   INT,
    @SuccessCount INT,
    @LossPercent  FLOAT,
    @RttMinMs     FLOAT,
    @RttAvgMs     FLOAT,
    @RttP95Ms     FLOAT,
    @JitterMs     FLOAT,
    @StartedAt    DATETIME2(3)
AS
BEGIN
    SET NOCOUNT ON;
    INSERT INTO dbo.UdpResult (
        UdpId, AttemptId, RemoteAddr, ProbeCount,
        SuccessCount, LossPercent, RttMinMs, RttAvgMs,
        RttP95Ms, JitterMs, StartedAt
    ) VALUES (
        @UdpId, @AttemptId, @RemoteAddr, @ProbeCount,
        @SuccessCount, @LossPercent, @RttMinMs, @RttAvgMs,
        @RttP95Ms, @JitterMs, @StartedAt
    );
END;
GO

-- ── usp_InsertError ────────────────────────────────────────────────────────

CREATE OR ALTER PROCEDURE dbo.usp_InsertError
    @ErrorId       NVARCHAR(36),
    @AttemptId     NVARCHAR(36) = NULL,
    @RunId         NVARCHAR(36),
    @ErrorCategory NVARCHAR(50),
    @ErrorMessage  NVARCHAR(MAX),
    @ErrorDetail   NVARCHAR(MAX) = NULL,
    @OccurredAt    DATETIME2(3)
AS
BEGIN
    SET NOCOUNT ON;
    INSERT INTO dbo.ErrorRecord (
        ErrorId, AttemptId, RunId, ErrorCategory, ErrorMessage, ErrorDetail, OccurredAt
    ) VALUES (
        @ErrorId, @AttemptId, @RunId, @ErrorCategory, @ErrorMessage, @ErrorDetail, @OccurredAt
    );
END;
GO

PRINT 'Stored procedures created / updated.';
GO
