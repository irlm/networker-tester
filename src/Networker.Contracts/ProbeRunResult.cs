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

    /// <summary>Source-network context (default route, interface kind, VPN
    /// heuristic) collected best-effort at run start (mirrors Rust
    /// <c>NetworkContext</c>). Null on older testers.</summary>
    [JsonPropertyName("client_network")]
    public NetworkContextInfo? ClientNetwork { get; init; }

    /// <summary>Offline GeoIP enrichment of the client's egress IP (mirrors
    /// Rust <c>GeoInfo</c>). Only present when a local MaxMind DB is
    /// configured and the egress IP is public.</summary>
    [JsonPropertyName("client_geo")]
    public GeoInfo? ClientGeo { get; init; }

    /// <summary>Offline GeoIP enrichment of the first resolved target IP.</summary>
    [JsonPropertyName("target_geo")]
    public GeoInfo? TargetGeo { get; init; }

    /// <summary>System load sampled on the tester at run start (mirrors Rust
    /// <c>LoadSample</c>, measurement-gap #15). Best-effort per platform.</summary>
    [JsonPropertyName("client_load_before")]
    public LoadSample? ClientLoadBefore { get; init; }

    /// <summary>System load sampled on the tester at run end.</summary>
    [JsonPropertyName("client_load_after")]
    public LoadSample? ClientLoadAfter { get; init; }

    /// <summary>One-shot SNTP cross-check of the client clock (mirrors Rust
    /// <c>ClockSync</c>, measurement-gap #16). Null when NTP was unreachable
    /// or disabled.</summary>
    [JsonPropertyName("clock_sync")]
    public ClockSync? ClockSync { get; init; }

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

    /// <summary>Latency-under-load / bufferbloat result (<c>rpm</c> mode only).</summary>
    [JsonPropertyName("rpm")]
    public RpmPhase? Rpm { get; init; }

    /// <summary>ICMP echo RTT result (<c>ping</c> mode only).</summary>
    [JsonPropertyName("ping")]
    public PingPhase? Ping { get; init; }

    /// <summary>Hop-discovery / traceroute-style result (<c>path</c> mode only).</summary>
    [JsonPropertyName("path")]
    public PathPhase? Path { get; init; }

    /// <summary>IPv4-vs-IPv6 comparison result (<c>dualstack</c> mode only).</summary>
    [JsonPropertyName("dualstack")]
    public DualStackPhase? DualStack { get; init; }

    /// <summary>WebSocket upgrade + message-RTT result (<c>websocket</c> mode only).</summary>
    [JsonPropertyName("websocket")]
    public WebSocketPhase? WebSocket { get; init; }

    /// <summary>Path-MTU discovery result (<c>pmtud</c> mode only).</summary>
    [JsonPropertyName("pmtud")]
    public PmtudPhase? Pmtud { get; init; }
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

/// <summary>
/// Latency-under-load / bufferbloat probe result (mirrors Rust `RpmResult`).
/// Phase 1 measures unloaded UDP echo RTT; phase 2 repeats it while sustained
/// HTTP downloads saturate the link. Headlines: <c>rpm</c> = 60000 / loaded
/// avg RTT (Apple-RPM-style, higher is better) and <c>bufferbloat_factor</c> =
/// loaded avg / unloaded avg (1.0 ≈ no bufferbloat).
/// </summary>
public sealed record RpmPhase
{
    [JsonPropertyName("remote_addr")]
    public string RemoteAddr { get; init; } = string.Empty;

    [JsonPropertyName("unloaded_probe_count")]
    public uint UnloadedProbeCount { get; init; }

    [JsonPropertyName("unloaded_success_count")]
    public uint UnloadedSuccessCount { get; init; }

    [JsonPropertyName("unloaded_loss_percent")]
    public double UnloadedLossPercent { get; init; }

    [JsonPropertyName("unloaded_rtt_min_ms")]
    public double UnloadedRttMinMs { get; init; }

    [JsonPropertyName("unloaded_rtt_avg_ms")]
    public double UnloadedRttAvgMs { get; init; }

    [JsonPropertyName("unloaded_rtt_p95_ms")]
    public double UnloadedRttP95Ms { get; init; }

    [JsonPropertyName("unloaded_jitter_ms")]
    public double UnloadedJitterMs { get; init; }

    [JsonPropertyName("loaded_probe_count")]
    public uint LoadedProbeCount { get; init; }

