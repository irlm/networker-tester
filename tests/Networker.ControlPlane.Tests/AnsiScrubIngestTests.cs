using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Realtime;
using Networker.ControlPlane.Realtime.RawWs;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Regression tests for design-audit F8 at the WRITE path: the agent relays
/// tester stderr verbatim (SGR-colorized by the tester's tracing subscriber),
/// and the control plane used to persist those raw ANSI codes into
/// <c>test_run.error_message</c> / <c>agent_command.error_message</c> — leaking
/// them to every consumer (API, exports). These tests run a real ANSI-laden
/// frame through <see cref="AgentMessageProcessor.HandleFrameAsync"/> against
/// a relational (Sqlite) <see cref="NetworkerDbContext"/> (the handlers use
/// <c>ExecuteUpdateAsync</c>, which InMemory does not support) and assert the
/// STORED text is clean.
/// </summary>
public sealed class AnsiScrubIngestTests
{
    private const string ProjectId = "proj-ansi-test";
    private const string Esc = "";

    /// <summary>The tester log line the audit captured, as the agent relays it.</summary>
    private static readonly string AnsiLaden =
        $"[tester] {Esc}[2m2026-07-14T01:22:24.974248Z{Esc}[0m " +
        $"{Esc}[31mERROR{Esc}[0m {Esc}[2mnetworker_tester{Esc}[0m connection refused (os error 111)";

    private const string CleanExpected =
        "[tester] 2026-07-14T01:22:24.974248Z ERROR networker_tester connection refused (os error 111)";

    [Fact]
    public async Task Error_frame_stores_ansi_stripped_error_message_on_the_run()
    {
        using var sp = BuildHost();
        var db = sp.GetRequiredService<NetworkerDbContext>();

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

        var processor = new AgentMessageProcessor(
            db,
            sp.GetRequiredService<EventBus>(),
            sp.GetRequiredService<ILogger<AgentMessageProcessor>>());

        var frame = System.Text.Json.JsonSerializer.Serialize<AgentMessage>(
            new ErrorMessage(runId, AnsiLaden));
        await processor.HandleFrameAsync(Guid.NewGuid(), frame);

        var run = await db.TestRuns.AsNoTracking().FirstAsync(r => r.Id == runId);
        Assert.Equal("failed", run.Status);
        Assert.Equal(CleanExpected, run.ErrorMessage);
        Assert.DoesNotContain(Esc, run.ErrorMessage);
    }

    [Fact]
    public async Task Command_result_frame_stores_ansi_stripped_error()
    {
        using var sp = BuildHost();
        var db = sp.GetRequiredService<NetworkerDbContext>();

        var commandId = Guid.NewGuid();
        db.AgentCommands.Add(new AgentCommand
        {
            CommandId = commandId,
            AgentId = Guid.NewGuid(),
            Verb = "install",
            Args = "{}",
            Status = "running",
            CreatedAt = DateTime.UtcNow,
        });
        await db.SaveChangesAsync();

        var processor = new AgentMessageProcessor(
            db,
            sp.GetRequiredService<EventBus>(),
            sp.GetRequiredService<ILogger<AgentMessageProcessor>>());

        var frame = System.Text.Json.JsonSerializer.Serialize<AgentMessage>(
            new CommandResultMessage(commandId, "error", null, AnsiLaden, 1200));
        await processor.HandleFrameAsync(Guid.NewGuid(), frame);

        var cmd = await db.AgentCommands.AsNoTracking().FirstAsync(c => c.CommandId == commandId);
        Assert.Equal("error", cmd.Status);
        Assert.Equal(CleanExpected, cmd.ErrorMessage);
        Assert.DoesNotContain(Esc, cmd.ErrorMessage);
    }

    // ── Test host wiring (same Sqlite pattern as RunDispatcherTesterFkTests:
    //    a relational provider is required because the handlers use
    //    ExecuteUpdateAsync; only the tables these paths touch are created) ──

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
                provisioning_deployment_id TEXT
            );
            """);
        Exec(conn, """
            CREATE TABLE agent_command (
                command_id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                config_id TEXT,
                verb TEXT NOT NULL,
                args TEXT NOT NULL,
                status TEXT NOT NULL,
                result TEXT,
                error_message TEXT,
                created_by TEXT,
                created_at TEXT NOT NULL,
                started_at TEXT,
                finished_at TEXT
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
