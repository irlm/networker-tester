-- Tester V001-V004 schema: Tables the dashboard migrations depend on.
-- This is idempotent (CREATE TABLE IF NOT EXISTS / CREATE INDEX IF NOT EXISTS).
-- Source: crates/networker-tester/src/output/db/postgres.rs

-- V001: Core tables
CREATE TABLE IF NOT EXISTS TestRun (
    RunId UUID NOT NULL, StartedAt TIMESTAMPTZ NOT NULL, FinishedAt TIMESTAMPTZ NULL,
    TargetUrl VARCHAR(2048) NOT NULL, TargetHost VARCHAR(255) NOT NULL, Modes VARCHAR(200) NOT NULL,
    TotalRuns INT NOT NULL DEFAULT 1, Concurrency INT NOT NULL DEFAULT 1,
    TimeoutMs BIGINT NOT NULL DEFAULT 30000, ClientOs VARCHAR(50) NOT NULL,
    ClientVersion VARCHAR(50) NOT NULL, SuccessCount INT NOT NULL DEFAULT 0,
    FailureCount INT NOT NULL DEFAULT 0, CONSTRAINT PK_TestRun PRIMARY KEY (RunId)
);
CREATE TABLE IF NOT EXISTS RequestAttempt (
    AttemptId UUID NOT NULL, RunId UUID NOT NULL, Protocol VARCHAR(20) NOT NULL,
    SequenceNum INT NOT NULL, StartedAt TIMESTAMPTZ NOT NULL, FinishedAt TIMESTAMPTZ NULL,
    Success BOOLEAN NOT NULL DEFAULT FALSE, ErrorMessage TEXT NULL, RetryCount INT NOT NULL DEFAULT 0,
    CONSTRAINT PK_RequestAttempt PRIMARY KEY (AttemptId),
    CONSTRAINT FK_Attempt_Run FOREIGN KEY (RunId) REFERENCES TestRun (RunId) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS DnsResult (
    DnsId UUID NOT NULL, AttemptId UUID NOT NULL, QueryName VARCHAR(255) NOT NULL,
    ResolvedIPs VARCHAR(1024) NOT NULL, DurationMs DOUBLE PRECISION NOT NULL,
    StartedAt TIMESTAMPTZ NOT NULL, Success BOOLEAN NOT NULL,
    CONSTRAINT PK_DnsResult PRIMARY KEY (DnsId),
    CONSTRAINT FK_Dns_Attempt FOREIGN KEY (AttemptId) REFERENCES RequestAttempt (AttemptId) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS TcpResult (
    TcpId UUID NOT NULL, AttemptId UUID NOT NULL, LocalAddr VARCHAR(50) NULL,
    RemoteAddr VARCHAR(50) NOT NULL, ConnectDurationMs DOUBLE PRECISION NOT NULL,
    AttemptCount INT NOT NULL DEFAULT 1, StartedAt TIMESTAMPTZ NOT NULL, Success BOOLEAN NOT NULL,
    MssBytesEstimate INT NULL, RttEstimateMs DOUBLE PRECISION NULL, Retransmits BIGINT NULL,
    TotalRetrans BIGINT NULL, SndCwnd BIGINT NULL, SndSsthresh BIGINT NULL,
    RttVarianceMs DOUBLE PRECISION NULL, RcvSpace BIGINT NULL, SegsOut BIGINT NULL,
    SegsIn BIGINT NULL, CongestionAlgorithm VARCHAR(32) NULL, DeliveryRateBps BIGINT NULL,
    MinRttMs DOUBLE PRECISION NULL,
    CONSTRAINT PK_TcpResult PRIMARY KEY (TcpId),
    CONSTRAINT FK_Tcp_Attempt FOREIGN KEY (AttemptId) REFERENCES RequestAttempt (AttemptId) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS TlsResult (
    TlsId UUID NOT NULL, AttemptId UUID NOT NULL, ProtocolVersion VARCHAR(20) NOT NULL,
    CipherSuite VARCHAR(100) NOT NULL, AlpnNegotiated VARCHAR(50) NULL,
    CertSubject VARCHAR(500) NULL, CertIssuer VARCHAR(500) NULL, CertExpiry TIMESTAMPTZ NULL,
    HandshakeDurationMs DOUBLE PRECISION NOT NULL, StartedAt TIMESTAMPTZ NOT NULL, Success BOOLEAN NOT NULL,
    CONSTRAINT PK_TlsResult PRIMARY KEY (TlsId),
    CONSTRAINT FK_Tls_Attempt FOREIGN KEY (AttemptId) REFERENCES RequestAttempt (AttemptId) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS HttpResult (
    HttpId UUID NOT NULL, AttemptId UUID NOT NULL, NegotiatedVersion VARCHAR(20) NOT NULL,
    StatusCode INT NOT NULL, HeadersSizeBytes INT NOT NULL DEFAULT 0, BodySizeBytes INT NOT NULL DEFAULT 0,
    TtfbMs DOUBLE PRECISION NOT NULL, TotalDurationMs DOUBLE PRECISION NOT NULL,
    RedirectCount INT NOT NULL DEFAULT 0, StartedAt TIMESTAMPTZ NOT NULL,
    PayloadBytes BIGINT NULL, ThroughputMbps DOUBLE PRECISION NULL,
    CONSTRAINT PK_HttpResult PRIMARY KEY (HttpId),
    CONSTRAINT FK_Http_Attempt FOREIGN KEY (AttemptId) REFERENCES RequestAttempt (AttemptId) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS UdpResult (
    UdpId UUID NOT NULL, AttemptId UUID NOT NULL, RemoteAddr VARCHAR(50) NOT NULL,
    ProbeCount INT NOT NULL, SuccessCount INT NOT NULL, LossPercent DOUBLE PRECISION NOT NULL,
    RttMinMs DOUBLE PRECISION NOT NULL, RttAvgMs DOUBLE PRECISION NOT NULL,
    RttP95Ms DOUBLE PRECISION NOT NULL, JitterMs DOUBLE PRECISION NOT NULL,
    StartedAt TIMESTAMPTZ NOT NULL,
    CONSTRAINT PK_UdpResult PRIMARY KEY (UdpId),
    CONSTRAINT FK_Udp_Attempt FOREIGN KEY (AttemptId) REFERENCES RequestAttempt (AttemptId) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS ErrorRecord (
    ErrorId UUID NOT NULL, AttemptId UUID NULL, RunId UUID NOT NULL,
    ErrorCategory VARCHAR(50) NOT NULL, ErrorMessage TEXT NOT NULL, ErrorDetail TEXT NULL,
    OccurredAt TIMESTAMPTZ NOT NULL,
    CONSTRAINT PK_ErrorRecord PRIMARY KEY (ErrorId),
    CONSTRAINT FK_Error_Attempt FOREIGN KEY (AttemptId) REFERENCES RequestAttempt (AttemptId) ON DELETE NO ACTION,
    CONSTRAINT FK_Error_Run FOREIGN KEY (RunId) REFERENCES TestRun (RunId) ON DELETE NO ACTION
);
CREATE TABLE IF NOT EXISTS ServerTimingResult (
    ServerId UUID NOT NULL, AttemptId UUID NOT NULL, RequestId VARCHAR(128) NULL,
    ServerTimestamp TIMESTAMPTZ NULL, ClockSkewMs DOUBLE PRECISION NULL,
    RecvBodyMs DOUBLE PRECISION NULL, ProcessingMs DOUBLE PRECISION NULL,
    TotalServerMs DOUBLE PRECISION NULL,
    CONSTRAINT PK_ServerTimingResult PRIMARY KEY (ServerId),
    CONSTRAINT FK_ServerTimingResult_Attempt FOREIGN KEY (AttemptId) REFERENCES RequestAttempt (AttemptId)
);
CREATE INDEX IF NOT EXISTS IX_TestRun_StartedAt ON TestRun (StartedAt DESC);
CREATE INDEX IF NOT EXISTS IX_TestRun_TargetHost ON TestRun (TargetHost);
CREATE INDEX IF NOT EXISTS IX_Attempt_Protocol ON RequestAttempt (Protocol, Success);
CREATE INDEX IF NOT EXISTS IX_Attempt_RunId ON RequestAttempt (RunId, SequenceNum);
CREATE INDEX IF NOT EXISTS IX_HttpResult_Version ON HttpResult (NegotiatedVersion, StatusCode);
CREATE INDEX IF NOT EXISTS IX_HttpResult_Throughput ON HttpResult (ThroughputMbps) WHERE ThroughputMbps IS NOT NULL;
CREATE INDEX IF NOT EXISTS IX_Error_Category ON ErrorRecord (ErrorCategory, OccurredAt DESC);
CREATE INDEX IF NOT EXISTS IX_ServerTimingResult_AttemptId ON ServerTimingResult (AttemptId);

-- V002: URL test tables
CREATE TABLE IF NOT EXISTS UrlTestRun (
    Id UUID NOT NULL, StartedAt TIMESTAMPTZ NOT NULL, CompletedAt TIMESTAMPTZ NULL,
    RequestedUrl VARCHAR(2048) NOT NULL, FinalUrl VARCHAR(2048) NULL, Status VARCHAR(32) NOT NULL,
    PageLoadStrategy VARCHAR(32) NOT NULL, BrowserEngine VARCHAR(64) NULL,
    BrowserVersion VARCHAR(64) NULL, UserAgent TEXT NULL, PrimaryOrigin VARCHAR(1024) NULL,
    ObservedProtocolPrimaryLoad VARCHAR(32) NULL, AdvertisedAltSvc TEXT NULL,
    ValidatedHttpVersions VARCHAR(128) NOT NULL DEFAULT '', TlsVersion VARCHAR(32) NULL,
    CipherSuite VARCHAR(128) NULL, Alpn VARCHAR(32) NULL, DnsMs DOUBLE PRECISION NULL,
    ConnectMs DOUBLE PRECISION NULL, HandshakeMs DOUBLE PRECISION NULL,
    TtfbMs DOUBLE PRECISION NULL, DomContentLoadedMs DOUBLE PRECISION NULL,
    LoadEventMs DOUBLE PRECISION NULL, NetworkIdleMs DOUBLE PRECISION NULL,
    CaptureEndMs DOUBLE PRECISION NULL, TotalRequests INT NOT NULL DEFAULT 0,
    TotalTransferBytes BIGINT NOT NULL DEFAULT 0, PeakConcurrentConnections INT NULL,
    RedirectCount INT NOT NULL DEFAULT 0, FailureCount INT NOT NULL DEFAULT 0,
    HarPath TEXT NULL, PcapPath TEXT NULL, PcapSummaryJson TEXT NULL,
    CaptureErrors TEXT NULL, EnvironmentNotes TEXT NULL,
    CONSTRAINT PK_UrlTestRun PRIMARY KEY (Id)
);
CREATE TABLE IF NOT EXISTS UrlTestResource (
    Id UUID NOT NULL, UrlTestRunId UUID NOT NULL, ResourceUrl VARCHAR(2048) NOT NULL,
    Origin VARCHAR(1024) NOT NULL, ResourceType VARCHAR(64) NOT NULL, MimeType VARCHAR(255) NULL,
    StatusCode INT NULL, Protocol VARCHAR(32) NULL, TransferSize BIGINT NULL,
    EncodedBodySize BIGINT NULL, DecodedBodySize BIGINT NULL, DurationMs DOUBLE PRECISION NULL,
    ConnectionId VARCHAR(128) NULL, ReusedConnection BOOLEAN NULL, InitiatorType VARCHAR(64) NULL,
    FromCache BOOLEAN NULL, Redirected BOOLEAN NULL, Failed BOOLEAN NOT NULL DEFAULT FALSE,
    CONSTRAINT PK_UrlTestResource PRIMARY KEY (Id),
    CONSTRAINT FK_UrlTestResource_Run FOREIGN KEY (UrlTestRunId) REFERENCES UrlTestRun (Id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS UrlTestProtocolRun (
    Id UUID NOT NULL, UrlTestRunId UUID NOT NULL, ProtocolMode VARCHAR(16) NOT NULL,
    RunNumber INT NOT NULL, AttemptType VARCHAR(16) NOT NULL, ObservedProtocol VARCHAR(32) NULL,
    FallbackOccurred BOOLEAN NULL, Succeeded BOOLEAN NOT NULL DEFAULT FALSE, StatusCode INT NULL,
    TtfbMs DOUBLE PRECISION NULL, TotalMs DOUBLE PRECISION NULL, FailureReason TEXT NULL, Error TEXT NULL,
    CONSTRAINT PK_UrlTestProtocolRun PRIMARY KEY (Id),
    CONSTRAINT FK_UrlTestProtocolRun_Run FOREIGN KEY (UrlTestRunId) REFERENCES UrlTestRun (Id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS IX_UrlTestRun_StartedAt ON UrlTestRun (StartedAt DESC);
CREATE INDEX IF NOT EXISTS IX_UrlTestRun_Status ON UrlTestRun (Status, StartedAt DESC);
CREATE INDEX IF NOT EXISTS IX_UrlTestResource_RunId ON UrlTestResource (UrlTestRunId);
CREATE INDEX IF NOT EXISTS IX_UrlTestProtocolRun_RunId ON UrlTestProtocolRun (UrlTestRunId, ProtocolMode, RunNumber);

-- V003: Benchmark tables
CREATE TABLE IF NOT EXISTS BenchmarkRun (
    BenchmarkRunId UUID NOT NULL, ContractVersion VARCHAR(20) NOT NULL,
    GeneratedAt TIMESTAMPTZ NOT NULL, Source VARCHAR(64) NOT NULL,
    TargetUrl VARCHAR(2048) NOT NULL, TargetHost VARCHAR(255) NOT NULL,
    Modes VARCHAR(200) NOT NULL, TotalRuns INT NOT NULL, Concurrency INT NOT NULL,
    TimeoutMs BIGINT NOT NULL, ClientOs VARCHAR(50) NOT NULL, ClientVersion VARCHAR(50) NOT NULL,
    MethodologyJson JSONB NOT NULL, DiagnosticsJson JSONB NOT NULL, AggregateSummaryJson JSONB NOT NULL,
    CONSTRAINT PK_BenchmarkRun PRIMARY KEY (BenchmarkRunId),
    CONSTRAINT FK_BenchmarkRun_TestRun FOREIGN KEY (BenchmarkRunId) REFERENCES TestRun (RunId) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS BenchmarkEnvironment (
    BenchmarkRunId UUID NOT NULL, ClientInfoJson JSONB NULL, ServerInfoJson JSONB NULL,
    NetworkBaselineJson JSONB NULL, PacketCaptureEnabled BOOLEAN NOT NULL DEFAULT FALSE,
    EnvironmentJson JSONB NOT NULL,
    CONSTRAINT PK_BenchmarkEnvironment PRIMARY KEY (BenchmarkRunId),
    CONSTRAINT FK_BenchmarkEnvironment_Run FOREIGN KEY (BenchmarkRunId) REFERENCES BenchmarkRun (BenchmarkRunId) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS BenchmarkDataQuality (
    BenchmarkRunId UUID NOT NULL, NoiseLevel VARCHAR(16) NOT NULL,
    SampleStabilityCv DOUBLE PRECISION NOT NULL, Sufficiency VARCHAR(16) NOT NULL,
    PublicationReady BOOLEAN NOT NULL DEFAULT FALSE, WarningsJson JSONB NOT NULL, QualityJson JSONB NOT NULL,
    CONSTRAINT PK_BenchmarkDataQuality PRIMARY KEY (BenchmarkRunId),
    CONSTRAINT FK_BenchmarkDataQuality_Run FOREIGN KEY (BenchmarkRunId) REFERENCES BenchmarkRun (BenchmarkRunId) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS BenchmarkCase (
    BenchmarkRunId UUID NOT NULL, CaseId VARCHAR(255) NOT NULL, Protocol VARCHAR(32) NOT NULL,
    PayloadBytes BIGINT NULL, HttpStack VARCHAR(128) NULL, MetricName VARCHAR(64) NOT NULL,
    MetricUnit VARCHAR(32) NOT NULL, HigherIsBetter BOOLEAN NOT NULL, CaseJson JSONB NOT NULL,
    CONSTRAINT PK_BenchmarkCase PRIMARY KEY (BenchmarkRunId, CaseId),
    CONSTRAINT FK_BenchmarkCase_Run FOREIGN KEY (BenchmarkRunId) REFERENCES BenchmarkRun (BenchmarkRunId) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS BenchmarkSample (
    AttemptId UUID NOT NULL, BenchmarkRunId UUID NOT NULL, CaseId VARCHAR(255) NOT NULL,
    LaunchIndex INT NOT NULL DEFAULT 0, Phase VARCHAR(32) NOT NULL, IterationIndex INT NOT NULL,
    Success BOOLEAN NOT NULL DEFAULT FALSE, RetryCount INT NOT NULL DEFAULT 0,
    InclusionStatus VARCHAR(64) NOT NULL, MetricValue DOUBLE PRECISION NULL,
    MetricUnit VARCHAR(32) NOT NULL, StartedAt TIMESTAMPTZ NOT NULL, FinishedAt TIMESTAMPTZ NULL,
    TotalDurationMs DOUBLE PRECISION NULL, TtfbMs DOUBLE PRECISION NULL, SampleJson JSONB NOT NULL,
    CONSTRAINT PK_BenchmarkSample PRIMARY KEY (AttemptId),
    CONSTRAINT FK_BenchmarkSample_Run FOREIGN KEY (BenchmarkRunId) REFERENCES BenchmarkRun (BenchmarkRunId) ON DELETE CASCADE,
    CONSTRAINT FK_BenchmarkSample_Case FOREIGN KEY (BenchmarkRunId, CaseId) REFERENCES BenchmarkCase (BenchmarkRunId, CaseId) ON DELETE CASCADE,
    CONSTRAINT FK_BenchmarkSample_Attempt FOREIGN KEY (AttemptId) REFERENCES RequestAttempt (AttemptId) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS BenchmarkSummary (
    BenchmarkRunId UUID NOT NULL, CaseId VARCHAR(255) NOT NULL, Protocol VARCHAR(32) NOT NULL,
    PayloadBytes BIGINT NULL, HttpStack VARCHAR(128) NULL, MetricName VARCHAR(64) NOT NULL,
    MetricUnit VARCHAR(32) NOT NULL, HigherIsBetter BOOLEAN NOT NULL, SampleCount BIGINT NOT NULL,
    IncludedSampleCount BIGINT NOT NULL, ExcludedSampleCount BIGINT NOT NULL,
    SuccessCount BIGINT NOT NULL, FailureCount BIGINT NOT NULL, TotalRequests BIGINT NOT NULL,
    ErrorCount BIGINT NOT NULL, BytesTransferred BIGINT NOT NULL, WallTimeMs DOUBLE PRECISION NOT NULL,
    Rps DOUBLE PRECISION NOT NULL, Min DOUBLE PRECISION NOT NULL, Mean DOUBLE PRECISION NOT NULL,
    P5 DOUBLE PRECISION NOT NULL, P25 DOUBLE PRECISION NOT NULL, P50 DOUBLE PRECISION NOT NULL,
    P75 DOUBLE PRECISION NOT NULL, P95 DOUBLE PRECISION NOT NULL, P99 DOUBLE PRECISION NOT NULL,
    P999 DOUBLE PRECISION NOT NULL, Max DOUBLE PRECISION NOT NULL, Stddev DOUBLE PRECISION NOT NULL,
    LatencyMeanMs DOUBLE PRECISION NULL, LatencyP50Ms DOUBLE PRECISION NULL,
    LatencyP99Ms DOUBLE PRECISION NULL, LatencyP999Ms DOUBLE PRECISION NULL,
    LatencyMaxMs DOUBLE PRECISION NULL, SummaryJson JSONB NOT NULL,
    CONSTRAINT PK_BenchmarkSummary PRIMARY KEY (BenchmarkRunId, CaseId),
    CONSTRAINT FK_BenchmarkSummary_Run FOREIGN KEY (BenchmarkRunId) REFERENCES BenchmarkRun (BenchmarkRunId) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS IX_BenchmarkRun_GeneratedAt ON BenchmarkRun (GeneratedAt DESC);
CREATE INDEX IF NOT EXISTS IX_BenchmarkCase_Protocol ON BenchmarkCase (Protocol, BenchmarkRunId);
CREATE INDEX IF NOT EXISTS IX_BenchmarkSample_RunCase ON BenchmarkSample (BenchmarkRunId, CaseId, Phase, Success);
CREATE INDEX IF NOT EXISTS IX_BenchmarkSummary_RunProtocol ON BenchmarkSummary (BenchmarkRunId, Protocol);
CREATE INDEX IF NOT EXISTS IX_BenchmarkDataQuality_PublicationReady ON BenchmarkDataQuality (PublicationReady, NoiseLevel);

-- V004: Benchmark launch table
CREATE TABLE IF NOT EXISTS BenchmarkLaunch (
    BenchmarkRunId UUID NOT NULL, LaunchIndex INT NOT NULL, Scenario VARCHAR(64) NOT NULL,
    PrimaryPhase VARCHAR(32) NOT NULL, StartedAt TIMESTAMPTZ NOT NULL, FinishedAt TIMESTAMPTZ NULL,
    SampleCount BIGINT NOT NULL, PrimarySampleCount BIGINT NOT NULL, WarmupSampleCount BIGINT NOT NULL,
    SuccessCount BIGINT NOT NULL, FailureCount BIGINT NOT NULL, PhasesJson JSONB NOT NULL,
    CONSTRAINT PK_BenchmarkLaunch PRIMARY KEY (BenchmarkRunId, LaunchIndex),
    CONSTRAINT FK_BenchmarkLaunch_Run FOREIGN KEY (BenchmarkRunId) REFERENCES BenchmarkRun (BenchmarkRunId) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS IX_BenchmarkLaunch_Phase ON BenchmarkLaunch (PrimaryPhase, Scenario, BenchmarkRunId);
