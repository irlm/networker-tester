using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Background;
using Networker.ControlPlane.Dispatch;
using Networker.ControlPlane.Realtime;
using Networker.ControlPlane.Realtime.RawWs;
using Networker.Data;
using Networker.Data.Entities;
using Networker.Security;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Regression tests for the live-prod correctness bug where a prior "fix"
/// stamped <c>test_run.tester_id = agent_id</c>. Ground truth (verified against
/// the prod Postgres schema): <c>test_run.tester_id</c> is a FK to
/// <c>project_tester(tester_id)</c> — the tester a run runs on, NEVER the
/// agent's id. The executing agent is tracked in <c>test_run.worker_id</c> (a
/// nullable, FK-free string). Writing the agent_id into tester_id violates
/// <c>test_run_tester_id_fkey</c> (23503) and 500s launch/run_started.
///
/// <para>These tests assert the CORRECT model end-to-end against a real
/// EF-Core-InMemory <see cref="NetworkerDbContext"/>:</para>
/// <list type="bullet">
///   <item>dispatch stamps <c>worker_id = agent</c> and
///   <c>tester_id = agent.tester_id</c> (or null for a tester-less agent) —
///   never the agent_id into tester_id;</item>
///   <item>run_started does the same;</item>
///   <item>disconnect orphan-fail keys on worker_id;</item>
///   <item>the watchdog maps a run to its agent via worker_id;</item>
///   <item>agent-selection prefers the agent bound to the run's project_tester.</item>
/// </list>
/// </summary>
public sealed class RunDispatcherTesterFkTests
{
    private const string ProjectId = "proj-fk-test";

    // ── Test host wiring ─────────────────────────────────────────────────────

    // A relational provider (Sqlite) is used, not InMemory, because the
    // production code paths use ExecuteUpdateAsync — which the InMemory provider
    // does not support. Sqlite (in-memory, shared open connection) supports it.
    private static ServiceProvider BuildHost(string dbName)
    {
        // One shared, kept-open in-memory Sqlite connection per host so the
        // schema built by EnsureCreated survives across the multiple DI scopes
        // the watchdog opens.
        var conn = new Microsoft.Data.Sqlite.SqliteConnection("DataSource=:memory:");
        conn.Open();

        var services = new ServiceCollection();
        services.AddLogging(b => b.SetMinimumLevel(LogLevel.Warning));
        services.AddSignalR();
        services.AddAgentProtocol();     // AgentConnectionRegistry
        services.AddDashboardEventBus();  // EventBus (needs IHubContext<BrowserHub>)
        services.AddSingleton(conn);
        services.AddDbContext<NetworkerDbContext>(o => o.UseSqlite(conn));

        var sp = services.BuildServiceProvider();

        // The full Postgres model can't be built on Sqlite (it declares a
        // Timescale sequence Sqlite rejects), so create only the tables these
        // code paths touch — with the real column names AND the
        // test_run_tester_id_fkey FK enabled, so Sqlite enforces exactly the
        // constraint prod violated when agent_id was written into tester_id.
        CreateMinimalSchema(conn);
        return sp;
    }

