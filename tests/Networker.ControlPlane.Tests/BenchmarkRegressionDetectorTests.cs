using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Provisioning;
using Networker.ControlPlane.Realtime;
using Networker.ControlPlane.Realtime.RawWs;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Integration-style tests for <see cref="BenchmarkRegressionDetector"/>
/// against a relational (Sqlite) <see cref="NetworkerDbContext"/> — the same
/// fixture pattern as <c>AnsiScrubIngestTests</c>. Covers baseline resolution
/// (previous run vs pinned <c>baseline_run_id</c>), persistence + event
/// emission, first-run/no-baseline, idempotence, and the run_finished hook in
/// <see cref="AgentMessageProcessor"/> end-to-end.
/// </summary>
public sealed class BenchmarkRegressionDetectorTests
{
    private const string ProjectId = "proj-regr-test";

    private static string SummariesJson(double p50, long success = 100, long failure = 0, long included = 100) =>
        $$"""
        [{
            "case_id": "http1-1024",
            "protocol": "http1",
            "metric_name": "total_duration_ms",
            "metric_unit": "ms",
            "higher_is_better": false,
            "sample_count": {{success + failure}},
            "included_sample_count": {{included}},
            "success_count": {{success}},
            "failure_count": {{failure}},
            "p50": {{p50}}
        }]
        """;

    [Fact]
    public async Task Detects_p50_regression_vs_previous_run_and_emits_event()
    {
        using var sp = BuildHost();
        var db = sp.GetRequiredService<NetworkerDbContext>();
        var bus = sp.GetRequiredService<EventBus>();

        var configId = SeedConfig(db, "cfg-a", baselineRunId: null);
        SeedCompletedRun(db, configId, SummariesJson(10.0), createdAt: DateTime.UtcNow.AddHours(-1));
        var currentRun = SeedCompletedRun(db, configId, SummariesJson(20.0), createdAt: DateTime.UtcNow);
        await db.SaveChangesAsync();

        await NewDetector(sp).DetectAsync(currentRun);

        var row = Assert.Single(await db.BenchmarkRegressions.AsNoTracking().ToListAsync());
        Assert.Equal(configId, row.TestConfigId);
        Assert.Equal(currentRun, row.TestRunId);
        Assert.NotNull(row.BaselineRunId);
        Assert.Equal("http1-1024", row.CaseId);
        Assert.Equal(RegressionAnalyzer.MetricP50LatencyMs, row.Metric);
        Assert.Equal(10.0, row.BaselineValue);
        Assert.Equal(20.0, row.CurrentValue);
        Assert.Equal(100.0, row.DeltaPercent, 6);
        Assert.Equal(RegressionAnalyzer.SeverityCritical, row.Severity);

        var evt = Assert.IsType<BenchmarkRegression>(Assert.Single(bus.Replay(0)).Event);
        Assert.Equal(configId, evt.ConfigId);
        Assert.Equal("cfg-a", evt.ConfigName);
        Assert.Equal(1, evt.RegressionCount);
    }

    [Fact]
    public async Task First_run_of_a_config_has_no_baseline_and_flags_nothing()
    {
        using var sp = BuildHost();
        var db = sp.GetRequiredService<NetworkerDbContext>();
        var bus = sp.GetRequiredService<EventBus>();

        var configId = SeedConfig(db, "cfg-first", baselineRunId: null);
        var onlyRun = SeedCompletedRun(db, configId, SummariesJson(500.0), createdAt: DateTime.UtcNow);
        await db.SaveChangesAsync();

        await NewDetector(sp).DetectAsync(onlyRun);

        Assert.Empty(await db.BenchmarkRegressions.AsNoTracking().ToListAsync());
        Assert.Empty(bus.Replay(0));
    }

    [Fact]
    public async Task Within_threshold_run_flags_nothing()
    {
        using var sp = BuildHost();
        var db = sp.GetRequiredService<NetworkerDbContext>();

        var configId = SeedConfig(db, "cfg-ok", baselineRunId: null);
        SeedCompletedRun(db, configId, SummariesJson(10.0), createdAt: DateTime.UtcNow.AddHours(-1));
        var currentRun = SeedCompletedRun(db, configId, SummariesJson(10.5), createdAt: DateTime.UtcNow);
        await db.SaveChangesAsync();

        await NewDetector(sp).DetectAsync(currentRun);

        Assert.Empty(await db.BenchmarkRegressions.AsNoTracking().ToListAsync());
    }

    [Fact]
    public async Task Pinned_baseline_run_wins_over_the_previous_run()
    {
        using var sp = BuildHost();
        var db = sp.GetRequiredService<NetworkerDbContext>();

        var configId = SeedConfig(db, "cfg-pinned", baselineRunId: null);
        var pinnedRun = SeedCompletedRun(db, configId, SummariesJson(10.0), createdAt: DateTime.UtcNow.AddHours(-2));
        // A newer intermediate run with p50 = 20 — if the detector wrongly used
        // "previous run" it would compare 21 vs 20 (within threshold, no flag).
        SeedCompletedRun(db, configId, SummariesJson(20.0), createdAt: DateTime.UtcNow.AddHours(-1));
        var currentRun = SeedCompletedRun(db, configId, SummariesJson(21.0), createdAt: DateTime.UtcNow);
        await db.SaveChangesAsync();
        await db.TestConfigs
            .Where(c => c.Id == configId)
            .ExecuteUpdateAsync(s => s.SetProperty(c => c.BaselineRunId, pinnedRun));

        await NewDetector(sp).DetectAsync(currentRun);

        var row = Assert.Single(await db.BenchmarkRegressions.AsNoTracking().ToListAsync());
        Assert.Equal(pinnedRun, row.BaselineRunId);
        Assert.Equal(10.0, row.BaselineValue);
        Assert.Equal(21.0, row.CurrentValue);
    }

