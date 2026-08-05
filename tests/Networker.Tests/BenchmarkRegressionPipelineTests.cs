using System.Net;
using System.Net.Http.Json;
using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Networker.ControlPlane.Provisioning;
using Networker.Data.Entities;
using Xunit;

namespace Networker.Tests;

/// <summary>
/// Audit P1-9: <c>BenchmarkRegressionDetector</c> is registered in DI and
/// invoked opportunistically on run completion, but nothing ever proved the
/// PIPELINE works — its only test used SQLite, and no workflow or test ever
/// asserted that a genuinely slower run produces a persisted regression that
/// surfaces on the API.
///
/// <para>This ingests a baseline run and then a materially slower one through
/// the real detector against real Postgres, and requires the regression to be
/// both persisted and visible on
/// <c>GET /api/projects/{id}/benchmark-regressions</c>.</para>
/// </summary>
public class BenchmarkRegressionPipelineTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fx;

    public BenchmarkRegressionPipelineTests(ControlPlaneFixture fx) => _fx = fx;

    private const string Pid = ControlPlaneFixture.SeededProjectId;

    /// <summary>Summaries JSON in the shape RegressionAnalyzer.ParseSummaries
    /// reads (verified against the analyzer): case_id + p50 + counts.</summary>
    private static string Summaries(double p50) => $$"""
        [{"case_id":"api-users","metric_unit":"ms","higher_is_better":false,
          "p50":{{p50.ToString(System.Globalization.CultureInfo.InvariantCulture)}},
          "success_count":100,"failure_count":0}]
        """;

    private async Task<Guid> SeedRunWithSummariesAsync(Guid configId, double p50, DateTime finishedAt)
    {
        await using var db = _fx.NewDbContext();
        var artifactId = Guid.NewGuid();
        var runId = Guid.NewGuid();
        // Real entity (verified in Networker.Data.Entities): BenchmarkArtifact,
        // keyed by Id and linked to the run by TestRunId; every jsonb column is
        // NOT NULL so they all need a value.
        db.BenchmarkArtifacts.Add(new BenchmarkArtifact
        {
            Id = artifactId,
            TestRunId = runId,
            Environment = "{}",
            Methodology = "{}",
            Launches = "[]",
            Cases = "[]",
            Summaries = Summaries(p50),
            DataQuality = "{}",
        });
        db.TestRuns.Add(new TestRun
        {
            Id = runId,
            TestConfigId = configId,
            ProjectId = Pid,
            Status = "completed",
            ArtifactId = artifactId,
            SuccessCount = 100,
            FailureCount = 0,
            StartedAt = finishedAt.AddMinutes(-5),
            FinishedAt = finishedAt,
            CreatedAt = finishedAt.AddMinutes(-6),
        });
        await db.SaveChangesAsync();
        return runId;
    }

    private async Task<Guid> SeedBenchmarkConfigAsync()
    {
        var cfgId = Guid.NewGuid();
        var now = DateTime.UtcNow;
        await using var db = _fx.NewDbContext();
        db.TestConfigs.Add(new TestConfig
        {
            Id = cfgId,
            ProjectId = Pid,
            Name = $"regr-{cfgId:N}",
            EndpointKind = "network",
            EndpointRef = """{"kind":"network","host":"10.0.0.5","port":8444}""",
            Workload = """{"modes":["apibench"],"runs":10}""",
            MaxDurationSecs = 600,
            CreatedAt = now.AddHours(-2),
            UpdatedAt = now.AddHours(-2),
        });
        await db.SaveChangesAsync();
        return cfgId;
    }

    [Fact]
    public async Task A_materially_slower_run_is_detected_persisted_and_served()
    {
        var cfgId = await SeedBenchmarkConfigAsync();
        var now = DateTime.UtcNow;

        // Baseline: 100ms p50. Detector must find no baseline for it (first run).
        var baselineRunId = await SeedRunWithSummariesAsync(cfgId, 100.0, now.AddHours(-1));
        using (var scope = _fx.Services.CreateScope())
        {
            var det = scope.ServiceProvider.GetRequiredService<BenchmarkRegressionDetector>();
            await det.DetectAsync(baselineRunId);
        }

        // Regressed: 160ms p50 — a 60% slowdown, far past the >10% policy.
        var slowRunId = await SeedRunWithSummariesAsync(cfgId, 160.0, now);
        using (var scope = _fx.Services.CreateScope())
        {
            var det = scope.ServiceProvider.GetRequiredService<BenchmarkRegressionDetector>();
            await det.DetectAsync(slowRunId);
        }

        // ── persisted ─────────────────────────────────────────────────────
        await using (var db = _fx.NewDbContext())
        {
            var rows = await db.BenchmarkRegressions.AsNoTracking()
                .Where(r => r.TestRunId == slowRunId)
                .ToListAsync();
            Assert.True(rows.Count > 0,
                "a 60% p50 slowdown produced NO benchmark_regression row — the detector pipeline is not working");
        }

        // ── and visible on the API ────────────────────────────────────────
        using var client = _fx.CreateAuthenticatedClient();
        var resp = await client.GetAsync($"/api/projects/{Pid}/benchmark-regressions?limit=20");
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        var body = await resp.Content.ReadAsStringAsync();
        Assert.Contains(slowRunId.ToString(), body, StringComparison.OrdinalIgnoreCase);
    }

    [Fact]
    public async Task An_equivalent_run_produces_no_regression()
    {
        // The false-positive direction: noise must not manufacture alerts.
        var cfgId = await SeedBenchmarkConfigAsync();
        var now = DateTime.UtcNow;

        var baselineRunId = await SeedRunWithSummariesAsync(cfgId, 100.0, now.AddHours(-1));
        using (var scope = _fx.Services.CreateScope())
        {
            await scope.ServiceProvider.GetRequiredService<BenchmarkRegressionDetector>()
                .DetectAsync(baselineRunId);
        }

        var sameRunId = await SeedRunWithSummariesAsync(cfgId, 102.0, now); // +2%
        using (var scope = _fx.Services.CreateScope())
        {
            await scope.ServiceProvider.GetRequiredService<BenchmarkRegressionDetector>()
                .DetectAsync(sameRunId);
        }

        await using var db = _fx.NewDbContext();
        var rows = await db.BenchmarkRegressions.AsNoTracking()
            .CountAsync(r => r.TestRunId == sameRunId);
        Assert.Equal(0, rows);
    }

    [Fact]
    public async Task Detection_is_idempotent_for_the_same_run()
    {
        var cfgId = await SeedBenchmarkConfigAsync();
        var now = DateTime.UtcNow;
        var baselineRunId = await SeedRunWithSummariesAsync(cfgId, 100.0, now.AddHours(-1));
        var slowRunId = await SeedRunWithSummariesAsync(cfgId, 200.0, now);

        int afterFirst, afterSecond;
        using (var scope = _fx.Services.CreateScope())
        {
            var det = scope.ServiceProvider.GetRequiredService<BenchmarkRegressionDetector>();
            await det.DetectAsync(baselineRunId);
            await det.DetectAsync(slowRunId);
        }
        await using (var db = _fx.NewDbContext())
        {
            afterFirst = await db.BenchmarkRegressions.AsNoTracking()
                .CountAsync(r => r.TestRunId == slowRunId);
        }

        // Re-delivery (the agent can report completion more than once) must be
        // a no-op — the detector early-returns when rows already exist.
        using (var scope = _fx.Services.CreateScope())
        {
            await scope.ServiceProvider.GetRequiredService<BenchmarkRegressionDetector>()
                .DetectAsync(slowRunId);
        }
        await using (var db = _fx.NewDbContext())
        {
            afterSecond = await db.BenchmarkRegressions.AsNoTracking()
                .CountAsync(r => r.TestRunId == slowRunId);
        }

        Assert.True(afterFirst > 0, "a 100% slowdown produced no regression row");
        Assert.Equal(afterFirst, afterSecond);
    }
}