    private static void CreateMinimalSchema(Microsoft.Data.Sqlite.SqliteConnection conn)
    {
        Exec(conn, "PRAGMA foreign_keys = ON;");
        Exec(conn, """
            CREATE TABLE project (
                project_id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                slug TEXT NOT NULL,
                description TEXT,
                created_by TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                settings TEXT NOT NULL,
                deleted_at TEXT,
                delete_protection INTEGER NOT NULL DEFAULT 0
            );
            """);
        Exec(conn, """
            CREATE TABLE project_tester (
                tester_id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                name TEXT NOT NULL,
                cloud TEXT NOT NULL,
                region TEXT NOT NULL,
                vm_size TEXT NOT NULL,
                vm_name TEXT,
                vm_resource_id TEXT,
                public_ip TEXT,
                ssh_user TEXT NOT NULL,
                power_state TEXT NOT NULL,
                allocation TEXT NOT NULL,
                status_message TEXT,
                locked_by_config_id TEXT,
                installer_version TEXT,
                last_installed_at TEXT,
                auto_shutdown_enabled INTEGER NOT NULL DEFAULT 0,
                auto_shutdown_local_hour INTEGER NOT NULL DEFAULT 0,
                next_shutdown_at TEXT,
                shutdown_deferral_count INTEGER NOT NULL DEFAULT 0,
                auto_probe_enabled INTEGER NOT NULL DEFAULT 0,
                last_used_at TEXT,
                avg_benchmark_duration_seconds INTEGER,
                benchmark_run_count INTEGER NOT NULL DEFAULT 0,
                created_by TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                cloud_connection_id TEXT,
                requested_os TEXT,
                requested_variant TEXT,
                os_distro TEXT,
                os_version TEXT,
                os_variant TEXT,
                os_arch TEXT,
                os_kernel TEXT,
                cloud_account_id TEXT
            );
            """);
        Exec(conn, """
            CREATE TABLE agent (
                agent_id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                region TEXT,
                provider TEXT,
                status TEXT NOT NULL,
                version TEXT,
                os TEXT,
                arch TEXT,
                last_heartbeat TEXT,
                registered_at TEXT NOT NULL,
                api_key TEXT NOT NULL,
                api_key_hash TEXT,
                tags TEXT,
                project_id TEXT NOT NULL,
                tester_id TEXT REFERENCES project_tester(tester_id)
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
                tester_id TEXT
                    CONSTRAINT test_run_tester_id_fkey REFERENCES project_tester(tester_id),
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

    private static NetworkerDbContext Db(IServiceProvider sp)
        => sp.GetRequiredService<NetworkerDbContext>();

    private static void SeedProject(NetworkerDbContext db)
    {
        var now = DateTime.UtcNow;
        db.Projects.Add(new Project
        {
            ProjectId = ProjectId,
            Name = "FK Test",
            Slug = "fk-test",
            Settings = "{}",
            CreatedAt = now,
            UpdatedAt = now,
        });
    }

    private static Guid SeedTester(NetworkerDbContext db, string name = "tester-a")
    {
        var id = Guid.NewGuid();
        var now = DateTime.UtcNow;
        db.ProjectTesters.Add(new ProjectTester
        {
            TesterId = id,
            ProjectId = ProjectId,
            Name = name,
            Cloud = "azure",
            Region = "eastus",
            VmSize = "Standard_B1s",
            SshUser = "azureuser",
            PowerState = "running",
            Allocation = "idle",
            CreatedBy = Guid.NewGuid(),
            CreatedAt = now,
            UpdatedAt = now,
        });
        return id;
    }

    /// <summary>Seed an agent whose <c>agent_id != tester_id</c> (the prod invariant).</summary>
    private static Guid SeedAgent(NetworkerDbContext db, Guid? boundTesterId, string version = "0.28.10")
    {
        var id = Guid.NewGuid();
        db.Agents.Add(new Agent
        {
            AgentId = id,
            Name = $"agent-{id:N}",
            Status = "online",
            Version = version,
            ApiKey = $"key-{id:N}",
            ProjectId = ProjectId,
            RegisteredAt = DateTime.UtcNow,
            TesterId = boundTesterId,
        });
        return id;
    }

    private static Guid SeedConfig(NetworkerDbContext db)
    {
        var id = Guid.NewGuid();
        db.TestConfigs.Add(new TestConfig
        {
            Id = id,
            ProjectId = ProjectId,
            Name = "cfg",
            EndpointKind = "network",
            EndpointRef = """{"kind":"network","host":"127.0.0.1","port":443}""",
            Workload = """{"insecure":false}""",
            CreatedAt = DateTime.UtcNow,
            UpdatedAt = DateTime.UtcNow,
        });
        return id;
    }

    private static AuthUser Caller() => new(
        UserId: Guid.NewGuid(),
        Email: "t@example.com",
        Role: "admin",
        IsPlatformAdmin: true);

    /// <summary>A fixed-key cipher for the dispatcher — these tests use no
    /// sdkprobe tokens, so the key value is irrelevant; it just satisfies the
    /// constructor.</summary>
    private static CredentialCipher TestCipher() =>
        new(new byte[CredentialCipher.KeySize]);

    private static string RunStartedFrame(Guid runId, DateTimeOffset startedAt)
        => System.Text.Json.JsonSerializer.Serialize(
            (AgentMessage)new RunStartedMessage(runId, startedAt));

    // ── 1. Dispatch stamp ────────────────────────────────────────────────────

    [Fact]
    public async Task Dispatch_stamps_worker_id_agent_and_tester_id_bound_tester()
    {
        using var sp = BuildHost(nameof(Dispatch_stamps_worker_id_agent_and_tester_id_bound_tester));
        var db = Db(sp);
        var registry = sp.GetRequiredService<AgentConnectionRegistry>();

        SeedProject(db);
        var testerId = SeedTester(db);
        var agentId = SeedAgent(db, boundTesterId: testerId);
        var configId = SeedConfig(db);
        await db.SaveChangesAsync();

        // Agent is online. Its outbound channel is a no-op sink (send succeeds).
        registry.Register(agentId, $"raw-{agentId}", (_, _) => Task.CompletedTask);

        var dispatcher = new RunDispatcher(
            db, registry,
            sp.GetRequiredService<EventBus>(),
            sp.GetRequiredService<ILogger<RunDispatcher>>(),
            TestCipher());

        // Launch pinned to the project_tester (a project_tester id, NOT an agent id).
        var runId = await dispatcher.LaunchAsync(configId, null, testerId, Caller(), default);

        var run = await db.TestRuns.AsNoTracking().FirstAsync(r => r.Id == runId);

        // worker_id ALWAYS records the executing agent id (as text).
        Assert.Equal(agentId.ToString(), run.WorkerId);
        // tester_id gets the agent's BOUND project_tester — here that IS testerId.
        Assert.Equal(testerId, run.TesterId);
        // The bug guard: agent_id must NEVER end up in tester_id.
        Assert.NotEqual(agentId, run.TesterId);
    }

    [Fact]
    public async Task Dispatch_leaves_tester_id_null_for_standalone_agent()
    {
        using var sp = BuildHost(nameof(Dispatch_leaves_tester_id_null_for_standalone_agent));
        var db = Db(sp);
        var registry = sp.GetRequiredService<AgentConnectionRegistry>();

        SeedProject(db);
        // Standalone agent: bound to NO project_tester (tester_id null).
        var agentId = SeedAgent(db, boundTesterId: null);
        var configId = SeedConfig(db);
        await db.SaveChangesAsync();

        registry.Register(agentId, $"raw-{agentId}", (_, _) => Task.CompletedTask);

        var dispatcher = new RunDispatcher(
            db, registry,
            sp.GetRequiredService<EventBus>(),
            sp.GetRequiredService<ILogger<RunDispatcher>>(),
            TestCipher());

        // Launch with no pinned tester → falls back to the only online agent.
        var runId = await dispatcher.LaunchAsync(configId, null, null, Caller(), default);

        var run = await db.TestRuns.AsNoTracking().FirstAsync(r => r.Id == runId);

        Assert.Equal(agentId.ToString(), run.WorkerId);
        // A standalone agent has no bound tester → tester_id stays NULL,
        // NEVER the agent_id.
        Assert.Null(run.TesterId);
    }

    // ── 2. run_started stamp ─────────────────────────────────────────────────

    [Fact]
    public async Task RunStarted_stamps_worker_id_and_bound_tester_never_agent_id()
    {
        using var sp = BuildHost(nameof(RunStarted_stamps_worker_id_and_bound_tester_never_agent_id));
        var db = Db(sp);

        SeedProject(db);
        var testerId = SeedTester(db);
        var agentId = SeedAgent(db, boundTesterId: testerId);
        var configId = SeedConfig(db);

        var runId = Guid.NewGuid();
        db.TestRuns.Add(new TestRun
        {
            Id = runId,
            TestConfigId = configId,
            ProjectId = ProjectId,
            Status = "queued",
            CreatedAt = DateTime.UtcNow,
        });
        await db.SaveChangesAsync();

        var processor = new AgentMessageProcessor(
            db,
            sp.GetRequiredService<EventBus>(),
            sp.GetRequiredService<ILogger<AgentMessageProcessor>>());

        var frame = RunStartedFrame(runId, DateTimeOffset.UtcNow);
        await processor.HandleFrameAsync(agentId, frame);

        var run = await db.TestRuns.AsNoTracking().FirstAsync(r => r.Id == runId);
        Assert.Equal("running", run.Status);
        Assert.Equal(agentId.ToString(), run.WorkerId);
        Assert.Equal(testerId, run.TesterId);
        Assert.NotEqual(agentId, run.TesterId);
    }

    // ── 3. Disconnect orphan-fail keys on worker_id ──────────────────────────

    [Fact]
    public async Task Disconnect_fails_runs_by_worker_id_not_tester_id()
    {
        using var sp = BuildHost(nameof(Disconnect_fails_runs_by_worker_id_not_tester_id));
        var db = Db(sp);

        SeedProject(db);
        var testerId = SeedTester(db);
        var agentId = SeedAgent(db, boundTesterId: testerId);
        var configId = SeedConfig(db);

        // A running run owned by this agent (worker_id = agentId, tester_id = the
        // bound project_tester).
        var ownedRun = Guid.NewGuid();
        db.TestRuns.Add(new TestRun
        {
            Id = ownedRun,
            TestConfigId = configId,
            ProjectId = ProjectId,
            Status = "running",
            WorkerId = agentId.ToString(),
            TesterId = testerId,
            StartedAt = DateTime.UtcNow,
            CreatedAt = DateTime.UtcNow,
        });

        // A run that merely SHARES the tester_id but is owned by a DIFFERENT
        // worker must NOT be failed — proves we key on worker_id, not tester_id.
        var otherWorkersRun = Guid.NewGuid();
        db.TestRuns.Add(new TestRun
        {
            Id = otherWorkersRun,
            TestConfigId = configId,
            ProjectId = ProjectId,
            Status = "running",
            WorkerId = Guid.NewGuid().ToString(),
            TesterId = testerId,
            StartedAt = DateTime.UtcNow,
            CreatedAt = DateTime.UtcNow,
        });
        await db.SaveChangesAsync();

        var processor = new AgentMessageProcessor(
            db,
            sp.GetRequiredService<EventBus>(),
            sp.GetRequiredService<ILogger<AgentMessageProcessor>>());

        await processor.HandleDisconnectAsync(agentId);

        var owned = await db.TestRuns.AsNoTracking().FirstAsync(r => r.Id == ownedRun);
        var other = await db.TestRuns.AsNoTracking().FirstAsync(r => r.Id == otherWorkersRun);

        Assert.Equal("failed", owned.Status);
        Assert.Equal("running", other.Status); // untouched — different worker
    }

    // ── 4. Watchdog maps run→agent via worker_id ─────────────────────────────

    [Fact]
    public async Task Watchdog_reaps_running_run_whose_worker_is_offline()
    {
        using var sp = BuildHost(nameof(Watchdog_reaps_running_run_whose_worker_is_offline));
        var db = Db(sp);
        var registry = sp.GetRequiredService<AgentConnectionRegistry>();

        SeedProject(db);
        var testerId = SeedTester(db);
        var offlineAgent = SeedAgent(db, boundTesterId: testerId);
        var configId = SeedConfig(db);

        // Stale running run (heartbeat > 120s old) whose worker is NOT online.
        var staleRun = Guid.NewGuid();
        db.TestRuns.Add(new TestRun
        {
            Id = staleRun,
            TestConfigId = configId,
            ProjectId = ProjectId,
            Status = "running",
            WorkerId = offlineAgent.ToString(),
            TesterId = testerId,
            StartedAt = DateTime.UtcNow.AddMinutes(-10),
            LastHeartbeat = DateTime.UtcNow.AddMinutes(-10),
            CreatedAt = DateTime.UtcNow.AddMinutes(-10),
        });
        await db.SaveChangesAsync();

        // offlineAgent is deliberately NOT registered → registry.IsOnline == false.
        await WatchdogTickHarness.RunOnceAsync(sp, registry);

        var run = await db.TestRuns.AsNoTracking().FirstAsync(r => r.Id == staleRun);
        Assert.Equal("failed", run.Status);
    }

    [Fact]
    public async Task Watchdog_spares_running_run_whose_worker_is_online()
    {
        using var sp = BuildHost(nameof(Watchdog_spares_running_run_whose_worker_is_online));
        var db = Db(sp);
        var registry = sp.GetRequiredService<AgentConnectionRegistry>();

        SeedProject(db);
        var testerId = SeedTester(db);
        var liveAgent = SeedAgent(db, boundTesterId: testerId);
        var configId = SeedConfig(db);

        var staleRun = Guid.NewGuid();
        db.TestRuns.Add(new TestRun
        {
            Id = staleRun,
            TestConfigId = configId,
            ProjectId = ProjectId,
            Status = "running",
            WorkerId = liveAgent.ToString(),
            TesterId = testerId,
            StartedAt = DateTime.UtcNow.AddMinutes(-10),
            LastHeartbeat = DateTime.UtcNow.AddMinutes(-10),
            CreatedAt = DateTime.UtcNow.AddMinutes(-10),
        });
        await db.SaveChangesAsync();

        // The worker (mapped via worker_id) is online → must be spared.
        registry.Register(liveAgent, $"raw-{liveAgent}", (_, _) => Task.CompletedTask);

        await WatchdogTickHarness.RunOnceAsync(sp, registry);

        var run = await db.TestRuns.AsNoTracking().FirstAsync(r => r.Id == staleRun);
        Assert.Equal("running", run.Status); // spared — worker still connected
    }
}

/// <summary>
/// Drives one <see cref="WatchdogService"/> reconciliation tick without the 60s
/// PeriodicTimer. The tick body (<c>TickAsync</c>) is private, so we invoke it
/// reflectively — the alternative (making it internal + InternalsVisibleTo)
/// would widen the API surface for a test-only need. A null leader lock makes
/// <c>TryRunGuardedAsync</c> run inline (single-node), which is what a bare test
/// host is.
/// </summary>
internal static class WatchdogTickHarness
{
    public static async Task RunOnceAsync(IServiceProvider sp, AgentConnectionRegistry registry)
    {
        var svc = new WatchdogService(
            sp.GetRequiredService<IServiceScopeFactory>(),
            registry,
            sp.GetRequiredService<EventBus>(),
            sp.GetRequiredService<ILogger<WatchdogService>>());

        var tick = typeof(WatchdogService).GetMethod(
            "TickAsync",
            System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic)!;
        await (Task)tick.Invoke(svc, new object[] { CancellationToken.None })!;
    }
}