    [JsonPropertyName("loaded_success_count")]
    public uint LoadedSuccessCount { get; init; }

    [JsonPropertyName("loaded_loss_percent")]
    public double LoadedLossPercent { get; init; }

    [JsonPropertyName("loaded_rtt_min_ms")]
    public double LoadedRttMinMs { get; init; }

    [JsonPropertyName("loaded_rtt_avg_ms")]
    public double LoadedRttAvgMs { get; init; }

    [JsonPropertyName("loaded_rtt_p95_ms")]
    public double LoadedRttP95Ms { get; init; }

    [JsonPropertyName("loaded_jitter_ms")]
    public double LoadedJitterMs { get; init; }

    /// <summary>Round-trips per minute under load: 60000 / loaded_rtt_avg_ms.
    /// Null when every loaded probe was lost.</summary>
    [JsonPropertyName("rpm")]
    public double? Rpm { get; init; }

    /// <summary>loaded_rtt_avg_ms / unloaded_rtt_avg_ms. Null when either
    /// phase has no successful probes.</summary>
    [JsonPropertyName("bufferbloat_factor")]
    public double? BufferbloatFactor { get; init; }

    /// <summary>Wall-clock duration of the loaded phase (ms).</summary>
    [JsonPropertyName("load_duration_ms")]
    public double LoadDurationMs { get; init; }

    /// <summary>Bytes delivered by downloads that completed inside the load window.</summary>
    [JsonPropertyName("load_bytes_transferred")]
    public ulong LoadBytesTransferred { get; init; }

    /// <summary>Number of downloads that completed inside the load window.</summary>
    [JsonPropertyName("load_downloads_completed")]
    public uint LoadDownloadsCompleted { get; init; }

    /// <summary>Mean throughput across completed downloads (MB/s); null when
    /// no download completed inside the window.</summary>
    [JsonPropertyName("load_throughput_mbps")]
    public double? LoadThroughputMbps { get; init; }

    [JsonPropertyName("started_at")]
    public DateTimeOffset StartedAt { get; init; }
}

/// <summary>ICMP echo RTT result (mirrors Rust `PingResult`).</summary>
public sealed record PingPhase
{
    /// <summary>IP actually pinged (first resolved address of the target host).</summary>
    [JsonPropertyName("remote_addr")]
    public string RemoteAddr { get; init; } = string.Empty;

    [JsonPropertyName("probe_count")]
    public uint ProbeCount { get; init; }

    [JsonPropertyName("success_count")]
    public uint SuccessCount { get; init; }

    [JsonPropertyName("loss_percent")]
    public double LossPercent { get; init; }

    [JsonPropertyName("rtt_min_ms")]
    public double RttMinMs { get; init; }

    [JsonPropertyName("rtt_avg_ms")]
    public double RttAvgMs { get; init; }

    [JsonPropertyName("rtt_p95_ms")]
    public double RttP95Ms { get; init; }

    [JsonPropertyName("jitter_ms")]
    public double JitterMs { get; init; }

    /// <summary>Per-probe RTT values (ms), null if the echo was lost.</summary>
    [JsonPropertyName("probe_rtts_ms")]
    public IReadOnlyList<double?> ProbeRttsMs { get; init; } = Array.Empty<double?>();

    /// <summary>IP TTL / hop limit observed on echo replies, when the platform
    /// exposes it to unprivileged sockets. Null is "not observable".</summary>
    [JsonPropertyName("reply_ttl")]
    public uint? ReplyTtl { get; init; }

    [JsonPropertyName("started_at")]
    public DateTimeOffset StartedAt { get; init; }
}

/// <summary>One hop discovered by the `path` probe (mirrors Rust `PathHop`).</summary>
public sealed record PathHopEntry
{
    /// <summary>TTL value that surfaced this hop (1-based).</summary>
    [JsonPropertyName("index")]
    public uint Index { get; init; }

    /// <summary>Router address that answered; null when the hop did not
    /// respond within the per-hop timeout (a traceroute `*`).</summary>
    [JsonPropertyName("addr")]
    public string? Addr { get; init; }

    /// <summary>Probe-send → ICMP-error-arrival RTT (ms); null for silent hops.</summary>
    [JsonPropertyName("rtt_ms")]
    public double? RttMs { get; init; }
}

