using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Realtime;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// <see cref="TesterQueueUpdateProducer"/> — the producer of live
/// <c>tester_queue_update</c> deltas that closed the 2026-07 feature gap (the
/// dashboard subscribed to these but nothing ever sent them). Pins: a run
/// transition for a SUBSCRIBED tester pushes the rebuilt queue (running +
/// 1-based positions + rolling-average ETAs); no subscribers / no tester /
/// unknown run are cheap no-ops; the trigger mapping.
/// </summary>
public sealed class TesterQueueUpdateProducerTests
{
    private const string ProjectId = "proj-tqup-0001";

    // ── Recording push seam (replaces the SignalR-backed broadcaster) ────────

    private sealed class RecordingPush : ITesterQueuePush
    {
        public readonly List<(string ProjectId, string TesterId, string Trigger,
            TesterQueueEntry? Running, IReadOnlyList<TesterQueueEntry> Queued)> Calls = [];

        public Task NotifyQueueUpdateAsync(
            string projectId, string testerId, string trigger,
            TesterQueueEntry? running, IReadOnlyList<TesterQueueEntry> queued,
            CancellationToken ct = default)
        {
            lock (Calls)
            {
                Calls.Add((projectId, testerId, trigger, running, queued));
            }
            return Task.CompletedTask;
        }
    }

    // ── Test host (mirrors RunDispatcherProjectScopeTests) ───────────────────

    private static (ServiceProvider Sp, TesterQueueRegistry Registry, RecordingPush Push,
        TesterQueueUpdateProducer Producer) BuildHost()
    {
        var conn = new Microsoft.Data.Sqlite.SqliteConnection("DataSource=:memory:");
        conn.Open();

        var services = new ServiceCollection();
        services.AddLogging(b => b.SetMinimumLevel(LogLevel.Warning));
        services.AddSingleton(conn);
        services.AddDbContext<NetworkerDbContext>(o => o.UseSqlite(conn));

        var sp = services.BuildServiceProvider();
        CreateMinimalSchema(conn);

        var registry = new TesterQueueRegistry();
        var push = new RecordingPush();
        var producer = new TesterQueueUpdateProducer(
            sp.GetRequiredService<IServiceScopeFactory>(),
            registry,
            push,
            sp.GetRequiredService<ILogger<TesterQueueUpdateProducer>>());
        return (sp, registry, push, producer);
    }

