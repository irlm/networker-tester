using System.Text.Json;
using System.Text.Json.Nodes;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Endpoints;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Tests for the #810 comparison-activity summary
/// (<see cref="BenchmarkRegressionsEndpoints.ComputeSummaryAsync"/>) and the
/// pin/unpin-baseline actions, against a relational (Sqlite)
/// <see cref="NetworkerDbContext"/> — the same fixture pattern as
/// <see cref="BenchmarkRegressionDetectorTests"/>. The summary is DERIVED from
/// run history (no counter), so these tests pin the exact derivation:
/// runs_compared = Σ max(eligible_runs − 1, 0) per config, where "eligible"
/// is the detector's own gate (completed + artifact).
/// </summary>
public sealed class BenchmarkRegressionSummaryTests : IDisposable
{
    private const string ProjectId = "proj-summary-test";
    private const string OtherProjectId = "proj-other";

    private readonly Microsoft.Data.Sqlite.SqliteConnection _conn;
    private readonly NetworkerDbContext _db;

    public BenchmarkRegressionSummaryTests()
    {
        _conn = new Microsoft.Data.Sqlite.SqliteConnection("DataSource=:memory:");
        _conn.Open();
        CreateMinimalSchema(_conn);

        var options = new DbContextOptionsBuilder<NetworkerDbContext>()
            .UseSqlite(_conn)
            .Options;
        _db = new NetworkerDbContext(options);
    }

    public void Dispose()
    {
        _db.Dispose();
        _conn.Dispose();
    }

    // ── Summary derivation ───────────────────────────────────────────────

    [Fact]
    public async Task Empty_project_reports_zero_activity()
    {
        var s = await BenchmarkRegressionsEndpoints.ComputeSummaryAsync(_db, ProjectId, default);

        Assert.Equal(0, s.ComparableConfigs);
        Assert.Equal(0, s.PinnedBaselineConfigs);
        Assert.Equal(0, s.RunsCompared);
        Assert.Null(s.LastComparisonAt);
        Assert.Equal(0, s.TotalRegressions);
        Assert.Null(s.LastRegressionAt);
        Assert.False(string.IsNullOrWhiteSpace(s.Semantics));
    }

    [Fact]
    public async Task Single_run_config_is_never_compared_and_not_comparable()
    {
        var cfg = SeedConfig("cfg-single");
        SeedRun(cfg, "completed", artifact: true, DateTime.UtcNow);
        await _db.SaveChangesAsync();

        var s = await BenchmarkRegressionsEndpoints.ComputeSummaryAsync(_db, ProjectId, default);

        Assert.Equal(0, s.RunsCompared);
        Assert.Equal(0, s.ComparableConfigs);
        Assert.Null(s.LastComparisonAt);
    }

    [Fact]
    public async Task Repeat_runs_count_n_minus_one_comparisons_and_track_the_newest()
    {
        var cfg = SeedConfig("cfg-repeat");
        var t0 = new DateTime(2026, 8, 1, 10, 0, 0, DateTimeKind.Utc);
        SeedRun(cfg, "completed", artifact: true, t0);
        SeedRun(cfg, "completed", artifact: true, t0.AddHours(1));
        var newest = t0.AddHours(2);
        SeedRun(cfg, "completed", artifact: true, newest);
        await _db.SaveChangesAsync();

        var s = await BenchmarkRegressionsEndpoints.ComputeSummaryAsync(_db, ProjectId, default);

        Assert.Equal(2, s.RunsCompared);
        Assert.Equal(1, s.ComparableConfigs);
        Assert.Equal(newest, s.LastComparisonAt);
    }

