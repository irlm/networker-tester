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
