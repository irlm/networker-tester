using System.Text.Json.Serialization;

namespace Networker.Contracts;

// C# side of the frozen networker-tester JSON contract.
//
// These records mirror the Rust `TestRun` / `RequestAttempt` / phase-result
// structs emitted by `networker-tester --json-stdout` (source of truth:
// crates/networker-tester/src/metrics.rs). The Rust output still carries more
// fields than are modelled here (cert chains, benchmark metadata, browser
// results, ...). Unknown JSON members are ignored on deserialization, so the
// contract can grow additively without breaking this side — a `schema_version`
// bump signals when consumers must be revised. Conversely, every field below
// that is additive on the Rust side (`#[serde(default)]`) is nullable here, so
// payloads from older testers still deserialize.
//
// NOTE (measurement-gap-analysis-2026-07 finding #1): the LIVE attempt path
// does not round-trip through these records at all — the agent streams each
// attempt's raw JSON verbatim (`RunExecutor` → `attempt_event` →
// `AgentMessageProcessor.OnAttemptEvent` → browser bus `attempt_result`), so
// the dashboard receives the full tester payload regardless of what is
// modelled here. These records are the TYPED seam for C# consumers, and the
// field names below are pinned against real tester output by
// ContractRoundTripTests — a rename on either side fails those tests.
//
// Serialization uses System.Text.Json source generation (see
// ProbeContractJsonContext) for trim-safe, reflection-free (de)serialization.

/// <summary>Top-level result of one tester run against one target.</summary>
public sealed record ProbeRunResult
{
    /// <summary>Version of the tester JSON contract, e.g. "1.0".</summary>
    [JsonPropertyName("schema_version")]
    public string SchemaVersion { get; init; } = "unknown";

    [JsonPropertyName("run_id")]
    public string RunId { get; init; } = string.Empty;

    [JsonPropertyName("target_url")]
    public string TargetUrl { get; init; } = string.Empty;

    [JsonPropertyName("target_host")]
    public string TargetHost { get; init; } = string.Empty;

    [JsonPropertyName("modes")]
    public IReadOnlyList<string> Modes { get; init; } = Array.Empty<string>();

    [JsonPropertyName("client_version")]
    public string ClientVersion { get; init; } = string.Empty;

    [JsonPropertyName("attempts")]
    public IReadOnlyList<ProbeAttempt> Attempts { get; init; } = Array.Empty<ProbeAttempt>();
}

/// <summary>One probe attempt within a run (mirrors Rust `RequestAttempt`).</summary>
public sealed record ProbeAttempt
{
    [JsonPropertyName("attempt_id")]
    public string AttemptId { get; init; } = string.Empty;

    [JsonPropertyName("protocol")]
    public string Protocol { get; init; } = string.Empty;

    [JsonPropertyName("sequence_num")]
    public uint SequenceNum { get; init; }

    [JsonPropertyName("success")]
    public bool Success { get; init; }

    [JsonPropertyName("dns")]
    public DnsPhase? Dns { get; init; }

    [JsonPropertyName("tcp")]
    public TcpPhase? Tcp { get; init; }

    [JsonPropertyName("tls")]
    public TlsPhase? Tls { get; init; }

    [JsonPropertyName("http")]
    public HttpPhase? Http { get; init; }

    [JsonPropertyName("udp")]
    public UdpPhase? Udp { get; init; }

    /// <summary>Server-side timing parsed from response headers (network-vs-server split).</summary>
    [JsonPropertyName("server_timing")]
    public ServerTimingPhase? ServerTiming { get; init; }
}

/// <summary>DNS resolution phase timing (mirrors Rust `DnsResult`).</summary>
public sealed record DnsPhase
{
    [JsonPropertyName("duration_ms")]
    public double DurationMs { get; init; }

    [JsonPropertyName("success")]
    public bool Success { get; init; }
}

