using System.Text.Json;
using Networker.Agent;

namespace Networker.Agent.Tests;

/// <summary>
/// Per-case artifact synthesis (#796): the accumulator that turns observed
/// attempts into the artifact's <c>cases</c>/<c>summaries</c>/<c>data_quality</c>
/// sections. The summaries shape must stay parseable by the control plane's
/// <c>RegressionAnalyzer.ParseSummaries</c> (case_id, metric_unit,
/// higher_is_better, p50, success_count, failure_count,
/// included_sample_count, ci95_*) — the cross-layer contract is pinned in
/// <c>tests/Networker.Tests/ApibenchArtifactPipelineTests.cs</c>.
/// </summary>
public class BenchmarkArtifactBuilderTests
{
    private static JsonElement Attempt(string json) =>
        JsonDocument.Parse(json).RootElement.Clone();

    private static JsonElement HttpAttempt(double ms, bool success = true) =>
        Attempt("{\"attempt_id\":\"a\",\"protocol\":\"http1\",\"success\":"
            + (success ? "true" : "false")
            + ",\"http\":{\"total_duration_ms\":"
            + ms.ToString(System.Globalization.CultureInfo.InvariantCulture)
            + "}}");

    [Fact]
    public void Workload_name_is_the_case_id_for_apibench_invocations()
    {
        var b = new BenchmarkArtifactBuilder();
        b.Record("api-users", HttpAttempt(10.0));
        b.Record("api-search", HttpAttempt(20.0));

        using var cases = JsonDocument.Parse(b.BuildCasesJson());
        var ids = cases.RootElement.EnumerateArray()
            .Select(c => c.GetProperty("id").GetString()).ToArray();
        Assert.Equal(new[] { "api-search", "api-users" }, ids); // ordinal order
    }

    [Fact]
    public void Base_invocations_use_the_tester_case_id_convention()
    {
        // Mirrors benchmark_case_id: "{protocol}:{payload|default}:{stack|default}".
        var b = new BenchmarkArtifactBuilder();
        b.Record(null, HttpAttempt(10.0));
        b.Record(null, Attempt("""
            {"protocol":"download","success":true,"http_stack":"nginx",
             "http":{"total_duration_ms":5.0,"payload_bytes":65536,"throughput_mbps":50.0}}
            """));

        using var cases = JsonDocument.Parse(b.BuildCasesJson());
        var ids = cases.RootElement.EnumerateArray()
            .Select(c => c.GetProperty("id").GetString()).ToArray();
        Assert.Contains("http1:default:default", ids);
        Assert.Contains("download:65536:nginx", ids);
    }

    [Fact]
    public void Throughput_cases_are_higher_is_better_in_mbps()
    {
        var b = new BenchmarkArtifactBuilder();
        b.Record(null, Attempt("""
            {"protocol":"download","success":true,
             "http":{"total_duration_ms":5.0,"payload_bytes":65536,"throughput_mbps":50.0}}
            """));

        using var summaries = JsonDocument.Parse(b.BuildSummariesJson());
        var s = Assert.Single(summaries.RootElement.EnumerateArray());
        Assert.True(s.GetProperty("higher_is_better").GetBoolean());
        Assert.Equal("MB/s", s.GetProperty("metric_unit").GetString());
        Assert.Equal(50.0, s.GetProperty("p50").GetDouble());
    }

    [Fact]
    public void Failures_count_but_never_enter_the_distribution()
    {
        var b = new BenchmarkArtifactBuilder();
        b.Record("api-users", HttpAttempt(10.0));
        b.Record("api-users", HttpAttempt(999.0, success: false)); // failed → excluded
        b.Record("api-users", HttpAttempt(20.0));

        using var summaries = JsonDocument.Parse(b.BuildSummariesJson());
        var s = Assert.Single(summaries.RootElement.EnumerateArray());
        Assert.Equal(2, s.GetProperty("success_count").GetInt64());
        Assert.Equal(1, s.GetProperty("failure_count").GetInt64());
        Assert.Equal(3, s.GetProperty("sample_count").GetInt64());
        Assert.Equal(2, s.GetProperty("included_sample_count").GetInt64());
        Assert.Equal(1, s.GetProperty("excluded_sample_count").GetInt64());
        Assert.Equal(15.0, s.GetProperty("p50").GetDouble()); // 999 kept out
        Assert.Equal(20.0, s.GetProperty("max").GetDouble());
    }