    [Fact]
    public async Task Only_completed_artifact_bearing_runs_count()
    {
        // Failed runs, running runs, and completed runs WITHOUT an artifact
        // (plain network tests) are outside the detector's gate.
        var cfg = SeedConfig("cfg-gate");
        SeedRun(cfg, "completed", artifact: true, DateTime.UtcNow.AddHours(-3));
        SeedRun(cfg, "failed", artifact: true, DateTime.UtcNow.AddHours(-2));
        SeedRun(cfg, "running", artifact: false, DateTime.UtcNow.AddHours(-1));
        SeedRun(cfg, "completed", artifact: false, DateTime.UtcNow);
        await _db.SaveChangesAsync();

        var s = await BenchmarkRegressionsEndpoints.ComputeSummaryAsync(_db, ProjectId, default);

        Assert.Equal(0, s.RunsCompared);
        Assert.Equal(0, s.ComparableConfigs);
    }

    [Fact]
    public async Task Pinned_config_with_one_run_is_comparable_for_its_next_run()
    {
        var cfg = SeedConfig("cfg-pinned");
        var run = SeedRun(cfg, "completed", artifact: true, DateTime.UtcNow);
        await _db.SaveChangesAsync();
        await _db.TestConfigs.Where(c => c.Id == cfg)
            .ExecuteUpdateAsync(u => u.SetProperty(c => c.BaselineRunId, run));
        _db.ChangeTracker.Clear(); // ExecuteUpdate bypasses the tracker

        var s = await BenchmarkRegressionsEndpoints.ComputeSummaryAsync(_db, ProjectId, default);

        Assert.Equal(1, s.PinnedBaselineConfigs);
        Assert.Equal(1, s.ComparableConfigs);
        Assert.Equal(0, s.RunsCompared); // nothing compared YET — comparable next run
    }

    [Fact]
    public async Task Regression_rows_are_exact_and_project_scoped()
    {
        var mine = SeedConfig("cfg-mine");
        var theirs = SeedConfig("cfg-theirs", OtherProjectId);
        var lastAt = new DateTime(2026, 8, 15, 9, 0, 0, DateTimeKind.Utc);
        SeedRegression(mine, lastAt.AddDays(-1));
        SeedRegression(mine, lastAt);
        SeedRegression(theirs, lastAt.AddDays(5)); // other project — invisible here
        await _db.SaveChangesAsync();

        var s = await BenchmarkRegressionsEndpoints.ComputeSummaryAsync(_db, ProjectId, default);

        Assert.Equal(2, s.TotalRegressions);
        Assert.Equal(lastAt, s.LastRegressionAt);
    }

    [Fact]
    public async Task Runs_in_other_projects_do_not_leak_into_the_summary()
    {
        var theirs = SeedConfig("cfg-foreign", OtherProjectId);
        SeedRun(theirs, "completed", artifact: true, DateTime.UtcNow.AddHours(-1), OtherProjectId);
        SeedRun(theirs, "completed", artifact: true, DateTime.UtcNow, OtherProjectId);
        await _db.SaveChangesAsync();

        var s = await BenchmarkRegressionsEndpoints.ComputeSummaryAsync(_db, ProjectId, default);

        Assert.Equal(0, s.RunsCompared);
        Assert.Equal(0, s.ComparableConfigs);
    }

    // ── Pin / unpin baseline ─────────────────────────────────────────────

    [Fact]
    public async Task Pin_sets_the_configs_baseline_to_a_completed_artifact_run()
    {
        var cfg = SeedConfig("cfg-pin");
        var run = SeedRun(cfg, "completed", artifact: true, DateTime.UtcNow);
        await _db.SaveChangesAsync();

        var (outcome, dto) = await BenchmarkRegressionsEndpoints.PinBaselineAsync(_db, run, default);

        Assert.Equal(BenchmarkRegressionsEndpoints.PinBaselineOutcome.Pinned, outcome);
        Assert.Equal(cfg, dto!.ConfigId);
        Assert.Equal(run, dto.BaselineRunId);
        Assert.Equal(run, (await _db.TestConfigs.AsNoTracking().SingleAsync(c => c.Id == cfg)).BaselineRunId);
    }

