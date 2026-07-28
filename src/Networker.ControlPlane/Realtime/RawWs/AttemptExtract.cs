using System.Text.Json;

namespace Networker.ControlPlane.Realtime.RawWs;

/// <summary>
/// The typed shape of one streamed attempt, extracted from the tester's live
/// attempt JSON (the same snake_case field names <c>AttemptView</c> pins). Only
/// the fields the V001 read paths + reports consume are captured; absent phases
/// are null. The full attempt JSON is retained verbatim in
/// <see cref="ExtraJson"/> for the <c>extra_json</c> column.
/// </summary>
public sealed record ParsedAttempt(
    Guid AttemptId,
    Guid RunId,
    string Protocol,
    int SequenceNum,
    DateTime? StartedAt,
    DateTime? FinishedAt,
    bool Success,
    string? ErrorMessage,
    int RetryCount,
    string? ExtraJson,
    string TargetHost,
    string TargetUrl,
    ParsedDns? Dns,
    ParsedTcp? Tcp,
    ParsedTls? Tls,
    ParsedHttp? Http,
    ParsedUdp? Udp,
    ParsedServerTiming? ServerTiming);

public sealed record ParsedDns(string? QueryName, string? ResolvedIps, double? DurationMs, bool Success);

public sealed record ParsedTcp(
    string? RemoteAddr, double? ConnectDurationMs, int? MssBytes, double? RttEstimateMs,
    long? Retransmits, long? TotalRetrans, long? SndCwnd, string? CongestionAlgorithm,
    long? DeliveryRateBps, double? MinRttMs);

public sealed record ParsedTls(
    string? ProtocolVersion, string? CipherSuite, string? AlpnNegotiated,
    DateTime? CertExpiry, double? HandshakeDurationMs);

public sealed record ParsedHttp(
    string? NegotiatedVersion, int? StatusCode, int? BodySizeBytes, double? TtfbMs,
    double? TotalDurationMs, int? RedirectCount, long? PayloadBytes, double? ThroughputMbps);

public sealed record ParsedUdp(
    int? ProbeCount, int? SuccessCount, double? LossPercent,
    double? RttMinMs, double? RttAvgMs, double? RttP95Ms, double? JitterMs);

public sealed record ParsedServerTiming(double? RecvBodyMs, double? ProcessingMs, double? TotalServerMs);

/// <summary>Pure JSON → <see cref="ParsedAttempt"/> extraction (no I/O).</summary>
public static class AttemptExtract
{
    /// <summary>
    /// Parse one <c>attempt_event.attempt</c> object. Returns null only when the
    /// top-level attempt is unusable (no attempt_id) — a malformed frame is
    /// dropped rather than persisted with a random id.
    /// </summary>
    public static ParsedAttempt? Parse(Guid runId, JsonElement attempt)
    {
        if (attempt.ValueKind != JsonValueKind.Object)
        {
            return null;
        }
        if (Guid(attempt, "attempt_id") is not { } attemptId)
        {
            return null;
        }

        // Best-effort target for the V001 testrun row (nothing reads it beyond
        // the FK / RunId): the resolved hostname if DNS ran, else the TCP peer.
        var dns = Child(attempt, "dns");
        var tcp = Child(attempt, "tcp");
        var host = (dns is { } d ? Str(d, "query_name") : null)
            ?? (tcp is { } t ? StripPort(Str(t, "remote_addr")) : null)
            ?? "";

        return new ParsedAttempt(
            AttemptId: attemptId,
            RunId: runId,
            Protocol: Str(attempt, "protocol") ?? "unknown",
            SequenceNum: Int(attempt, "sequence_num") ?? 0,
            StartedAt: Date(attempt, "started_at"),
            FinishedAt: Date(attempt, "finished_at"),
            Success: Bool(attempt, "success") ?? false,
            ErrorMessage: Str(attempt, "error_message"),
            RetryCount: Int(attempt, "retry_count") ?? 0,
            ExtraJson: attempt.GetRawText(),
            TargetHost: host,
            TargetUrl: host,
            Dns: ParseDns(dns),
            Tcp: ParseTcp(Child(attempt, "tcp")),
            Tls: ParseTls(Child(attempt, "tls")),
            Http: ParseHttp(Child(attempt, "http")),
            Udp: ParseUdp(Child(attempt, "udp")),
            ServerTiming: ParseServerTiming(Child(attempt, "server_timing")));
    }

