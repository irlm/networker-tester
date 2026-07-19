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
}
