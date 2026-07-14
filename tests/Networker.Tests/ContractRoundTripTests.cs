using System.Text.Json;
using Networker.Contracts;
using Xunit;

namespace Networker.Tests;

/// <summary>
/// Guards the Rust↔C# JSON seam — the single most important contract in the
/// hybrid. A silent snake_case↔PascalCase mismatch would deserialize every
/// timing field to 0/null and the whole system would "work" while reporting
/// garbage. These tests fail loudly if a field stops round-tripping.
///
/// The sample payload mirrors what <c>networker-tester --json-stdout</c> emits
/// (schema_version + per-phase snake_case fields), including extra fields the
/// C# layer does not model — proving the contract grows additively.
/// </summary>
public class ContractRoundTripTests
{
    private const string SampleTesterJson = """
    {
      "schema_version": "1.0",
      "run_id": "9ba79159-be2e-4b2c-8989-c9c032b8a091",
      "target_url": "https://www.cloudflare.com",
      "target_host": "www.cloudflare.com",
      "modes": ["http1"],
      "client_version": "0.28.13",
      "some_future_field": "the C# layer ignores this",
      "attempts": [
        {
          "attempt_id": "a1",
          "protocol": "http1",
          "sequence_num": 0,
          "success": true,
          "dns": { "duration_ms": 8.4, "success": true },
          "tcp": { "connect_duration_ms": 12.1, "success": true },
          "tls": { "handshake_duration_ms": 148.6, "success": true, "protocol_version": "TLSv1.3" },
          "http": {
            "status_code": 200,
            "negotiated_version": "HTTP/1.1",
            "ttfb_ms": 107.8,
            "total_duration_ms": 372.6
          }
        }
      ]
    }
    """;

    private static ProbeRunResult Deserialize(string json) =>
        JsonSerializer.Deserialize(json, ProbeContractJsonContext.Default.ProbeRunResult)!;

    [Fact]
    public void TopLevel_fields_round_trip()
    {
        var r = Deserialize(SampleTesterJson);

        Assert.Equal("1.0", r.SchemaVersion);
        Assert.Equal("9ba79159-be2e-4b2c-8989-c9c032b8a091", r.RunId);
        Assert.Equal("https://www.cloudflare.com", r.TargetUrl);
        Assert.Equal("www.cloudflare.com", r.TargetHost);
        Assert.Equal("0.28.13", r.ClientVersion);
        Assert.Equal(new[] { "http1" }, r.Modes);
        Assert.Single(r.Attempts);
    }

    [Fact]
    public void Per_phase_timings_round_trip()
    {
        var a = Deserialize(SampleTesterJson).Attempts[0];

        Assert.Equal("http1", a.Protocol);
        Assert.True(a.Success);

        Assert.NotNull(a.Dns);
        Assert.Equal(8.4, a.Dns!.DurationMs);

        Assert.NotNull(a.Tcp);
        Assert.Equal(12.1, a.Tcp!.ConnectDurationMs);

        Assert.NotNull(a.Tls);
        Assert.Equal(148.6, a.Tls!.HandshakeDurationMs);

        Assert.NotNull(a.Http);
        Assert.Equal(200, a.Http!.StatusCode);
        Assert.Equal("HTTP/1.1", a.Http.NegotiatedVersion);
        Assert.Equal(107.8, a.Http.TtfbMs);
        Assert.Equal(372.6, a.Http.TotalDurationMs);
    }

    [Fact]
    public void Unknown_rust_fields_are_ignored_not_thrown()
    {
        // "some_future_field" above is not modeled in C#. Deserialization must
        // tolerate it so the Rust contract can add fields without breaking the
        // agent — the whole point of the versioned, additive seam.
        var ex = Record.Exception(() => Deserialize(SampleTesterJson));
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