/// <summary>
/// Hop-discovery result (mirrors Rust `PathResult`). <c>method</c> records HOW
/// the hops were (or were not) obtained — unprivileged capability differs per
/// platform, and hops are never fabricated (empty on macOS/Windows).
/// </summary>
public sealed record PathPhase
{
    /// <summary>Destination address the probes were aimed at.</summary>
    [JsonPropertyName("remote_addr")]
    public string RemoteAddr { get; init; } = string.Empty;

    /// <summary>Discovered hops in TTL order; empty when the platform cannot
    /// observe hop addresses unprivileged (see <c>method</c>).</summary>
    [JsonPropertyName("hops")]
    public IReadOnlyList<PathHopEntry> Hops { get; init; } = Array.Empty<PathHopEntry>();

    /// <summary>Number of hops to the destination; null when the destination
    /// never answered.</summary>
    [JsonPropertyName("hop_count")]
    public uint? HopCount { get; init; }

    /// <summary>True when a destination-generated response was observed.</summary>
    [JsonPropertyName("destination_reached")]
    public bool DestinationReached { get; init; }

    /// <summary>RTT to the destination itself (ms), when it answered.</summary>
    [JsonPropertyName("destination_rtt_ms")]
    public double? DestinationRttMs { get; init; }

    /// <summary>How the path was measured, e.g. "udp-ttl/ip-recverr".</summary>
    [JsonPropertyName("method")]
    public string Method { get; init; } = string.Empty;

    /// <summary>Highest TTL probed.</summary>
    [JsonPropertyName("max_ttl")]
    public uint MaxTtl { get; init; }

    [JsonPropertyName("started_at")]
    public DateTimeOffset StartedAt { get; init; }
}

/// <summary>One address-family leg of the dualstack probe (mirrors Rust `DualStackLeg`).</summary>
public sealed record DualStackLegPhase
{
    /// <summary>False when the family had no DNS records — nothing was probed,
    /// nothing failed.</summary>
    [JsonPropertyName("attempted")]
    public bool Attempted { get; init; }

    /// <summary>True when the HTTP GET over this family completed.</summary>
    [JsonPropertyName("success")]
    public bool Success { get; init; }

    /// <summary>Address the leg connected to.</summary>
    [JsonPropertyName("addr")]
    public string? Addr { get; init; }

    [JsonPropertyName("dns_ms")]
    public double? DnsMs { get; init; }

    [JsonPropertyName("tcp_ms")]
    public double? TcpMs { get; init; }

    [JsonPropertyName("tls_ms")]
    public double? TlsMs { get; init; }

    [JsonPropertyName("ttfb_ms")]
    public double? TtfbMs { get; init; }

    [JsonPropertyName("total_ms")]
    public double? TotalMs { get; init; }

    /// <summary>Why the leg failed / was not attempted.</summary>
    [JsonPropertyName("error")]
    public string? Error { get; init; }
}

/// <summary>IPv4-vs-IPv6 comparison result (mirrors Rust `DualStackResult`).</summary>
public sealed record DualStackPhase
{
    [JsonPropertyName("ipv4")]
    public DualStackLegPhase? Ipv4 { get; init; }

    [JsonPropertyName("ipv6")]
    public DualStackLegPhase? Ipv6 { get; init; }

    /// <summary>"ipv4" / "ipv6" — family with the lower total_ms among
    /// successful legs; null unless both legs succeeded.</summary>
    [JsonPropertyName("faster_family")]
    public string? FasterFamily { get; init; }

    /// <summary>slower total_ms − faster total_ms (≥ 0); only when both legs succeeded.</summary>
    [JsonPropertyName("delta_ms")]
    public double? DeltaMs { get; init; }

    /// <summary>Which family a happy-eyeballs (RFC 8305) client would use, with the reason.</summary>
    [JsonPropertyName("happy_eyeballs_verdict")]
    public string HappyEyeballsVerdict { get; init; } = string.Empty;

    /// <summary>Grace period used for the verdict (ms) — RFC 8305's recommended 250.</summary>
    [JsonPropertyName("happy_eyeballs_grace_ms")]
    public double HappyEyeballsGraceMs { get; init; }

    [JsonPropertyName("started_at")]
    public DateTimeOffset StartedAt { get; init; }
}