    [Fact]
    public async Task Repeated_detection_for_the_same_run_is_idempotent()
    {
        using var sp = BuildHost();
        var db = sp.GetRequiredService<NetworkerDbContext>();

        var configId = SeedConfig(db, "cfg-idem", baselineRunId: null);
        SeedCompletedRun(db, configId, SummariesJson(10.0), createdAt: DateTime.UtcNow.AddHours(-1));
        var currentRun = SeedCompletedRun(db, configId, SummariesJson(20.0), createdAt: DateTime.UtcNow);
        await db.SaveChangesAsync();

        var detector = NewDetector(sp);
        await detector.DetectAsync(currentRun);
        await detector.DetectAsync(currentRun);

        Assert.Single(await db.BenchmarkRegressions.AsNoTracking().ToListAsync());
    }

    [Fact]
    public async Task Run_finished_frame_triggers_detection_through_the_processor()
    {
        using var sp = BuildHost();
        var db = sp.GetRequiredService<NetworkerDbContext>();

        var configId = SeedConfig(db, "cfg-frame", baselineRunId: null);
        SeedCompletedRun(db, configId, SummariesJson(10.0), createdAt: DateTime.UtcNow.AddHours(-1));
        var runId = Guid.NewGuid();
        db.TestRuns.Add(new TestRun
        {
            Id = runId,
            TestConfigId = configId,
            ProjectId = ProjectId,
            Status = "running",
            CreatedAt = DateTime.UtcNow,
        });
        await db.SaveChangesAsync();

        var processor = new AgentMessageProcessor(
            db,
            sp.GetRequiredService<EventBus>(),
            sp.GetRequiredService<ILogger<AgentMessageProcessor>>(),
            alerts: null,
            regressions: NewDetector(sp));

        var empty = JsonDocument.Parse("{}").RootElement;
        var summaries = JsonDocument.Parse(SummariesJson(20.0)).RootElement;
        var frame = JsonSerializer.Serialize<AgentMessage>(new RunFinishedMessage(
            runId,
            "completed",
            new BenchmarkArtifactPayload(
                empty, empty,
                JsonDocument.Parse("[]").RootElement,
                JsonDocument.Parse("[]").RootElement,
                null,
                summaries,
                empty)));
        await processor.HandleFrameAsync(Guid.NewGuid(), frame);

        var row = Assert.Single(await db.BenchmarkRegressions.AsNoTracking().ToListAsync());
        Assert.Equal(runId, row.TestRunId);
        Assert.Equal(RegressionAnalyzer.MetricP50LatencyMs, row.Metric);
    }

    // ── Fixture (same Sqlite pattern as AnsiScrubIngestTests: relational
    //    provider, only the tables these paths touch) ─────────────────────────

    private static BenchmarkRegressionDetector NewDetector(ServiceProvider sp) => new(
        sp.GetRequiredService<NetworkerDbContext>(),
        sp.GetRequiredService<EventBus>(),
        sp.GetRequiredService<ILogger<BenchmarkRegressionDetector>>());

    private static Guid SeedConfig(NetworkerDbContext db, string name, Guid? baselineRunId)
    {
        var id = Guid.NewGuid();
        db.TestConfigs.Add(new TestConfig
        {
            Id = id,
            ProjectId = ProjectId,
            Name = name,
            EndpointKind = "network",
            EndpointRef = "{}",
            Workload = "{}",
            Methodology = "{}",
            BaselineRunId = baselineRunId,
            CreatedAt = DateTime.UtcNow,
            UpdatedAt = DateTime.UtcNow,
            MaxDurationSecs = 900,
        });
        return id;
    }

    private static Guid SeedCompletedRun(
        NetworkerDbContext db, Guid configId, string summariesJson, DateTime createdAt)
    {
        var runId = Guid.NewGuid();
        var artifactId = Guid.NewGuid();
        db.TestRuns.Add(new TestRun
        {
            Id = runId,
            TestConfigId = configId,
            ProjectId = ProjectId,
            Status = "completed",
            ArtifactId = artifactId,
            CreatedAt = createdAt,
            FinishedAt = createdAt,
        });
        db.BenchmarkArtifacts.Add(new BenchmarkArtifact
        {
            Id = artifactId,
            TestRunId = runId,
            Environment = "{}",
            Methodology = "{}",
            Launches = "[]",
            Cases = "[]",
            Summaries = summariesJson,
            DataQuality = "{}",
            CreatedAt = createdAt,
        });
        return runId;
    }

    private static ServiceProvider BuildHost()
    {
        var conn = new Microsoft.Data.Sqlite.SqliteConnection("DataSource=:memory:");
        conn.Open();

        var services = new ServiceCollection();
        services.AddLogging(b => b.SetMinimumLevel(LogLevel.Warning));
        services.AddSignalR();
        services.AddAgentProtocol();
        services.AddDashboardEventBus();
        services.AddSingleton(conn);
        services.AddDbContext<NetworkerDbContext>(o => o.UseSqlite(conn));

        var sp = services.BuildServiceProvider();
        CreateMinimalSchema(conn);
        return sp;
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
                client_envelope TEXT
            );
            """);
        Exec(conn, """
            CREATE TABLE benchmark_artifact (
                id TEXT PRIMARY KEY,
                test_run_id TEXT NOT NULL,
                environment TEXT NOT NULL,
                methodology TEXT NOT NULL,
                launches TEXT NOT NULL,
                cases TEXT NOT NULL,
                samples TEXT,
                summaries TEXT NOT NULL,
                data_quality TEXT NOT NULL,
                created_at TEXT NOT NULL
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
