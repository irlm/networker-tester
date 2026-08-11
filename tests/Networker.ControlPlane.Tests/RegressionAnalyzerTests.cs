using System.Text.Json;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Provisioning;
using Networker.ControlPlane.Realtime;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Tests for the benchmark regression policy (deep-measurement audit
/// M5/A12/G2): each completed run's per-case p50 and success rate are compared
/// against the baseline run — p50 worse by &gt;10% (in the case's metric
/// direction) or success rate below 99% flags a regression; both checks are
/// guarded by a minimum sample count so noise-level runs are never flagged.
/// Also covers the <see cref="BenchmarkRegression"/> event emission seam.
/// </summary>
public sealed class RegressionAnalyzerTests
{
    private static CaseStats Latency(
        string caseId, double p50,
        long success = 100, long failure = 0, long included = 100) =>
        new(caseId, "ms", HigherIsBetter: false, p50, success, failure, included);

    private static CaseStats Throughput(
        string caseId, double p50,
        long success = 100, long failure = 0, long included = 100) =>
        new(caseId, "MB/s", HigherIsBetter: true, p50, success, failure, included);

    // ── p50 policy ───────────────────────────────────────────────────────────

    [Fact]
    public void P50_increase_over_10_percent_flags_latency_regression()
    {
        var result = RegressionAnalyzer.Detect(
            [Latency("http1-1k", 11.5)],
            [Latency("http1-1k", 10.0)]);

        var reg = Assert.Single(result);
        Assert.Equal("http1-1k", reg.CaseId);
        Assert.Equal(RegressionAnalyzer.MetricP50LatencyMs, reg.Metric);
        Assert.Equal("ms", reg.MetricUnit);
        Assert.Equal(10.0, reg.BaselineValue);
        Assert.Equal(11.5, reg.CurrentValue);
        Assert.Equal(15.0, reg.DeltaPercent, 6);
        Assert.Equal(RegressionAnalyzer.SeverityWarning, reg.Severity);
    }

    [Fact]
    public void P50_increase_within_10_percent_is_not_flagged()
    {
        var result = RegressionAnalyzer.Detect(
            [Latency("http1-1k", 10.9)],
            [Latency("http1-1k", 10.0)]);

        Assert.Empty(result);
    }

    [Fact]
    public void P50_improvement_is_never_flagged()
    {
        var result = RegressionAnalyzer.Detect(
            [Latency("http1-1k", 5.0)],
            [Latency("http1-1k", 10.0)]);

        Assert.Empty(result);
    }

    [Fact]
    public void P50_increase_over_25_percent_is_critical()
    {
        var result = RegressionAnalyzer.Detect(
            [Latency("http1-1k", 14.0)],
            [Latency("http1-1k", 10.0)]);

        Assert.Equal(RegressionAnalyzer.SeverityCritical, Assert.Single(result).Severity);
    }

    [Fact]
    public void Throughput_drop_over_10_percent_flags_in_the_higher_is_better_direction()
    {
        // For higher_is_better cases an INCREASE is fine; a >10% DROP regresses.
        var noFlag = RegressionAnalyzer.Detect(
            [Throughput("tp-1m", 120.0)],
            [Throughput("tp-1m", 100.0)]);
        Assert.Empty(noFlag);

        var flagged = RegressionAnalyzer.Detect(
            [Throughput("tp-1m", 85.0)],
            [Throughput("tp-1m", 100.0)]);

        var reg = Assert.Single(flagged);
        Assert.Equal(RegressionAnalyzer.MetricP50, reg.Metric);
        Assert.Equal("MB/s", reg.MetricUnit);
        Assert.Equal(-15.0, reg.DeltaPercent, 6);
    }

    // ── baseline / small-n guards ────────────────────────────────────────────

    [Fact]
    public void First_run_with_no_baseline_flags_nothing()
    {
        var result = RegressionAnalyzer.Detect(
            [Latency("http1-1k", 1000.0, success: 50, failure: 50)],
            baseline: []);

        Assert.Empty(result);
    }

    [Fact]
    public void Case_missing_from_baseline_is_skipped()
    {
        var result = RegressionAnalyzer.Detect(
            [Latency("new-case", 1000.0)],
            [Latency("other-case", 10.0)]);

        Assert.Empty(result);
    }

    [Fact]
    public void Small_sample_runs_are_never_flagged()
    {
        // A 3-sample run tripling its p50 (and failing 1 of 3 attempts) is
        // noise-level: below MinSamples nothing may fire (M5 §A1/§A3).
        var result = RegressionAnalyzer.Detect(
            [Latency("http1-1k", 30.0, success: 2, failure: 1, included: 3)],
            [Latency("http1-1k", 10.0, success: 3, failure: 0, included: 3)]);

        Assert.Empty(result);
    }

    [Fact]
    public void Small_baseline_sample_count_also_blocks_the_p50_check()
    {
        var result = RegressionAnalyzer.Detect(
            [Latency("http1-1k", 30.0, included: 100)],
            [Latency("http1-1k", 10.0, included: RegressionAnalyzer.MinSamples - 1)]);

        Assert.Empty(result);
    }

    [Fact]
    public void Zero_baseline_p50_is_skipped()
    {
        var result = RegressionAnalyzer.Detect(
            [Latency("http1-1k", 30.0)],
            [Latency("http1-1k", 0.0)]);

        Assert.Empty(result);
    }

    // ── success-rate policy ──────────────────────────────────────────────────

    [Fact]
    public void Success_rate_below_99_percent_is_flagged()
    {
        // 97/100 = 97% < 99% floor; p50 unchanged so only the rate fires.
        var result = RegressionAnalyzer.Detect(
            [Latency("http1-1k", 10.0, success: 97, failure: 3)],
            [Latency("http1-1k", 10.0, success: 100, failure: 0)]);

        var reg = Assert.Single(result);
        Assert.Equal(RegressionAnalyzer.MetricSuccessRate, reg.Metric);
        Assert.Equal("%", reg.MetricUnit);
        Assert.Equal(100.0, reg.BaselineValue, 6);
        Assert.Equal(97.0, reg.CurrentValue, 6);
        Assert.Equal(-3.0, reg.DeltaPercent, 6);
        Assert.Equal(RegressionAnalyzer.SeverityWarning, reg.Severity);
    }

    [Fact]
    public void Success_rate_below_95_percent_is_critical()
    {
        var result = RegressionAnalyzer.Detect(
            [Latency("http1-1k", 10.0, success: 90, failure: 10)],
            [Latency("http1-1k", 10.0, success: 100, failure: 0)]);

        Assert.Equal(RegressionAnalyzer.SeverityCritical, Assert.Single(result).Severity);
    }

    [Fact]
    public void Success_rate_at_or_above_99_percent_is_not_flagged()
    {
        var result = RegressionAnalyzer.Detect(
            [Latency("http1-1k", 10.0, success: 99, failure: 1)],
            [Latency("http1-1k", 10.0, success: 100, failure: 0)]);

        Assert.Empty(result);
    }

    [Fact]
    public void Both_checks_can_fire_for_one_case()
    {
        var result = RegressionAnalyzer.Detect(
            [Latency("http1-1k", 20.0, success: 90, failure: 10)],
            [Latency("http1-1k", 10.0, success: 100, failure: 0)]);

        Assert.Equal(2, result.Count);
        Assert.Contains(result, r => r.Metric == RegressionAnalyzer.MetricP50LatencyMs);
        Assert.Contains(result, r => r.Metric == RegressionAnalyzer.MetricSuccessRate);
    }

    // ── summaries parsing ────────────────────────────────────────────────────

    [Fact]
    public void ParseSummaries_reads_the_tester_artifact_shape()
    {
        // Field subset of the tester's BenchmarkSummary (output/json.rs) —
        // unknown members must be ignored.
        const string json = """
            [{
                "case_id": "http1-1024",
                "protocol": "http1",
                "metric_name": "total_duration_ms",
                "metric_unit": "ms",
                "higher_is_better": false,
                "sample_count": 40,
                "included_sample_count": 32,
                "excluded_sample_count": 8,
                "success_count": 40,
                "failure_count": 0,
                "p50": 12.5,
                "p95": 20.0,
                "mean": 13.1
            }]
            """;

        var stats = Assert.Single(RegressionAnalyzer.ParseSummaries(json));
        Assert.Equal("http1-1024", stats.CaseId);
        Assert.Equal("ms", stats.MetricUnit);
        Assert.False(stats.HigherIsBetter);
        Assert.Equal(12.5, stats.P50);
        Assert.Equal(40, stats.SuccessCount);
        Assert.Equal(0, stats.FailureCount);
        Assert.Equal(32, stats.IncludedSampleCount);
    }

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("not json")]
    [InlineData("{\"an\":\"object\"}")]
    [InlineData("[{\"no_case_id\": true}]")]
    public void ParseSummaries_tolerates_malformed_documents(string? json)
    {
        Assert.Empty(RegressionAnalyzer.ParseSummaries(json));
    }

    // ── CI noise gate ────────────────────────────────────────────────────────

    private static CaseStats LatencyWithCi(
        string caseId, double p50, double ciLo, double ciHi) =>
        new(caseId, "ms", HigherIsBetter: false, p50, 100, 0, 100, ciLo, ciHi);

    [Fact]
    public void P50_swing_with_overlapping_cis_is_suppressed_as_noise()
    {
        // +15% crosses the threshold, but the intervals overlap — one noisy
        // run on a shared box, not a movement.
        var result = RegressionAnalyzer.Detect(
            [LatencyWithCi("http1-1k", 11.5, ciLo: 9.8, ciHi: 13.0)],
            [LatencyWithCi("http1-1k", 10.0, ciLo: 9.0, ciHi: 11.2)]);

        Assert.Empty(result);
    }

    [Fact]
    public void P50_swing_with_disjoint_cis_still_flags()
    {
        var result = RegressionAnalyzer.Detect(
            [LatencyWithCi("http1-1k", 11.5, ciLo: 11.3, ciHi: 11.8)],
            [LatencyWithCi("http1-1k", 10.0, ciLo: 9.8, ciHi: 10.2)]);

        var reg = Assert.Single(result);
        Assert.Equal(15.0, reg.DeltaPercent, 6);
    }

    [Fact]
    public void Throughput_drop_needs_current_ci_entirely_below_baseline_ci()
    {
        // Throughput worsens DOWNWARD: overlapping intervals → suppressed.
        var overlapping = RegressionAnalyzer.Detect(
            [new CaseStats("dl-1m", "MB/s", true, 80.0, 100, 0, 100, 70.0, 95.0)],
            [new CaseStats("dl-1m", "MB/s", true, 100.0, 100, 0, 100, 90.0, 110.0)]);
        Assert.Empty(overlapping);

        var disjoint = RegressionAnalyzer.Detect(
            [new CaseStats("dl-1m", "MB/s", true, 80.0, 100, 0, 100, 78.0, 82.0)],
            [new CaseStats("dl-1m", "MB/s", true, 100.0, 100, 0, 100, 97.0, 103.0)]);
        Assert.Single(disjoint);
    }

    [Fact]
    public void Zero_zero_ci_is_treated_as_absent_and_keeps_threshold_behaviour()
    {
        // Old artifacts serde-default the ci95 fields to 0.0/0.0 — that must
        // read as "no CI", not as an ultra-tight interval at zero.
        var result = RegressionAnalyzer.Detect(
            [LatencyWithCi("http1-1k", 11.5, ciLo: 0.0, ciHi: 0.0)],
            [LatencyWithCi("http1-1k", 10.0, ciLo: 0.0, ciHi: 0.0)]);

        Assert.Single(result);
    }

    [Fact]
    public void ParseSummaries_reads_ci95_bounds()
    {
        var parsed = RegressionAnalyzer.ParseSummaries(
            "[{\"case_id\":\"http1-1k\",\"metric_unit\":\"ms\",\"higher_is_better\":false," +
            "\"p50\":10.0,\"success_count\":100,\"failure_count\":0," +
            "\"included_sample_count\":100,\"ci95_lower\":9.5,\"ci95_upper\":10.5}]");

        var stats = Assert.Single(parsed);
        Assert.Equal(9.5, stats.Ci95Lower);
        Assert.Equal(10.5, stats.Ci95Upper);
    }

    // ── event emission seam ──────────────────────────────────────────────────

    private static EventBus NewBus()
    {
        var services = new ServiceCollection();
        services.AddLogging(b => b.SetMinimumLevel(LogLevel.Warning));
        services.AddSignalR();
        services.AddDashboardEventBus();
        return services.BuildServiceProvider().GetRequiredService<EventBus>();
    }

    [Fact]
    public void EmitRegressionEvent_publishes_benchmark_regression()
    {
        var bus = NewBus();
        var configId = Guid.NewGuid();
        var regressions = RegressionAnalyzer.Detect(
            [Latency("dns", 15.0), Latency("tls", 25.0)],
            [Latency("dns", 10.0), Latency("tls", 20.0)]);
        Assert.Equal(2, regressions.Count);

        var seq = RegressionAnalyzer.EmitRegressionEvent(bus, configId, "my-config", regressions);

        Assert.Equal(1, seq);
        var replayed = bus.Replay(0);
        var evt = Assert.IsType<BenchmarkRegression>(Assert.Single(replayed).Event);
        Assert.Equal(configId, evt.ConfigId);
        Assert.Equal("my-config", evt.ConfigName);
        Assert.Equal(2, evt.RegressionCount);
        // regressions carried verbatim as a JSON array.
        Assert.Equal(JsonValueKind.Array, evt.Regressions.ValueKind);
        Assert.Equal(2, evt.Regressions.GetArrayLength());
    }
}