/// <summary>
/// WebSocket probe result (mirrors Rust `WebSocketResult`). Connection phases
/// (DNS/TCP/TLS) live in the attempt's phase sub-results; this carries the
/// HTTP 101 upgrade round-trip and the echo message RTT distribution.
/// </summary>
public sealed record WebSocketPhase
{
    /// <summary>ws:// or wss:// URL the probe connected to.</summary>
    [JsonPropertyName("url")]
    public string Url { get; init; } = string.Empty;

    /// <summary>HTTP 101 upgrade round-trip (ms), excluding DNS/TCP/TLS.</summary>
    [JsonPropertyName("upgrade_ms")]
    public double UpgradeMs { get; init; }

    /// <summary>Status code of the upgrade response (101 on success).</summary>
    [JsonPropertyName("upgrade_status")]
    public int? UpgradeStatus { get; init; }

    /// <summary>Echo messages sent after the upgrade.</summary>
    [JsonPropertyName("message_count")]
    public uint MessageCount { get; init; }

    /// <summary>Echoes received (matched by embedded sequence id).</summary>
    [JsonPropertyName("echo_count")]
    public uint EchoCount { get; init; }

    [JsonPropertyName("loss_percent")]
    public double LossPercent { get; init; }

    [JsonPropertyName("msg_rtt_min_ms")]
    public double MsgRttMinMs { get; init; }

    [JsonPropertyName("msg_rtt_avg_ms")]
    public double MsgRttAvgMs { get; init; }

    [JsonPropertyName("msg_rtt_p95_ms")]
    public double MsgRttP95Ms { get; init; }

    /// <summary>Arrival-order jitter over received echoes.</summary>
    [JsonPropertyName("jitter_ms")]
    public double JitterMs { get; init; }

    /// <summary>Per-message RTT values (ms), null if the echo never arrived.</summary>
    [JsonPropertyName("msg_rtts_ms")]
    public IReadOnlyList<double?> MsgRttsMs { get; init; } = Array.Empty<double?>();

    /// <summary>Bytes per echo message payload.</summary>
    [JsonPropertyName("payload_size")]
    public ulong PayloadSize { get; init; }

    [JsonPropertyName("started_at")]
    public DateTimeOffset StartedAt { get; init; }
}

/// <summary>
/// Path-MTU discovery result (mirrors Rust `PmtudResult`). <c>method</c>
/// records how the verdict was reached (same honesty contract as `PathPhase`);
/// <c>path_mtu</c> is null when no feedback allowed a verdict.
/// </summary>
public sealed record PmtudPhase
{
    /// <summary>Destination address the DF probes were aimed at.</summary>
    [JsonPropertyName("remote_addr")]
    public string RemoteAddr { get; init; } = string.Empty;

    /// <summary>Discovered path MTU in bytes at the IP layer; null when no
    /// feedback allowed a verdict.</summary>
    [JsonPropertyName("path_mtu")]
    public uint? PathMtu { get; init; }

    /// <summary>Largest UDP payload that traversed unfragmented.</summary>
    [JsonPropertyName("max_unfragmented_payload")]
    public uint? MaxUnfragmentedPayload { get; init; }

    /// <summary>DF-flagged datagrams sent during the search.</summary>
    [JsonPropertyName("probes_sent")]
    public uint ProbesSent { get; init; }

    /// <summary>How the MTU was (or was not) determined, e.g. "df-udp-echo/ip-recverr".</summary>
    [JsonPropertyName("method")]
    public string Method { get; init; } = string.Empty;

    /// <summary>Next-hop MTU reported by an ICMP fragmentation-needed message
    /// (Linux error queue only).</summary>
    [JsonPropertyName("icmp_mtu")]
    public uint? IcmpMtu { get; init; }

    /// <summary>MTU of the default-route interface, for contrast with the path value.</summary>
    [JsonPropertyName("local_mtu")]
    public uint? LocalMtu { get; init; }

    /// <summary>IP + UDP header bytes assumed when converting payload ⇄ MTU
    /// (28 for IPv4, 48 for IPv6).</summary>
    [JsonPropertyName("header_bytes")]
    public uint HeaderBytes { get; init; }

    /// <summary>True when the search confirmed its ceiling without finding a
    /// "too big" bound — the true path MTU may be larger.</summary>
    [JsonPropertyName("lower_bound_only")]
    public bool LowerBoundOnly { get; init; }

    [JsonPropertyName("started_at")]
    public DateTimeOffset StartedAt { get; init; }
}