    private static ParsedDns? ParseDns(JsonElement? e) => e is { } d
        ? new ParsedDns(Str(d, "query_name"), JoinIps(d, "resolved_ips"), Dbl(d, "duration_ms"), Bool(d, "success") ?? false)
        : null;

    private static ParsedTcp? ParseTcp(JsonElement? e) => e is { } t
        ? new ParsedTcp(Str(t, "remote_addr"), Dbl(t, "connect_duration_ms"), Int(t, "mss_bytes"),
            Dbl(t, "rtt_estimate_ms"), Long(t, "retransmits"), Long(t, "total_retrans"), Long(t, "snd_cwnd"),
            Str(t, "congestion_algorithm"), Long(t, "delivery_rate_bps"), Dbl(t, "min_rtt_ms"))
        : null;

    private static ParsedTls? ParseTls(JsonElement? e) => e is { } t
        ? new ParsedTls(Str(t, "protocol_version"), Str(t, "cipher_suite"), Str(t, "alpn_negotiated"),
            Date(t, "cert_expiry"), Dbl(t, "handshake_duration_ms"))
        : null;

    private static ParsedHttp? ParseHttp(JsonElement? e) => e is { } h
        ? new ParsedHttp(Str(h, "negotiated_version"), Int(h, "status_code"), Int(h, "body_size_bytes"),
            Dbl(h, "ttfb_ms"), Dbl(h, "total_duration_ms"), Int(h, "redirect_count"),
            Long(h, "payload_bytes"), Dbl(h, "throughput_mbps"))
        : null;

    private static ParsedUdp? ParseUdp(JsonElement? e) => e is { } u
        ? new ParsedUdp(Int(u, "probe_count"), Int(u, "success_count"), Dbl(u, "loss_percent"),
            Dbl(u, "rtt_min_ms"), Dbl(u, "rtt_avg_ms"), Dbl(u, "rtt_p95_ms"), Dbl(u, "jitter_ms"))
        : null;

    private static ParsedServerTiming? ParseServerTiming(JsonElement? e) => e is { } s
        ? new ParsedServerTiming(Dbl(s, "recv_body_ms"), Dbl(s, "processing_ms"), Dbl(s, "total_server_ms"))
        : null;

    // ── JSON accessors (tolerant: wrong-kind / missing → null) ───────────────

    private static JsonElement? Child(JsonElement e, string name) =>
        e.ValueKind == JsonValueKind.Object && e.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.Object
            ? v : null;

    private static string? Str(JsonElement e, string name) =>
        e.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.String ? v.GetString() : null;

    private static bool? Bool(JsonElement e, string name) =>
        e.TryGetProperty(name, out var v) && v.ValueKind is JsonValueKind.True or JsonValueKind.False ? v.GetBoolean() : null;

    private static double? Dbl(JsonElement e, string name) =>
        e.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.Number && v.TryGetDouble(out var d) ? d : null;

    private static int? Int(JsonElement e, string name) =>
        e.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.Number && v.TryGetInt32(out var i) ? i : null;

    private static long? Long(JsonElement e, string name) =>
        e.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.Number && v.TryGetInt64(out var l) ? l : null;

    private static DateTime? Date(JsonElement e, string name) =>
        e.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.String && v.TryGetDateTime(out var d)
            ? d.ToUniversalTime() : null;

    private static Guid? Guid(JsonElement e, string name) =>
        e.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.String && System.Guid.TryParse(v.GetString(), out var g)
            ? g : null;

    /// <summary>"1.2.3.4:443" → "1.2.3.4"; leaves bare hosts and IPv6 alone.</summary>
    private static string? StripPort(string? addr)
    {
        if (string.IsNullOrEmpty(addr) || addr.Contains(':') is false)
        {
            return addr;
        }
        var lastColon = addr.LastIndexOf(':');
        // Only strip a trailing :port on an IPv4/host (one colon); leave IPv6.
        return addr.IndexOf(':') == lastColon ? addr[..lastColon] : addr;
    }

    private static string? JoinIps(JsonElement e, string name)
    {
        if (!e.TryGetProperty(name, out var v) || v.ValueKind != JsonValueKind.Array)
        {
            return null;
        }
        var ips = v.EnumerateArray().Where(x => x.ValueKind == JsonValueKind.String).Select(x => x.GetString());
        return string.Join(",", ips.Where(s => !string.IsNullOrEmpty(s)));
    }
}
