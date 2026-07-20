using System.Text.Json;
using Networker.ControlPlane.Reports;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Formula + family-map tests for the perf-per-cost report. The value scores
/// are the product surface — a wrong formula here means the report lies about
/// provider economics, so each formula is pinned with hand-computed numbers.
/// </summary>
public sealed class PerfPerCostLogicTests
{
    // ── latency_cost_index = p95_ms × hourly_usd (lower better) ─────────────

    [Fact]
    public void Latency_cost_index_is_p95_times_hourly_rate()
    {
        // 120 ms p95 on a $0.0416/hr VM → 120 × 0.0416 = 4.992
        Assert.Equal(4.992, PerfPerCostLogic.LatencyCostIndex(120.0, 0.0416m));
    }

    [Fact]
    public void Latency_cost_index_prefers_cheap_vm_at_equal_p95()
    {
        var cheap = PerfPerCostLogic.LatencyCostIndex(100.0, 0.0104m);
        var pricey = PerfPerCostLogic.LatencyCostIndex(100.0, 0.096m);
        Assert.True(cheap < pricey); // lower is better
    }

    [Fact]
    public void Latency_cost_index_breaks_even_when_double_price_halves_p95()
    {
        var slowCheap = PerfPerCostLogic.LatencyCostIndex(200.0, 0.05m);
        var fastPricey = PerfPerCostLogic.LatencyCostIndex(100.0, 0.10m);
        Assert.Equal(slowCheap, fastPricey);
    }

    [Theory]
    [InlineData(null, "0.05")]   // no perf data
    [InlineData(120.0, null)]    // no cost row (missing SKU)
    [InlineData(120.0, "0")]     // zero cost is not a valid divisor/weight
    [InlineData(-1.0, "0.05")]   // negative latency is corrupt data
    public void Latency_cost_index_is_null_when_inputs_are_unusable(double? p95, string? rate)
    {
        decimal? hourly = rate is null
            ? null
            : decimal.Parse(rate, System.Globalization.CultureInfo.InvariantCulture);
        Assert.Null(PerfPerCostLogic.LatencyCostIndex(p95, hourly));
    }

    // ── mbps_per_dollar_hour = median_mbps ÷ hourly_usd (higher better) ─────

    [Fact]
    public void Mbps_per_dollar_hour_is_median_over_hourly_rate()
    {
        // 940 Mbps on a $0.096/hr VM → 940 / 0.096 = 9791.6667
        Assert.Equal(9791.6667, PerfPerCostLogic.MbpsPerDollarHour(940.0, 0.096m));
    }

    [Fact]
    public void Mbps_per_dollar_hour_prefers_cheap_vm_at_equal_throughput()
    {
        var cheap = PerfPerCostLogic.MbpsPerDollarHour(500.0, 0.0104m);
        var pricey = PerfPerCostLogic.MbpsPerDollarHour(500.0, 0.384m);
        Assert.True(cheap > pricey); // higher is better
    }

    [Theory]
    [InlineData(null, "0.05")]
    [InlineData(500.0, null)]
    [InlineData(500.0, "0")]
    [InlineData(-5.0, "0.05")]
    public void Mbps_per_dollar_hour_is_null_when_inputs_are_unusable(double? mbps, string? rate)
    {
        decimal? hourly = rate is null
            ? null
            : decimal.Parse(rate, System.Globalization.CultureInfo.InvariantCulture);
        Assert.Null(PerfPerCostLogic.MbpsPerDollarHour(mbps, hourly));
    }

    // ── family classification ───────────────────────────────────────────────

    [Fact]
    public void Only_thru_is_a_throughput_family()
    {
        Assert.False(PerfPerCostLogic.IsLatencyFamily(PerfPerCostLogic.FamilyThru));
        Assert.True(PerfPerCostLogic.IsLatencyFamily(PerfPerCostLogic.FamilyNet));
        Assert.True(PerfPerCostLogic.IsLatencyFamily(PerfPerCostLogic.FamilyHttp));
        Assert.True(PerfPerCostLogic.IsLatencyFamily(PerfPerCostLogic.FamilyPage));
    }

    /// <summary>
    /// Drift guard: the C# protocol→family map must be exactly the tester-level
    /// entries of <c>shared/modes.json</c> (the canonical manifest — same file
    /// the Rust enum and TS chips are guarded against).
    /// </summary>
    [Fact]
    public void Family_map_matches_shared_modes_manifest()
    {
        using var doc = JsonDocument.Parse(File.ReadAllText(
            Path.Combine(AppContext.BaseDirectory, "shared", "modes.json")));

        var manifest = doc.RootElement.GetProperty("modes").EnumerateArray()
            .Where(m => m.GetProperty("level").GetString() == "tester")
            .ToDictionary(
                m => m.GetProperty("id").GetString()!,
                m => m.GetProperty("family").GetString()!);

        Assert.Equal(
            manifest.OrderBy(kv => kv.Key, StringComparer.Ordinal),
            PerfPerCostLogic.ProtocolFamily
                .ToDictionary(kv => kv.Key, kv => kv.Value)
                .OrderBy(kv => kv.Key, StringComparer.Ordinal));
    }

    [Fact]
    public void Family_case_sql_covers_every_family_and_defaults_to_net()
    {
        var sql = PerfPerCostLogic.FamilyCaseSql("a.Protocol");

        Assert.StartsWith("CASE ", sql);
        Assert.EndsWith($"ELSE '{PerfPerCostLogic.FamilyNet}' END", sql);
        foreach (var family in new[] { "net", "http", "page", "thru" })
        {
            Assert.Contains($"THEN '{family}'", sql);
        }
        // Spot-check a member of each family lands in the right arm.
        Assert.Contains("'udp'", sql);
        Assert.Contains("'http3'", sql);
        Assert.Contains("'browser2'", sql);
        Assert.Contains("'udpupload'", sql);
    }
}
