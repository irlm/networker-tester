using System.Text.Json.Serialization;

namespace Networker.Contracts;

// C# side of the frozen networker-tester JSON contract.
//
// These records mirror the Rust `TestRun` / `RequestAttempt` / phase-result
// structs emitted by `networker-tester --json-stdout`. Only the fields the C#
// app layer currently consumes are modelled; the Rust output carries many more
// fields (kernel TCP stats, benchmark metadata, browser results, ...). Unknown
// JSON members are ignored on deserialization, so the contract can grow
// additively without breaking this side — a `schema_version` bump signals when
// consumers must be revised.
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
}

/// <summary>DNS resolution phase timing (mirrors Rust `DnsResult`).</summary>
public sealed record DnsPhase
{
    [JsonPropertyName("duration_ms")]
    public double DurationMs { get; init; }

    [JsonPropertyName("success")]
    public bool Success { get; init; }
}

/// <summary>TCP connect phase timing (mirrors Rust `TcpResult`).</summary>
public sealed record TcpPhase
{
    [JsonPropertyName("connect_duration_ms")]
    public double ConnectDurationMs { get; init; }

    [JsonPropertyName("success")]
    public bool Success { get; init; }
}

/// <summary>TLS handshake phase timing (mirrors Rust `TlsResult`).</summary>
public sealed record TlsPhase
{
    [JsonPropertyName("handshake_duration_ms")]
    public double HandshakeDurationMs { get; init; }

    [JsonPropertyName("protocol_version")]
    public string? ProtocolVersion { get; init; }

    [JsonPropertyName("success")]
    public bool Success { get; init; }
}

/// <summary>HTTP request phase timing (mirrors Rust `HttpResult`).</summary>
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
}
