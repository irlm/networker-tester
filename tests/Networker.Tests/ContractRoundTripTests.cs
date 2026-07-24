using System.Text.Json;
using System.Text.RegularExpressions;
using Networker.Contracts;
using Xunit;

namespace Networker.Tests;

/// <summary>
/// Guards the Rust↔C# JSON seam — the single most important contract in the
/// hybrid. A silent snake_case↔PascalCase mismatch would deserialize every
/// timing field to 0/null and the whole system would "work" while reporting
/// garbage. These tests fail loudly if a field stops round-tripping.
///
/// The golden fixture (fixtures/tester-golden.json) is REAL output captured
/// from <c>networker-tester --json-stdout</c> probing a local
/// <c>networker-endpoint</c> — never hand-typed. Regenerate it with
/// <c>scripts/regenerate-contract-golden.sh</c> whenever the tester's TestRun
/// schema changes, and commit the result alongside the schema change. Because
/// the payload is live capture, assertions are structural (fields present,
/// timings positive, version well-formed) rather than pinned to exact values.
/// </summary>
public class ContractRoundTripTests
{
    private static readonly string GoldenJson = File.ReadAllText(
        Path.Combine(AppContext.BaseDirectory, "fixtures", "tester-golden.json"));

    private static ProbeRunResult Deserialize(string json) =>
        JsonSerializer.Deserialize(json, ProbeContractJsonContext.Default.ProbeRunResult)!;

    [Fact]
    public void Golden_top_level_fields_round_trip()
    {
        var r = Deserialize(GoldenJson);

        Assert.Equal("1.0", r.SchemaVersion);
        Assert.True(Guid.TryParse(r.RunId, out _), $"run_id not a UUID: '{r.RunId}'");
        Assert.StartsWith("https://", r.TargetUrl);
        Assert.False(string.IsNullOrWhiteSpace(r.TargetHost));
        Assert.Contains("http1", r.Modes);
        Assert.NotEmpty(r.Attempts);

        // client_version is the tester's CARGO_PKG_VERSION — a dotted triple.
        // Not pinned to an exact value so the fixture doesn't have to be
        // regenerated on every release, only on schema changes.
        Assert.Matches(new Regex(@"^\d+\.\d+\.\d+$"), r.ClientVersion);
    }

    [Fact]
    public void Golden_per_phase_timings_round_trip()
    {
        var a = Deserialize(GoldenJson).Attempts[0];

        Assert.Equal("http1", a.Protocol);
        Assert.True(a.Success);

        // Non-null with positive durations proves the snake_case field names
        // still match — a naming mismatch would leave these 0/null.
        Assert.NotNull(a.Dns);
        Assert.True(a.Dns!.Success);
        Assert.True(a.Dns.DurationMs >= 0);

        Assert.NotNull(a.Tcp);
        Assert.True(a.Tcp!.Success);
        Assert.True(a.Tcp.ConnectDurationMs > 0, "tcp.connect_duration_ms not positive");

        Assert.NotNull(a.Tls);
        Assert.True(a.Tls!.Success);
        Assert.True(a.Tls.HandshakeDurationMs > 0, "tls.handshake_duration_ms not positive");
        Assert.False(string.IsNullOrWhiteSpace(a.Tls.ProtocolVersion));

        Assert.NotNull(a.Http);
        Assert.Equal(200, a.Http!.StatusCode);
        Assert.Equal("HTTP/1.1", a.Http.NegotiatedVersion);
        Assert.True(a.Http.TtfbMs > 0, "http.ttfb_ms not positive");
        Assert.True(a.Http.TotalDurationMs > 0, "http.total_duration_ms not positive");
    }