    [Theory]
    [InlineData("running", true)]
    [InlineData("failed", true)]
    [InlineData("completed", false)] // completed but artifact-less — detector can't use it
    public async Task Pin_rejects_runs_the_detector_could_not_compare_against(
        string status, bool artifact)
    {
        var cfg = SeedConfig($"cfg-reject-{status}-{artifact}");
        var run = SeedRun(cfg, status, artifact, DateTime.UtcNow);
        await _db.SaveChangesAsync();

        var (outcome, dto) = await BenchmarkRegressionsEndpoints.PinBaselineAsync(_db, run, default);

        Assert.Equal(BenchmarkRegressionsEndpoints.PinBaselineOutcome.NotComparable, outcome);
        Assert.Null(dto);
        Assert.Null((await _db.TestConfigs.AsNoTracking().SingleAsync(c => c.Id == cfg)).BaselineRunId);
    }

    [Fact]
    public async Task Pin_of_an_unknown_run_is_not_found()
    {
        var (outcome, _) = await BenchmarkRegressionsEndpoints.PinBaselineAsync(_db, Guid.NewGuid(), default);
        Assert.Equal(BenchmarkRegressionsEndpoints.PinBaselineOutcome.NotFound, outcome);
    }

    [Fact]
    public async Task Unpin_clears_the_pin_only_from_the_pinned_run()
    {
        var cfg = SeedConfig("cfg-unpin");
        var pinnedRun = SeedRun(cfg, "completed", artifact: true, DateTime.UtcNow.AddHours(-1));
        var otherRun = SeedRun(cfg, "completed", artifact: true, DateTime.UtcNow);
        await _db.SaveChangesAsync();
        await _db.TestConfigs.Where(c => c.Id == cfg)
            .ExecuteUpdateAsync(u => u.SetProperty(c => c.BaselineRunId, pinnedRun));
        _db.ChangeTracker.Clear(); // ExecuteUpdate bypasses the tracker

        // Unpinning from a run that is NOT the pin must not clobber the pin.
        var (notPinned, _) = await BenchmarkRegressionsEndpoints.UnpinBaselineAsync(_db, otherRun, default);
        Assert.Equal(BenchmarkRegressionsEndpoints.PinBaselineOutcome.NotPinned, notPinned);
        Assert.Equal(pinnedRun, (await _db.TestConfigs.AsNoTracking().SingleAsync(c => c.Id == cfg)).BaselineRunId);

        var (unpinned, dto) = await BenchmarkRegressionsEndpoints.UnpinBaselineAsync(_db, pinnedRun, default);
        Assert.Equal(BenchmarkRegressionsEndpoints.PinBaselineOutcome.Unpinned, unpinned);
        Assert.Null(dto!.BaselineRunId);
        Assert.Null((await _db.TestConfigs.AsNoTracking().SingleAsync(c => c.Id == cfg)).BaselineRunId);
    }

    // ── Wire contract ────────────────────────────────────────────────────

    [Fact]
    public void Summary_serializes_the_exact_snake_case_field_set()
    {
        var json = JsonSerializer.Serialize(
            new BenchmarkRegressionSummary(3, 1, 12,
                new DateTime(2026, 8, 18, 12, 0, 0, DateTimeKind.Utc), 2,
                new DateTime(2026, 8, 18, 12, 0, 0, DateTimeKind.Utc),
                BenchmarkRegressionsEndpoints.SummarySemantics),
            new JsonSerializerOptions(JsonSerializerDefaults.Web));
        var root = JsonNode.Parse(json)!.AsObject();

        var expected = new[]
        {
            "comparable_configs", "pinned_baseline_configs", "runs_compared",
            "last_comparison_at", "total_regressions", "last_regression_at",
            "semantics",
        };
        Assert.Equal(expected.OrderBy(x => x), root.Select(p => p.Key).OrderBy(x => x));
        Assert.Equal(12, (int)root["runs_compared"]!);
    }

    // ── Fixture ──────────────────────────────────────────────────────────