/// <summary>
/// TCP connect phase timing + kernel socket stats (mirrors Rust `TcpResult`).
/// The kernel stats come from TCP_INFO (Linux) / TCP_CONNECTION_INFO (macOS)
/// and are all best-effort — null on Windows, older kernels, or older testers.
/// </summary>
public sealed record TcpPhase
{
    [JsonPropertyName("connect_duration_ms")]
    public double ConnectDurationMs { get; init; }

    [JsonPropertyName("success")]
    public bool Success { get; init; }

    /// <summary>MSS as reported by TCP_MAXSEG (best-effort, Unix only).</summary>
    [JsonPropertyName("mss_bytes")]
    public uint? MssBytes { get; init; }

    /// <summary>Smoothed RTT in ms from the kernel.</summary>
    [JsonPropertyName("rtt_estimate_ms")]
    public double? RttEstimateMs { get; init; }

    /// <summary>Segments currently queued for retransmit (tcpi_retransmits).</summary>
    [JsonPropertyName("retransmits")]
    public uint? Retransmits { get; init; }

    /// <summary>Lifetime retransmission count (tcpi_total_retrans).</summary>
    [JsonPropertyName("total_retrans")]
    public uint? TotalRetrans { get; init; }

    /// <summary>Congestion window in segments (tcpi_snd_cwnd).</summary>
    [JsonPropertyName("snd_cwnd")]
    public uint? SndCwnd { get; init; }

    /// <summary>Congestion control algorithm name, e.g. "cubic", "bbr".</summary>
    [JsonPropertyName("congestion_algorithm")]
    public string? CongestionAlgorithm { get; init; }

    /// <summary>Estimated TCP delivery rate in bytes/sec (Linux ≥ 4.9).</summary>
    [JsonPropertyName("delivery_rate_bps")]
    public ulong? DeliveryRateBps { get; init; }

    /// <summary>Minimum RTT ever observed by the kernel in ms (Linux ≥ 4.9).</summary>
    [JsonPropertyName("min_rtt_ms")]
    public double? MinRttMs { get; init; }
}

/// <summary>TLS handshake phase timing + negotiation depth (mirrors Rust `TlsResult`).</summary>
public sealed record TlsPhase
{
    [JsonPropertyName("handshake_duration_ms")]
    public double HandshakeDurationMs { get; init; }

    [JsonPropertyName("protocol_version")]
    public string? ProtocolVersion { get; init; }

    [JsonPropertyName("success")]
    public bool Success { get; init; }

    /// <summary>Negotiated cipher suite, e.g. "TLS13_AES_256_GCM_SHA384".</summary>
    [JsonPropertyName("cipher_suite")]
    public string? CipherSuite { get; init; }

    /// <summary>ALPN protocol negotiated, e.g. "h2", "http/1.1".</summary>
    [JsonPropertyName("alpn_negotiated")]
    public string? AlpnNegotiated { get; init; }

    /// <summary>Leaf certificate expiry.</summary>
    [JsonPropertyName("cert_expiry")]
    public DateTimeOffset? CertExpiry { get; init; }

    /// <summary>True when the handshake reused prior session state.</summary>
    [JsonPropertyName("resumed")]
    public bool? Resumed { get; init; }

    /// <summary>rustls handshake classification: "full", "full-hrr", or "resumed".</summary>
    [JsonPropertyName("handshake_kind")]
    public string? HandshakeKind { get; init; }

    /// <summary>TLS backend that performed the handshake, e.g. "rustls", "native/openssl".</summary>
    [JsonPropertyName("tls_backend")]
    public string? TlsBackend { get; init; }
}

/// <summary>HTTP request phase timing + transfer/CPU detail (mirrors Rust `HttpResult`).</summary>
public sealed record HttpPhase
{
    [JsonPropertyName("status_code")]
    public int StatusCode { get; init; }

    [JsonPropertyName("negotiated_version")]
    public string? NegotiatedVersion { get; init; }

    [JsonPropertyName("ttfb_ms")]
    public double TtfbMs { get; init; }

    [JsonPropertyName("total_duration_ms")]
    public double TotalDurationMs { get; init; }

