using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Realtime;
using Networker.ControlPlane.Realtime.RawWs;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Regression tests for E2E-pass finding P0-1 (2026-07-28): the agent streams
/// tester stderr as <c>error</c> frames labelled <c>[tester] …</c> (RunExecutor
/// step 5), and the tester's tracing subscriber writes INFO to stderr — so the
/// control plane used to mark every healthy run <c>failed</c> ~10 ms after
/// spawn, with the startup INFO line as its "error" (a 60/60-success run
/// showed <c>status=failed</c> live). Relayed subprocess output must be log
/// streaming only; the agent's own UNPREFIXED critical frames keep deciding
/// status.
/// </summary>
public sealed class TesterStderrRelayTests
{
    private const string ProjectId = "proj-relay-test";

    [Theory]
    [InlineData("[tester] 2026-07-28T20:54:44.105117Z  INFO networker_tester: Starting networker-tester targets=[\"https://example.com/health\"]")]
    [InlineData("[tester] 2026-07-28T20:54:44Z ERROR networker_tester: connection refused (os error 111)")]
    [InlineData("[tester/http2] warmup 3/10 complete")]
    public async Task Relayed_tester_stderr_never_fails_a_running_run(string relayed)
    {
        using var sp = BuildHost();
        var db = sp.GetRequiredService<NetworkerDbContext>();
        var runId = await SeedRunningRunAsync(db);

        var processor = BuildProcessor(sp, db);
        var frame = System.Text.Json.JsonSerializer.Serialize<AgentMessage>(
            new ErrorMessage(runId, relayed));
        await processor.HandleFrameAsync(Guid.NewGuid(), frame);

        var run = await db.TestRuns.AsNoTracking().FirstAsync(r => r.Id == runId);
        Assert.Equal("running", run.Status);       // untouched
        Assert.Null(run.ErrorMessage);             // logs are not the verdict
        Assert.Null(run.FinishedAt);
    }

    [Fact]
    public async Task Unprefixed_agent_critical_frame_still_fails_the_run()
    {
        using var sp = BuildHost();
        var db = sp.GetRequiredService<NetworkerDbContext>();
        var runId = await SeedRunningRunAsync(db);

        var processor = BuildProcessor(sp, db);
        var frame = System.Text.Json.JsonSerializer.Serialize<AgentMessage>(
            new ErrorMessage(runId, "Failed to spawn tester: no such file or directory"));
        await processor.HandleFrameAsync(Guid.NewGuid(), frame);

        var run = await db.TestRuns.AsNoTracking().FirstAsync(r => r.Id == runId);
        Assert.Equal("failed", run.Status);
        Assert.Equal("Failed to spawn tester: no such file or directory", run.ErrorMessage);
        Assert.NotNull(run.FinishedAt);
    }

    [Theory]
    [InlineData("[tester] anything", true)]
    [InlineData("[tester/download] anything", true)]
    [InlineData("Failed to spawn tester: x", false)]
    [InlineData("Tester (tester/http) exceeded the overall run deadline of 00:16:00 — killed", false)]
    [InlineData("[testerx] not-the-label", false)]   // label match is exact
    [InlineData("", false)]
    public void Relay_prefix_detection_matches_the_agent_label_exactly(string message, bool expected)
    {
        Assert.Equal(expected, AgentMessageProcessor.IsRelayedSubprocessOutput(message));
    }

    // ── host wiring: same Sqlite pattern as AnsiScrubIngestTests ─────────────

    private static async Task<Guid> SeedRunningRunAsync(NetworkerDbContext db)
    {
        var runId = Guid.NewGuid();
        db.TestRuns.Add(new TestRun
        {
            Id = runId,
            TestConfigId = Guid.NewGuid(),
            ProjectId = ProjectId,
            Status = "running",
            CreatedAt = DateTime.UtcNow,
        });
        await db.SaveChangesAsync();
        return runId;
    }

    private static AgentMessageProcessor BuildProcessor(ServiceProvider sp, NetworkerDbContext db) =>
        new(db,
            sp.GetRequiredService<EventBus>(),
            sp.GetRequiredService<ILogger<AgentMessageProcessor>>());

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
        return sp;
    }

    private static void Exec(Microsoft.Data.Sqlite.SqliteConnection conn, string sql)
    {
        using var cmd = conn.CreateCommand();
        cmd.CommandText = sql;
        cmd.ExecuteNonQuery();
    }
}
