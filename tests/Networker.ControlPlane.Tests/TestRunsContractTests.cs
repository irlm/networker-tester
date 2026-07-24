using System.Text.Json;
using System.Text.Json.Nodes;
using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pins the wire shape of <c>GET /api/v2/test-runs/{id}/attempts</c> (audit
/// F3): the <c>{"attempts":[...]}</c> envelope the legacy Rust handler
/// returned and the frontend client types, and the per-attempt snake_case
/// field set that mirrors the tester's <c>RequestAttempt</c> table and the
/// frontend <c>Attempt</c> type.
/// </summary>
public sealed class TestRunsContractTests
{
    private static readonly JsonSerializerOptions WebOptions =
        new(JsonSerializerDefaults.Web);

    private static AttemptView SampleAttempt() => new(
        AttemptId: Guid.Parse("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
        Protocol: "http2",
        SequenceNum: 3,
        StartedAt: new DateTime(2026, 7, 14, 1, 22, 24, DateTimeKind.Utc),
        FinishedAt: new DateTime(2026, 7, 14, 1, 22, 25, DateTimeKind.Utc),
        Success: false,
        ErrorMessage: "connection refused (os error 111)",
        RetryCount: 1);

    [Fact]
    public void Attempts_response_is_an_object_with_a_top_level_attempts_array()
    {
        var json = JsonSerializer.Serialize(
            new AttemptListResponse(new[] { SampleAttempt() }), WebOptions);
        var root = JsonNode.Parse(json)!.AsObject();

        Assert.Single(root);
        Assert.True(root.ContainsKey("attempts"));
        Assert.IsType<JsonArray>(root["attempts"]);
        Assert.Single(root["attempts"]!.AsArray());
    }

    [Fact]
    public void Empty_attempts_still_serializes_the_envelope()
    {
        // An existing run with no probe rows is 200 + `{"attempts":[]}` —
        // never a 404 (the audit-F3 dead end) and never a bare `[]`.
        var json = JsonSerializer.Serialize(
            new AttemptListResponse(Array.Empty<AttemptView>()), WebOptions);

        Assert.Equal("""{"attempts":[]}""", json);
    }

    [Fact]
    public void Attempt_item_emits_the_exact_snake_case_field_set()
    {
        var json = JsonSerializer.Serialize(SampleAttempt(), WebOptions);
        var item = JsonNode.Parse(json)!.AsObject();

        var expected = new[]
        {
            "attempt_id", "protocol", "sequence_num", "started_at",
            "finished_at", "success", "error_message", "retry_count",
        };

        Assert.Equal(expected, item.Select(p => p.Key).ToArray());
        Assert.Equal("http2", item["protocol"]!.GetValue<string>());
        Assert.Equal(3, item["sequence_num"]!.GetValue<int>());
        Assert.False(item["success"]!.GetValue<bool>());
        Assert.Equal(1, item["retry_count"]!.GetValue<int>());
    }

    [Fact]
    public void Absent_phase_objects_are_omitted_not_null()
    {
        // Backward compatibility: an attempt without persisted phase rows must
        // serialize with the EXACT pre-widening field set (asserted above) —
        // no `"dns": null` noise. This is what keeps old runs' wire shape
        // byte-identical after the phase-detail widening.
        var json = JsonSerializer.Serialize(SampleAttempt(), WebOptions);
        var item = JsonNode.Parse(json)!.AsObject();

        foreach (var phase in new[] { "dns", "tcp", "tls", "http", "udp", "server_timing" })
        {
            Assert.False(item.ContainsKey(phase), $"absent phase '{phase}' must be omitted");
        }
    }

    [Fact]
    public void Phase_objects_emit_the_tester_snake_case_field_names()
    {
        // The nested phase objects must use the SAME snake_case names as the
        // tester's live JSON (crates/networker-tester/src/metrics.rs) so the
        // frontend LiveAttempt type renders REST and live attempts through one
        // code path. A rename here silently blanks the run-detail phase cards.
        var full = SampleAttempt() with
        {
            Dns = new AttemptDnsView(1.5, true, "example.com", new[] { "93.184.216.34" }),
            Tcp = new AttemptTcpView(2.5, "93.184.216.34:443", 1448, 12.5, 0, 3, 10, "bbr", 1250000, 11.9),
            Tls = new AttemptTlsView(9.1, "TLSv1_3", "TLS13_AES_256_GCM_SHA384", "h2",
                new DateTime(2027, 1, 1, 0, 0, 0, DateTimeKind.Utc)),
            Http = new AttemptHttpView(200, "HTTP/2.0", 20.5, 180.0, 10485760, 0, 10485760L, 41.7),
            Udp = new AttemptUdpView(3.4, 2.1, 6.7, 0.9, 2.5, 40, 39),
            ServerTiming = new AttemptServerTimingView(7.9, 1.2, 9.3),
        };
        var item = JsonNode.Parse(JsonSerializer.Serialize(full, WebOptions))!.AsObject();

        Assert.Equal(
            new[] { "duration_ms", "success", "query_name", "resolved_ips" },
            item["dns"]!.AsObject().Select(p => p.Key).ToArray());
        Assert.Equal(
            new[]
            {
                "connect_duration_ms", "remote_addr", "mss_bytes", "rtt_estimate_ms",
                "retransmits", "total_retrans", "snd_cwnd", "congestion_algorithm",
                "delivery_rate_bps", "min_rtt_ms",
            },
            item["tcp"]!.AsObject().Select(p => p.Key).ToArray());
        Assert.Equal(
            new[]
            {
                "handshake_duration_ms", "protocol_version", "cipher_suite",
                "alpn_negotiated", "cert_expiry",
            },
            item["tls"]!.AsObject().Select(p => p.Key).ToArray());
        Assert.Equal(
            new[]
            {
                "status_code", "negotiated_version", "ttfb_ms", "total_duration_ms",
                "body_size_bytes", "redirect_count", "payload_bytes", "throughput_mbps",
            },
            item["http"]!.AsObject().Select(p => p.Key).ToArray());
        Assert.Equal(
            new[]
            {
                "rtt_avg_ms", "rtt_min_ms", "rtt_p95_ms", "jitter_ms",
                "loss_percent", "probe_count", "success_count",
            },
            item["udp"]!.AsObject().Select(p => p.Key).ToArray());
        Assert.Equal(
            new[] { "processing_ms", "recv_body_ms", "total_server_ms" },
            item["server_timing"]!.AsObject().Select(p => p.Key).ToArray());

        Assert.Equal("bbr", item["tcp"]!["congestion_algorithm"]!.GetValue<string>());
        Assert.Equal(41.7, item["http"]!["throughput_mbps"]!.GetValue<double>());
        Assert.Equal(0.9, item["udp"]!["jitter_ms"]!.GetValue<double>());
    }

    [Fact]
    public void Null_optional_phase_fields_are_omitted_within_a_phase()
    {
        // Kernel TCP stats are best-effort (null on Windows testers / old
        // kernels) — a null column is omitted, mirroring the tester's
        // skip_serializing_if on the live path.
        var withTcp = SampleAttempt() with
        {
            Tcp = new AttemptTcpView(2.5, "93.184.216.34:443", null, null, null, null, null, null, null, null),
        };
        var tcp = JsonNode.Parse(JsonSerializer.Serialize(withTcp, WebOptions))!
            .AsObject()["tcp"]!.AsObject();

        Assert.Equal(new[] { "connect_duration_ms", "remote_addr" }, tcp.Select(p => p.Key).ToArray());
    }
}