    private Guid SeedConfig(string name, string projectId = ProjectId)
    {
        var id = Guid.NewGuid();
        _db.TestConfigs.Add(new TestConfig
        {
            Id = id,
            ProjectId = projectId,
            Name = name,
            EndpointKind = "network",
            EndpointRef = "{}",
            Workload = "{}",
            Methodology = "{}",
            CreatedAt = DateTime.UtcNow,
            UpdatedAt = DateTime.UtcNow,
            MaxDurationSecs = 900,
        });
        return id;
    }

    private Guid SeedRun(
        Guid configId, string status, bool artifact, DateTime finishedAt,
        string projectId = ProjectId)
    {
        var runId = Guid.NewGuid();
        _db.TestRuns.Add(new TestRun
        {
            Id = runId,
            TestConfigId = configId,
            ProjectId = projectId,
            Status = status,
            ArtifactId = artifact ? Guid.NewGuid() : null,
            CreatedAt = finishedAt.AddMinutes(-5),
            FinishedAt = status is "completed" or "failed" ? finishedAt : null,
        });
        return runId;
    }

    private void SeedRegression(Guid configId, DateTime detectedAt)
    {
        _db.BenchmarkRegressions.Add(new BenchmarkRegressionRecord
        {
            RegressionId = Guid.NewGuid(),
            TestConfigId = configId,
            TestRunId = Guid.NewGuid(),
            BaselineRunId = Guid.NewGuid(),
            CaseId = "http1-1024",
            Metric = "p50_latency_ms",
            MetricUnit = "ms",
            BaselineValue = 10,
            CurrentValue = 20,
            DeltaPercent = 100,
            Severity = "critical",
            DetectedAt = detectedAt,
        });
    }

    private static void CreateMinimalSchema(Microsoft.Data.Sqlite.SqliteConnection conn)
    {
        Exec(conn, """
            CREATE TABLE test_config (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT,
                endpoint_kind TEXT NOT NULL,
                test_kind TEXT NOT NULL DEFAULT 'network',
                endpoint_ref TEXT NOT NULL,
                workload TEXT NOT NULL,
                methodology TEXT,
                created_by TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                baseline_run_id TEXT,
                max_duration_secs INTEGER NOT NULL,
                token_enc BLOB,
                token_nonce BLOB
            );
            """);
        Exec(conn, """
            CREATE TABLE test_run (
                id TEXT PRIMARY KEY,
                test_config_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                status TEXT NOT NULL,
                started_at TEXT,
                finished_at TEXT,
                success_count INTEGER NOT NULL DEFAULT 0,
                failure_count INTEGER NOT NULL DEFAULT 0,
                error_message TEXT,
                artifact_id TEXT,
                tester_id TEXT,
                worker_id TEXT,
                last_heartbeat TEXT,
                created_at TEXT NOT NULL,
                comparison_group_id TEXT,
                provisioning_deployment_id TEXT,
                provision_attempts INTEGER NOT NULL DEFAULT 0,
                next_provision_attempt_at TEXT,
                client_envelope TEXT
            );
            """);
        Exec(conn, """
            CREATE TABLE benchmark_regression (
                regression_id TEXT PRIMARY KEY,
                test_config_id TEXT NOT NULL,
                test_run_id TEXT NOT NULL,
                baseline_run_id TEXT,
                case_id TEXT NOT NULL,
                metric TEXT NOT NULL,
                metric_unit TEXT NOT NULL,
                baseline_value REAL NOT NULL,
                current_value REAL NOT NULL,
                delta_percent REAL NOT NULL,
                severity TEXT NOT NULL DEFAULT 'warning',
                detected_at TEXT NOT NULL
            );
            """);
    }

    private static void Exec(Microsoft.Data.Sqlite.SqliteConnection conn, string sql)
    {
        using var cmd = conn.CreateCommand();
        cmd.CommandText = sql;
        cmd.ExecuteNonQuery();
    }
}