    [Fact]
    public void Golden_measurement_depth_fields_round_trip()
    {
        // The widened seam (measurement-gap-analysis-2026-07 finding #1) —
        // TLS negotiation depth, HTTP transfer detail, and server_timing must
        // come through from REAL tester output, not just the hand-written
        // full-fat sample below. Assertions stay structural (present /
        // well-formed) because the fixture is live capture: kernel TCP stats
        // and Server-Timing splits are platform/endpoint dependent, so only
        // the fields the golden capture is guaranteed to carry are pinned.
        var a = Deserialize(GoldenJson).Attempts[0];

        Assert.NotNull(a.Tls);
        Assert.False(string.IsNullOrWhiteSpace(a.Tls!.CipherSuite));
        Assert.False(string.IsNullOrWhiteSpace(a.Tls.AlpnNegotiated));
        Assert.False(string.IsNullOrWhiteSpace(a.Tls.TlsBackend));
        Assert.NotNull(a.Tls.CertExpiry);

        Assert.NotNull(a.Http);
        Assert.NotNull(a.Http!.BodySizeBytes);
        Assert.True(a.Http.BodySizeBytes > 0, "http.body_size_bytes not positive");
        Assert.NotNull(a.Http.RedirectCount);

        // server_timing is present on golden (networker-endpoint echoes
        // X-Networker-* headers) — must parse, even with only a subset of
        // fields populated.
        Assert.NotNull(a.ServerTiming);
    }

    [Fact]
    public void Full_fat_attempt_deserializes_every_widened_field()
    {
        // Hand-written payload exercising EVERY field the widened contract
        // models (a golden capture cannot guarantee kernel TCP stats or the
        // sdkprobe split on all platforms). Field names mirror
        // crates/networker-tester/src/metrics.rs exactly — a rename on either
        // side fails here.
        const string json = """
        {
          "schema_version": "1.0",
          "run_id": "0e0efd8e-6a94-41f5-a2c6-2c1f6c25d1cf",
          "attempts": [ {
            "attempt_id": "a1", "protocol": "download", "sequence_num": 4, "success": true,
            "dns": { "duration_ms": 5.2, "success": true },
            "tcp": {
              "connect_duration_ms": 1.5, "success": true,
              "mss_bytes": 1448, "rtt_estimate_ms": 12.25,
              "retransmits": 0, "total_retrans": 3, "snd_cwnd": 10,
              "congestion_algorithm": "bbr",
              "delivery_rate_bps": 1250000, "min_rtt_ms": 11.9
            },
            "tls": {
              "handshake_duration_ms": 9.1, "protocol_version": "TLSv1_3", "success": true,
              "cipher_suite": "TLS13_AES_256_GCM_SHA384", "alpn_negotiated": "h2",
              "cert_expiry": "2027-01-01T00:00:00Z",
              "resumed": true, "handshake_kind": "resumed", "tls_backend": "rustls"
            },
            "http": {
              "status_code": 200, "negotiated_version": "HTTP/2.0",
              "ttfb_ms": 20.5, "total_duration_ms": 180.0,
              "throughput_mbps": 41.7, "goodput_mbps": 39.2,
              "payload_bytes": 10485760, "body_size_bytes": 10485760,
              "redirect_count": 1, "cpu_time_ms": 6.4,
              "csw_voluntary": 42, "csw_involuntary": 7
            },
            "udp": {
              "rtt_avg_ms": 3.4, "rtt_min_ms": 2.1, "rtt_p95_ms": 6.7,
              "jitter_ms": 0.9, "loss_percent": 2.5,
              "probe_count": 40, "success_count": 39
            },
            "server_timing": {
              "server_ms": 8.5, "network_ms": 12.0, "app_ms": 8.5,
              "split_anomaly": true,
              "processing_ms": 7.9, "recv_body_ms": 1.2, "total_server_ms": 9.3
            }
          } ]
        }
        """;
        var a = Deserialize(json).Attempts[0];

        Assert.NotNull(a.Tcp);
        Assert.Equal(1448u, a.Tcp!.MssBytes);
        Assert.Equal(12.25, a.Tcp.RttEstimateMs);
        Assert.Equal(0u, a.Tcp.Retransmits);
        Assert.Equal(3u, a.Tcp.TotalRetrans);
        Assert.Equal(10u, a.Tcp.SndCwnd);
        Assert.Equal("bbr", a.Tcp.CongestionAlgorithm);
        Assert.Equal(1250000ul, a.Tcp.DeliveryRateBps);
        Assert.Equal(11.9, a.Tcp.MinRttMs);

        Assert.NotNull(a.Tls);
        Assert.Equal("TLS13_AES_256_GCM_SHA384", a.Tls!.CipherSuite);
        Assert.Equal("h2", a.Tls.AlpnNegotiated);
        Assert.Equal(new DateTimeOffset(2027, 1, 1, 0, 0, 0, TimeSpan.Zero), a.Tls.CertExpiry);
        Assert.True(a.Tls.Resumed);
        Assert.Equal("resumed", a.Tls.HandshakeKind);
        Assert.Equal("rustls", a.Tls.TlsBackend);

        Assert.NotNull(a.Http);
        Assert.Equal(41.7, a.Http!.ThroughputMbps);
        Assert.Equal(39.2, a.Http.GoodputMbps);
        Assert.Equal(10485760L, a.Http.PayloadBytes);
        Assert.Equal(10485760L, a.Http.BodySizeBytes);
        Assert.Equal(1u, a.Http.RedirectCount);
        Assert.Equal(6.4, a.Http.CpuTimeMs);
        Assert.Equal(42ul, a.Http.CswVoluntary);
        Assert.Equal(7ul, a.Http.CswInvoluntary);

        Assert.NotNull(a.Udp);
        Assert.Equal(3.4, a.Udp!.RttAvgMs);
        Assert.Equal(2.1, a.Udp.RttMinMs);
        Assert.Equal(6.7, a.Udp.RttP95Ms);
        Assert.Equal(0.9, a.Udp.JitterMs);
        Assert.Equal(2.5, a.Udp.LossPercent);
        Assert.Equal(40u, a.Udp.ProbeCount);
        Assert.Equal(39u, a.Udp.SuccessCount);

        Assert.NotNull(a.ServerTiming);
        Assert.Equal(8.5, a.ServerTiming!.ServerMs);
        Assert.Equal(12.0, a.ServerTiming.NetworkMs);
        Assert.Equal(8.5, a.ServerTiming.AppMs);
        Assert.True(a.ServerTiming.SplitAnomaly);
        Assert.Equal(7.9, a.ServerTiming.ProcessingMs);
        Assert.Equal(1.2, a.ServerTiming.RecvBodyMs);
        Assert.Equal(9.3, a.ServerTiming.TotalServerMs);
    }