    [Fact]
    public void A_lost_udp_attempt_contributes_no_sentinel_zero_sample()
    {
        // rtt_avg_ms is a 0.0 sentinel when every echo was lost (trust audit
        // V11) — same exclusion the tester applies.
        var b = new BenchmarkArtifactBuilder();
        b.Record(null, Attempt("""
            {"protocol":"udp","success":true,
             "udp":{"success_count":0,"rtt_avg_ms":0.0}}
            """));
        b.Record(null, Attempt("""
            {"protocol":"udp","success":true,
             "udp":{"success_count":5,"rtt_avg_ms":7.5}}
            """));

        using var summaries = JsonDocument.Parse(b.BuildSummariesJson());
        var s = Assert.Single(summaries.RootElement.EnumerateArray());
        Assert.Equal(2, s.GetProperty("success_count").GetInt64());
        Assert.Equal(1, s.GetProperty("included_sample_count").GetInt64());
        Assert.Equal(7.5, s.GetProperty("p50").GetDouble());
    }

    [Fact]
    public void Ci95_stays_a_zero_placeholder_below_ten_samples()
    {
        // The regression analyzer treats an all-zero CI as unusable and keeps
        // its pure-threshold path — small runs must not fabricate confidence.
        var b = new BenchmarkArtifactBuilder();
        for (var i = 0; i < BenchmarkArtifactBuilder.MinSamplesForCi - 1; i++)
            b.Record("api-users", HttpAttempt(10.0 + i));

        using var summaries = JsonDocument.Parse(b.BuildSummariesJson());
        var s = Assert.Single(summaries.RootElement.EnumerateArray());
        Assert.Equal(0.0, s.GetProperty("ci95_lower").GetDouble());
        Assert.Equal(0.0, s.GetProperty("ci95_upper").GetDouble());
    }

    [Fact]
    public void Ci95_brackets_the_median_at_ten_or_more_samples()
    {
        var b = new BenchmarkArtifactBuilder();
        for (var i = 1; i <= 20; i++)
            b.Record("api-users", HttpAttempt(i));

        using var summaries = JsonDocument.Parse(b.BuildSummariesJson());
        var s = Assert.Single(summaries.RootElement.EnumerateArray());
        var p50 = s.GetProperty("p50").GetDouble();
        var lo = s.GetProperty("ci95_lower").GetDouble();
        var hi = s.GetProperty("ci95_upper").GetDouble();
        Assert.True(lo < hi, $"CI [{lo}, {hi}] must be a proper interval");
        Assert.InRange(p50, lo, hi);
    }

    [Theory]
    [InlineData(0.50, 15.0)]
    [InlineData(0.95, 19.5)] // linear interpolation: 10 + 0.95*(20-10)
    [InlineData(0.0, 10.0)]
    [InlineData(1.0, 20.0)]
    public void Percentile_interpolates_linearly(double q, double expected)
    {
        Assert.Equal(expected, BenchmarkArtifactBuilder.Percentile(new[] { 10.0, 20.0 }, q), 10);
    }

    [Fact]
    public void Data_quality_uses_the_artifact_sufficiency_vocabulary()
    {
        // adequate / marginal / insufficient (json.rs vocabulary — the
        // dashboard colors on exactly these strings).
        var empty = new BenchmarkArtifactBuilder();
        using (var dq = JsonDocument.Parse(empty.BuildDataQualityJson()))
        {
            Assert.Equal("insufficient", dq.RootElement.GetProperty("sufficiency").GetString());
            Assert.False(dq.RootElement.GetProperty("publication_ready").GetBoolean());
        }

        var small = new BenchmarkArtifactBuilder();
        small.Record("api-users", HttpAttempt(10.0));
        using (var dq = JsonDocument.Parse(small.BuildDataQualityJson()))
        {
            Assert.Equal("marginal", dq.RootElement.GetProperty("sufficiency").GetString());
        }

        var sampled = new BenchmarkArtifactBuilder();
        for (var i = 0; i < BenchmarkArtifactBuilder.MinSamplesForCi; i++)
            sampled.Record("api-users", HttpAttempt(10.0 + i));
        using (var dq = JsonDocument.Parse(sampled.BuildDataQualityJson()))
        {
            Assert.Equal("adequate", dq.RootElement.GetProperty("sufficiency").GetString());
        }
    }

    [Fact]
    public void Empty_builder_emits_empty_arrays_not_nulls()
    {
        var b = new BenchmarkArtifactBuilder();
        Assert.Equal("[]", b.BuildCasesJson());
        Assert.Equal("[]", b.BuildSummariesJson());
        Assert.False(b.HasCases);
    }
}