    private static void CreateMinimalSchema(Microsoft.Data.Sqlite.SqliteConnection conn)
    {
        Exec(conn, """
            CREATE TABLE project_tester (
                tester_id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                name TEXT NOT NULL,
                cloud TEXT NOT NULL,
                region TEXT NOT NULL,
                vm_size TEXT NOT NULL,
                ssh_user TEXT NOT NULL,
                power_state TEXT NOT NULL,
                allocation TEXT NOT NULL,
                status_message TEXT,
                auto_shutdown_enabled INTEGER NOT NULL DEFAULT 0,
                auto_shutdown_local_hour INTEGER NOT NULL DEFAULT 0,
                shutdown_deferral_count INTEGER NOT NULL DEFAULT 0,
                auto_probe_enabled INTEGER NOT NULL DEFAULT 0,
                avg_benchmark_duration_seconds INTEGER,
                benchmark_run_count INTEGER NOT NULL DEFAULT 0,
                created_by TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT,
                vm_name TEXT, vm_resource_id TEXT, public_ip TEXT,
                locked_by_config_id TEXT, installer_version TEXT,
                last_installed_at TEXT, next_shutdown_at TEXT, last_used_at TEXT,
                cloud_connection_id TEXT, requested_os TEXT, requested_variant TEXT,
                os_distro TEXT, os_version TEXT, os_variant TEXT, os_arch TEXT,
                os_kernel TEXT, cloud_account_id TEXT
            );
            """);
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
                max_duration_secs INTEGER NOT NULL DEFAULT 0,
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
                provisioning_deployment_id TEXT
            );
            """);
    }

    private static void Exec(Microsoft.Data.Sqlite.SqliteConnection conn, string sql)
    {
        using var cmd = conn.CreateCommand();
        cmd.CommandText = sql;
        cmd.ExecuteNonQuery();
    }

    // ── Seeding ──────────────────────────────────────────────────────────────

    private static Guid SeedTester(IServiceProvider sp, int? avgSecs)
    {
        using var scope = sp.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
        var testerId = Guid.NewGuid();
        db.ProjectTesters.Add(new ProjectTester
        {
            TesterId = testerId,
            ProjectId = ProjectId,
            Name = "tq-tester",
            Cloud = "azure",
            Region = "eastus",
            VmSize = "Standard_B1s",
            SshUser = "azureuser",
            PowerState = "running",
            Allocation = "on-demand",
            AutoShutdownEnabled = false,
            AutoShutdownLocalHour = 0,
            ShutdownDeferralCount = 0,
            AutoProbeEnabled = false,
            AvgBenchmarkDurationSeconds = avgSecs,
            BenchmarkRunCount = 0,
            CreatedAt = DateTime.UtcNow,
        });
        db.SaveChanges();
        return testerId;
    }

    private static Guid SeedRun(
        IServiceProvider sp, Guid? testerId, string status, string configName, DateTime createdAt)
    {
        using var scope = sp.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
        var configId = Guid.NewGuid();
        db.TestConfigs.Add(new TestConfig
        {
            Id = configId,
            ProjectId = ProjectId,
            Name = configName,
            EndpointKind = "network",
            EndpointRef = "{}",
            Workload = "{}",
            MaxDurationSecs = 60,
            CreatedAt = createdAt,
            UpdatedAt = createdAt,
        });
        var runId = Guid.NewGuid();
        db.TestRuns.Add(new TestRun
        {
            Id = runId,
            TestConfigId = configId,
            ProjectId = ProjectId,
            Status = status,
            TesterId = testerId,
            CreatedAt = createdAt,
        });
        db.SaveChanges();
        return runId;
    }

    // ── Tests ────────────────────────────────────────────────────────────────

    [Fact]
    public async Task Transition_pushes_rebuilt_queue_to_subscribed_tester()
    {
        var (sp, registry, push, producer) = BuildHost();
        var testerId = SeedTester(sp, avgSecs: 120);
        var t0 = new DateTime(2026, 7, 22, 0, 0, 0, DateTimeKind.Utc);
        SeedRun(sp, testerId, "running", "cfg-running", t0);
        SeedRun(sp, testerId, "queued", "cfg-q1", t0.AddMinutes(1));
        var triggering = SeedRun(sp, testerId, "queued", "cfg-q2", t0.AddMinutes(2));
        registry.TrySubscribe(ProjectId, testerId.ToString(), "conn-1");

        await producer.HandleAsync(triggering, "run_queued");

        var call = Assert.Single(push.Calls);
        Assert.Equal(ProjectId, call.ProjectId);
        Assert.Equal(testerId.ToString(), call.TesterId);
        Assert.Equal("run_queued", call.Trigger);
        Assert.NotNull(call.Running);
        Assert.Equal("cfg-running", call.Running!.Name);
        // Queued oldest-first with 1-based positions + rolling-average ETAs.
        Assert.Equal(2, call.Queued.Count);
        Assert.Equal("cfg-q1", call.Queued[0].Name);
        Assert.Equal(1u, call.Queued[0].Position);
        Assert.Equal(0u, call.Queued[0].EtaSeconds);
        Assert.Equal("cfg-q2", call.Queued[1].Name);
        Assert.Equal(2u, call.Queued[1].Position);
        Assert.Equal(120u, call.Queued[1].EtaSeconds);
    }

    [Fact]
    public async Task No_subscribers_means_no_push()
    {
        var (sp, _, push, producer) = BuildHost();
        var testerId = SeedTester(sp, avgSecs: null);
        var runId = SeedRun(sp, testerId, "running", "cfg", DateTime.UtcNow);

        await producer.HandleAsync(runId, "run_running");

        Assert.Empty(push.Calls);
    }

    [Fact]
    public async Task Run_without_a_tester_is_a_no_op()
    {
        var (sp, registry, push, producer) = BuildHost();
        var runId = SeedRun(sp, testerId: null, "queued", "cfg", DateTime.UtcNow);

        await producer.HandleAsync(runId, "run_queued");

        Assert.Empty(push.Calls);
    }

    [Fact]
    public async Task Unknown_run_is_a_no_op_not_an_exception()
    {
        var (_, _, push, producer) = BuildHost();

        await producer.HandleAsync(Guid.NewGuid(), "run_running");

        Assert.Empty(push.Calls);
    }

    [Theory]
    [InlineData("queued", "run_queued")]
    [InlineData("running", "run_running")]
    [InlineData("failed", "run_failed")]
    [InlineData("cancelled", "run_cancelled")]
    public void TriggerFor_prefixes_the_run_status(string status, string expected)
    {
        Assert.Equal(expected, TesterQueueUpdateProducer.TriggerFor(status));
    }
}