    [Fact]
    public void Old_minimal_payload_leaves_every_widened_field_null()
    {
        // Backward compatibility: a pre-widening payload (only the 4 phase
        // timings, none of the additive fields) must deserialize with all
        // widened fields null/default — never throw, never invent values.
        const string json = """
        {
          "schema_version": "1.0",
          "run_id": "r",
          "attempts": [ {
            "attempt_id": "a", "protocol": "http1", "sequence_num": 0, "success": true,
            "dns": { "duration_ms": 5.0, "success": true },
            "tcp": { "connect_duration_ms": 1.0, "success": true },
            "tls": { "handshake_duration_ms": 9.0, "protocol_version": "TLSv1_3", "success": true },
            "http": { "status_code": 200, "negotiated_version": "HTTP/1.1", "ttfb_ms": 2.0, "total_duration_ms": 3.0 }
          } ]
        }
        """;
        var a = Deserialize(json).Attempts[0];

        Assert.NotNull(a.Tcp);
        Assert.Null(a.Tcp!.MssBytes);
        Assert.Null(a.Tcp.RttEstimateMs);
        Assert.Null(a.Tcp.Retransmits);
        Assert.Null(a.Tcp.TotalRetrans);
        Assert.Null(a.Tcp.SndCwnd);
        Assert.Null(a.Tcp.CongestionAlgorithm);
        Assert.Null(a.Tcp.DeliveryRateBps);
        Assert.Null(a.Tcp.MinRttMs);

        Assert.NotNull(a.Tls);
        Assert.Null(a.Tls!.CipherSuite);
        Assert.Null(a.Tls.AlpnNegotiated);
        Assert.Null(a.Tls.CertExpiry);
        Assert.Null(a.Tls.Resumed);
        Assert.Null(a.Tls.HandshakeKind);
        Assert.Null(a.Tls.TlsBackend);

        Assert.NotNull(a.Http);
        Assert.Null(a.Http!.ThroughputMbps);
        Assert.Null(a.Http.GoodputMbps);
        Assert.Null(a.Http.PayloadBytes);
        Assert.Null(a.Http.BodySizeBytes);
        Assert.Null(a.Http.RedirectCount);
        Assert.Null(a.Http.CpuTimeMs);
        Assert.Null(a.Http.CswVoluntary);
        Assert.Null(a.Http.CswInvoluntary);

        Assert.Null(a.Udp);
        Assert.Null(a.ServerTiming);
    }

