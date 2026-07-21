using System.Text.Json;
using System.Text.Json.Nodes;
using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pins the wire shape of <c>GET /api/projects/{id}/reports/app-network</c> —
/// the snake_case field set the frontend consumes, including the self-documenting
/// <c>formulas</c> block, the overall verdict/summary fields, and the per-group
/// split fields (nullable medians, verdict, main_issue).
/// </summary>
public sealed class AppNetworkContractTests
{
    private static readonly JsonSerializerOptions WebOptions = new(JsonSerializerDefaults.Web);

    private static AppNetworkReport SampleReport() => new(
        GeneratedAt: new DateTime(2026, 7, 20, 12, 0, 0, DateTimeKind.Utc),
        Formulas: new AppNetworkFormulas("server formula", "network formula", "split rule", "anomaly rule"),
        Mode: "sdkprobe",
        AttemptCount: 40,
        SplitAnomalyCount: 1,
        OverallVerdict: "server_bound",
        OverallMainIssue: "Server processing dominates ...",
        OverallMedianServerMs: 180.0,
        OverallMedianNetworkMs: 40.0,
        OverallMedianWallMs: 220.0,
        OverallServerRatio: 0.8182,
        Groups:
        [
            new AppNetworkGroup(
                ConfigId: Guid.Parse("22222222-2222-4222-8222-222222222222"),
                ConfigName: "checkout-api",
                RunCount: 4,
                AttemptCount: 40,
                SplitAnomalyCount: 1,
                MedianServerMs: 180.0,
                P95ServerMs: 240.0,
                MedianNetworkMs: 40.0,
                P95NetworkMs: 60.0,
                MedianWallMs: 220.0,
                ServerRatio: 0.8182,
                Verdict: "server_bound",
                MainIssue: "Server processing dominates ..."),
        ]);

    [Fact]
    public void Report_emits_the_exact_top_level_snake_case_field_set()
    {
        var json = JsonSerializer.Serialize(SampleReport(), WebOptions);
        var root = JsonNode.Parse(json)!.AsObject();

        Assert.Equal(
            new[]
            {
                "generated_at", "formulas", "mode", "attempt_count",
                "split_anomaly_count", "overall_verdict", "overall_main_issue",
                "overall_median_server_ms", "overall_median_network_ms",
                "overall_median_wall_ms", "overall_server_ratio", "groups",
            },
            root.Select(kv => kv.Key).ToArray());

        Assert.Equal(
            new[] { "server_ms", "network_ms", "split", "split_anomaly" },
            root["formulas"]!.AsObject().Select(kv => kv.Key).ToArray());
    }

    [Fact]
    public void Group_rows_emit_the_exact_snake_case_field_set()
    {
        var json = JsonSerializer.Serialize(SampleReport(), WebOptions);
        var group = JsonNode.Parse(json)!["groups"]!.AsArray()[0]!.AsObject();

        Assert.Equal(
            new[]
            {
                "config_id", "config_name", "run_count", "attempt_count",
                "split_anomaly_count", "median_server_ms", "p95_server_ms",
                "median_network_ms", "p95_network_ms", "median_wall_ms",
                "server_ratio", "verdict", "main_issue",
            },
            group.Select(kv => kv.Key).ToArray());
    }

    [Fact]
    public void Null_medians_are_emitted_as_json_null_not_omitted()
    {
        var empty = SampleReport() with
        {
            OverallMedianServerMs = null,
            OverallServerRatio = null,
        };
        var json = JsonSerializer.Serialize(empty, WebOptions);
        var root = JsonNode.Parse(json)!.AsObject();

        Assert.True(root.ContainsKey("overall_median_server_ms"));
        Assert.Null(root["overall_median_server_ms"]);
        Assert.Null(root["overall_server_ratio"]);
    }
}