/// <summary>
/// Best-effort description of the SOURCE network the client ran from (mirrors
/// Rust `NetworkContext`). Every field is optional — collection failures leave
/// fields null and never abort a run.
/// </summary>
public sealed record NetworkContextInfo
{
    /// <summary>Name of the interface owning the default route (e.g. "en0").</summary>
    [JsonPropertyName("default_interface")]
    public string? DefaultInterface { get; init; }

    /// <summary>"ethernet" | "wifi" | "virtual" | "unknown".</summary>
    [JsonPropertyName("interface_kind")]
    public string? InterfaceKind { get; init; }

    /// <summary>MTU of the default interface.</summary>
    [JsonPropertyName("mtu")]
    public uint? Mtu { get; init; }

    /// <summary>Egress address actually used for this run.</summary>
    [JsonPropertyName("local_ip")]
    public string? LocalIp { get; init; }

    /// <summary>Default gateway address.</summary>
    [JsonPropertyName("gateway_ip")]
    public string? GatewayIp { get; init; }

    /// <summary>Conservative VPN heuristic: true only when the default route
    /// goes through a tunnel-like interface.</summary>
    [JsonPropertyName("vpn_detected")]
    public bool? VpnDetected { get; init; }

    /// <summary>Tunnel interface name when <c>vpn_detected</c> is true.</summary>
    [JsonPropertyName("vpn_interface")]
    public string? VpnInterface { get; init; }

    /// <summary>Whether an IPv6 default route is present on this host.</summary>
    [JsonPropertyName("ipv6_available")]
    public bool? Ipv6Available { get; init; }
}

/// <summary>
/// Geo / ISP / ASN enrichment for one IP address, resolved from local MaxMind
/// databases (mirrors Rust `GeoInfo`). All fields best-effort.
/// </summary>
public sealed record GeoInfo
{
    /// <summary>ISO 3166-1 alpha-2 country code, e.g. "US".</summary>
    [JsonPropertyName("country")]
    public string? Country { get; init; }

    /// <summary>City name (English).</summary>
    [JsonPropertyName("city")]
    public string? City { get; init; }

    /// <summary>Autonomous system number, e.g. 13335.</summary>
    [JsonPropertyName("asn")]
    public uint? Asn { get; init; }

    /// <summary>Autonomous system organization, e.g. "Cloudflare, Inc.".</summary>
    [JsonPropertyName("as_org")]
    public string? AsOrg { get; init; }

    /// <summary>Build date (YYYY-MM-DD) of the .mmdb the lookup came from.</summary>
    [JsonPropertyName("db_date")]
    public string? DbDate { get; init; }
}

/// <summary>
/// System load sampled on the tester (mirrors Rust `LoadSample`,
/// measurement-gap #15). Best-effort per platform — a field is null when the
/// platform does not expose it, never fabricated.
/// </summary>
public sealed record LoadSample
{
    /// <summary>1-minute load average; above the core count means the tester
    /// itself was contended and measurements may be noisy.</summary>
    [JsonPropertyName("load_avg_1m")]
    public double? LoadAvg1m { get; init; }

    /// <summary>CPU busy percentage over a sampling window (currently always
    /// null on the Rust side — reserved).</summary>
    [JsonPropertyName("cpu_busy_percent")]
    public double? CpuBusyPercent { get; init; }

    /// <summary>Available (reclaimable) memory in MB.</summary>
    [JsonPropertyName("mem_available_mb")]
    public ulong? MemAvailableMb { get; init; }
}

/// <summary>
/// One-shot SNTP (RFC 4330) cross-check of the client clock (mirrors Rust
/// `ClockSync`, measurement-gap #16). Best-effort; whole struct omitted when
/// the query failed or was disabled.
/// </summary>
public sealed record ClockSync
{
    /// <summary>NTP server queried (default pool.ntp.org:123).</summary>
    [JsonPropertyName("ntp_server")]
    public string? NtpServer { get; init; }

    /// <summary>Estimated client clock offset vs the NTP server in ms
    /// (positive = local clock is behind the server).</summary>
    [JsonPropertyName("offset_ms")]
    public double? OffsetMs { get; init; }

    /// <summary>SNTP round-trip delay in ms.</summary>
    [JsonPropertyName("round_trip_ms")]
    public double? RoundTripMs { get; init; }
}