    [Fact]
    public void Split_anomaly_defaults_false_when_absent()
    {
        // Rust skips serializing `split_anomaly: false` (skip_serializing_if =
        // Not::not) — absence MUST read back as false, not throw.
        const string json = """
        {
          "run_id": "r",
          "attempts": [ { "attempt_id": "a", "protocol": "sdkprobe", "sequence_num": 0, "success": true,
                          "server_timing": { "server_ms": 4.0, "network_ms": 6.0 } } ]
        }
        """;
        var st = Deserialize(json).Attempts[0].ServerTiming;

        Assert.NotNull(st);
        Assert.False(st!.SplitAnomaly);
        Assert.Equal(4.0, st.ServerMs);
        Assert.Equal(6.0, st.NetworkMs);
    }

    [Fact]
    public void Golden_unmodelled_rust_fields_are_ignored_not_thrown()
    {
        // The real payload carries many fields the C# layer does not model
        // (started_at, client_os, baseline, server_info, kernel TCP stats, …).
        // Deserialization must tolerate all of them — the whole point of the
        // versioned, additive seam. The raw JSON is checked to actually
        // contain such fields, so this test cannot silently weaken.
        using var doc = JsonDocument.Parse(GoldenJson);
        Assert.True(doc.RootElement.TryGetProperty("started_at", out _),
            "golden no longer carries unmodelled fields — regenerate it from the real tester");

        var ex = Record.Exception(() => Deserialize(GoldenJson));
        Assert.Null(ex);
    }

    [Fact]
    public void Missing_optional_phase_deserializes_to_null()
    {
        const string json = """
        {
          "schema_version": "1.0",
          "run_id": "r",
          "attempts": [ { "attempt_id": "a", "protocol": "dns", "sequence_num": 0, "success": true,
                          "dns": { "duration_ms": 5.0, "success": true } } ]
        }
        """;
        var a = Deserialize(json).Attempts[0];

        Assert.NotNull(a.Dns);
        Assert.Null(a.Tcp);
        Assert.Null(a.Tls);
        Assert.Null(a.Http);
    }

    [Fact]
    public void Missing_schema_version_does_not_crash()
    {
        // A pre-schema_version tester (or a partial payload) must still parse.
        // NOTE: System.Text.Json source-gen does NOT apply a C# property
        // initializer default (`= "unknown"`) to an ABSENT field — it leaves
        // it null. So consumers must not rely on the initializer for a missing
        // field. In practice the tester always emits schema_version (Phase 0),
        // so this only asserts resilience: parses without throwing.
        const string json = """{ "run_id": "r", "attempts": [] }""";
        var ex = Record.Exception(() => Deserialize(json));

        Assert.Null(ex);
        Assert.Empty(Deserialize(json).Attempts);
    }
}