    /// <summary>Measured throughput in MB/s; null for normal latency probes.</summary>
    [JsonPropertyName("throughput_mbps")]
    public double? ThroughputMbps { get; init; }

    /// <summary>End-to-end goodput incl. connection setup; throughput probes only.</summary>
    [JsonPropertyName("goodput_mbps")]
    public double? GoodputMbps { get; init; }

    /// <summary>Bytes requested (download) or sent (upload); 0 for normal probes.</summary>
    [JsonPropertyName("payload_bytes")]
    public long? PayloadBytes { get; init; }

    [JsonPropertyName("body_size_bytes")]
    public long? BodySizeBytes { get; init; }

    [JsonPropertyName("redirect_count")]
    public uint? RedirectCount { get; init; }

    /// <summary>Process CPU time (user + system) consumed during this probe (ms).</summary>
    [JsonPropertyName("cpu_time_ms")]
    public double? CpuTimeMs { get; init; }

    /// <summary>Client-side voluntary context switches during this probe (Unix only).</summary>
    [JsonPropertyName("csw_voluntary")]
    public ulong? CswVoluntary { get; init; }

    /// <summary>Client-side involuntary context switches during this probe (Unix only).</summary>
    [JsonPropertyName("csw_involuntary")]
    public ulong? CswInvoluntary { get; init; }
}

/// <summary>UDP latency probe result (mirrors Rust `UdpResult`).</summary>
public sealed record UdpPhase
{
    [JsonPropertyName("rtt_avg_ms")]
    public double RttAvgMs { get; init; }

    [JsonPropertyName("rtt_min_ms")]
    public double RttMinMs { get; init; }

    [JsonPropertyName("rtt_p95_ms")]
    public double RttP95Ms { get; init; }

    [JsonPropertyName("jitter_ms")]
    public double JitterMs { get; init; }

    [JsonPropertyName("loss_percent")]
    public double LossPercent { get; init; }

    [JsonPropertyName("probe_count")]
    public uint ProbeCount { get; init; }

    [JsonPropertyName("success_count")]
    public uint SuccessCount { get; init; }
}

/// <summary>
/// Server-side timing parsed from X-Networker-* / Server-Timing response
/// headers (mirrors Rust `ServerTimingResult`). Carries the network-vs-server
/// latency split: <c>server_ms</c> (time the server did work) vs
/// <c>network_ms</c> (transfer, = max(0, ttfb − server_ms)), with
/// <c>split_anomaly</c> flagging datapoints where the reported server time
/// exceeded the measured wall and the network leg was clamped to 0.
/// </summary>
public sealed record ServerTimingPhase
{
    /// <summary>Server-side portion of total request latency (ms).</summary>
    [JsonPropertyName("server_ms")]
    public double? ServerMs { get; init; }

    /// <summary>Network-transfer portion of total request latency (ms).</summary>
    [JsonPropertyName("network_ms")]
    public double? NetworkMs { get; init; }

    /// <summary>LagHound SDK app processing time (Server-Timing: app;dur=X).</summary>
    [JsonPropertyName("app_ms")]
    public double? AppMs { get; init; }

    /// <summary>True when the split was clamped (reported server_ms &gt; ttfb_ms).
    /// Absent in the Rust JSON when false (skip_serializing_if).</summary>
    [JsonPropertyName("split_anomaly")]
    public bool SplitAnomaly { get; init; }

    /// <summary>Server processing time (Server-Timing: proc;dur=X, download only).</summary>
    [JsonPropertyName("processing_ms")]
    public double? ProcessingMs { get; init; }

    /// <summary>Body drain time on server side (Server-Timing: recv;dur=X, upload only).</summary>
    [JsonPropertyName("recv_body_ms")]
    public double? RecvBodyMs { get; init; }

    /// <summary>Total server time (Server-Timing: total;dur=X).</summary>
    [JsonPropertyName("total_server_ms")]
    public double? TotalServerMs { get; init; }
}
