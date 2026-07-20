using System.Text.Json;
using System.Text.Json.Nodes;
using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pins the wire shape of <c>GET /api/projects/{id}/reports/perf-per-cost</c>
/// — snake_case field set the frontend <c>PerfPerCostReport</c> type consumes,
/// including the self-documenting <c>formulas</c> block and the honesty
/// fields (<c>cost_note</c>, <c>missing_cost_skus</c>, nullable
/// <c>hourly_usd</c>/<c>value_score</c>).
/// </summary>
public sealed class PerfPerCostContractTests
{
    private static readonly JsonSerializerOptions WebOptions =
        new(JsonSerializerDefaults.Web);

    private static PerfPerCostReport SampleReport() => new(
        GeneratedAt: new DateTime(2026, 7, 20, 12, 0, 0, DateTimeKind.Utc),
        CostTable: new CostTableInfo("2026-07-20", "list prices only", "shared/cloud-costs.json"),
        Formulas: new FormulasInfo("latency formula", "throughput formula"),
        CompletedRuns: 12,
        ProvidersWithData: 2,
        Groups:
        [
            new PerfPerCostGroup(
                Provider: "azure",
                VmSize: "Standard_B2s",
                Region: "eastus",
                HourlyUsd: 0.0416m,
                CostRegion: "eastus",
                CostSourceUrl: "https://prices.azure.com/x",
                CostAsOf: "2026-07-20",
                CostNote: null,
                Families:
                [
                    new PerfPerCostFamily(
                        Family: "http", RunCount: 4, SampleCount: 200,
                        MetricLabel: "latency_ms", Median: 42.1, P95Ms: 120.0,
                        ValueMetric: "latency_cost_index", ValueScore: 4.992),
                ]),
            new PerfPerCostGroup(
                Provider: "aws",
                VmSize: "t4g.exotic",
                Region: "us-east-1",
                HourlyUsd: null,
                CostRegion: null,
                CostSourceUrl: null,
                CostAsOf: null,
                CostNote: "no price row for this SKU",
                Families:
                [
                    new PerfPerCostFamily(
                        Family: "thru", RunCount: 1, SampleCount: 30,
                        MetricLabel: "throughput_mbps", Median: 940.0, P95Ms: null,
                        ValueMetric: "mbps_per_dollar_hour", ValueScore: null),
                ]),
        ],
        MissingCostSkus: [new MissingCostSku("aws", "t4g.exotic", "us-east-1")]);

    [Fact]
    public void Report_emits_the_exact_top_level_snake_case_field_set()
    {
        var json = JsonSerializer.Serialize(SampleReport(), WebOptions);
        var root = JsonNode.Parse(json)!.AsObject();

        Assert.Equal(
            new[]
            {
                "generated_at", "cost_table", "formulas", "completed_runs",
                "providers_with_data", "groups", "missing_cost_skus",
            },
            root.Select(kv => kv.Key).ToArray());
        Assert.Equal(
            new[] { "as_of", "disclaimer", "source" },
            root["cost_table"]!.AsObject().Select(kv => kv.Key).ToArray());
        Assert.Equal(
            new[] { "latency_cost_index", "mbps_per_dollar_hour" },
            root["formulas"]!.AsObject().Select(kv => kv.Key).ToArray());
    }

    [Fact]
    public void Group_and_family_rows_emit_the_exact_snake_case_field_set()
    {
        var json = JsonSerializer.Serialize(SampleReport(), WebOptions);
        var group = JsonNode.Parse(json)!["groups"]!.AsArray()[0]!.AsObject();

        Assert.Equal(
            new[]
            {
                "provider", "vm_size", "region", "hourly_usd", "cost_region",
                "cost_source_url", "cost_as_of", "cost_note", "families",
            },
            group.Select(kv => kv.Key).ToArray());

        var family = group["families"]!.AsArray()[0]!.AsObject();
        Assert.Equal(
            new[]
            {
                "family", "run_count", "sample_count", "metric_label",
                "median", "p95_ms", "value_metric", "value_score",
            },
            family.Select(kv => kv.Key).ToArray());
    }

    [Fact]
    public void Missing_cost_sku_rows_survive_with_null_cost_never_dropped()
    {
        var json = JsonSerializer.Serialize(SampleReport(), WebOptions);
        var root = JsonNode.Parse(json)!.AsObject();

        // The unpriced AWS group is still in `groups` (perf shown, cost null)...
        var aws = root["groups"]!.AsArray()[1]!.AsObject();
        Assert.True(aws.ContainsKey("hourly_usd"));
        Assert.Null(aws["hourly_usd"]); // JSON null, not omitted
        Assert.Contains("no price row", aws["cost_note"]!.GetValue<string>());
        Assert.Null(aws["families"]!.AsArray()[0]!["value_score"]);

        // ...and surfaced in the missing list for the UI banner.
        var missing = root["missing_cost_skus"]!.AsArray();
        Assert.Single(missing);
        Assert.Equal("t4g.exotic", missing[0]!["vm_size"]!.GetValue<string>());
    }
}
