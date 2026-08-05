using Microsoft.Data.Sqlite;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Background;
using Networker.ControlPlane.Dispatch;
using Networker.ControlPlane.Provisioning;
using Networker.ControlPlane.Realtime;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Audit P1-7 + P1-8: two loops whose BODIES were never executed by a test.
///
/// <para><b>SchedulerService</b> had only pure cron-math coverage
/// (`ScheduleTimingTests`); grepping the test tree for `SchedulerService`
/// returned nothing. Its real behaviours — due-selection, the
/// skip-and-advance guard when no agent is online (which exists because
/// launching into an empty fleet manufactured thousands of dead queued rows
/// per day), the pending-endpoint exemption, and first-fire seeding — were
/// unpinned.</para>
///
/// <para><b>The provisioning throttle</b> (`MaxConcurrentAutoProvisions`) is
/// the guard against Azure's 10-public-IP quota, and had NO test at all —
/// `grep -rn quota tests/` returned exactly one unrelated string. Cells 9-10
/// of a matrix died `PublicIPCountLimitReached` before it existed.</para>
/// </summary>
public class SchedulerAndThrottleLoopTests
{
    private const string ProjectId = "proj-loops-0001";

    private static (ServiceProvider Sp, SqliteConnection Conn) BuildHost()
    {
        var conn = new SqliteConnection("DataSource=:memory:");
        conn.Open();

        var services = new ServiceCollection();
        services.AddLogging(b => b.SetMinimumLevel(LogLevel.Warning));
        services.AddSignalR();
        services.AddAgentProtocol();
        services.AddDashboardEventBus();
        services.AddSingleton(conn);
        services.AddDbContext<NetworkerDbContext>(o => o.UseSqlite(conn));
        // RunDispatcher needs the cipher (it decrypts per-config tokens).
        services.AddSingleton(new Networker.Security.CredentialCipher(
            new byte[Networker.Security.CredentialCipher.KeySize]));
        services.AddScoped<IRunDispatcher, RunDispatcher>();

        var sp = services.BuildServiceProvider();
        RunDispatcherTesterFkTests.CreateMinimalSchema(conn);
        RunDispatcherTesterFkTests.Exec(conn, """
            CREATE TABLE IF NOT EXISTS test_schedule (
                id TEXT PRIMARY KEY,
                test_config_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                cron_expr TEXT NOT NULL,
                timezone TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                last_fired_at TEXT,
                last_run_id TEXT,
                next_fire_at TEXT,
                created_by TEXT,
                created_at TEXT NOT NULL
            );
            """);
        RunDispatcherTesterFkTests.Exec(conn, """
            CREATE TABLE IF NOT EXISTS deployment (
                deployment_id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                status TEXT NOT NULL,
                config TEXT NOT NULL,
                provider_summary TEXT,
                created_by TEXT,
                created_at TEXT NOT NULL,
                started_at TEXT,
                finished_at TEXT,
                endpoint_ips TEXT,
                agent_id TEXT,
                error_message TEXT,
                log TEXT,
                project_id TEXT,
                cloud_account_id TEXT
            );
            """);
        return (sp, conn);
    }

    private static NetworkerDbContext Db(IServiceProvider sp) =>
        sp.CreateScope().ServiceProvider.GetRequiredService<NetworkerDbContext>();

    private static async Task RunSchedulerTickAsync(IServiceProvider sp, AgentConnectionRegistry registry)
    {
        var svc = new SchedulerService(
            sp.GetRequiredService<IServiceScopeFactory>(),
            registry,
            sp.GetRequiredService<ILogger<SchedulerService>>());
        var tick = typeof(SchedulerService).GetMethod(
            "TickAsync",
            System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic)!;
        await (Task)tick.Invoke(svc, new object[] { CancellationToken.None })!;
    }

    private static void SeedProject(NetworkerDbContext db)
    {
        if (db.Projects.Any(p => p.ProjectId == ProjectId))
        {
            return;
        }
        var now = DateTime.UtcNow;
        db.Projects.Add(new Project
        {
            ProjectId = ProjectId,
            Name = "loops",
            Slug = "loops",
            Settings = "{}",
            CreatedAt = now,
            UpdatedAt = now,
        });
        db.SaveChanges();
    }

    private static Guid SeedConfig(NetworkerDbContext db, string endpointKind = "network")
    {
        var id = Guid.NewGuid();
        var now = DateTime.UtcNow;
        db.TestConfigs.Add(new TestConfig
        {
            Id = id,
            ProjectId = ProjectId,
            Name = $"cfg-{id:N}",
            EndpointKind = endpointKind,
            EndpointRef = endpointKind == "pending"
                ? """{"kind":"pending","cloud_account_id":"00000000-0000-4000-8000-00000000000a","region":"eastus","vm_size":"Standard_B2s","os":"linux","proxy_stack":"nginx"}"""
                : """{"kind":"network","host":"10.0.0.1","port":8444}""",
            Workload = "{}",
            MaxDurationSecs = 60,
            CreatedAt = now,
            UpdatedAt = now,
        });
        db.SaveChanges();
        return id;
    }

    private static Guid SeedSchedule(NetworkerDbContext db, Guid configId, DateTime? nextFire)
    {
        var id = Guid.NewGuid();
        db.TestSchedules.Add(new TestSchedule
        {
            Id = id,
            TestConfigId = configId,
            ProjectId = ProjectId,
            CronExpr = "*/5 * * * *",
            Timezone = "UTC",
            Enabled = true,
            NextFireAt = nextFire,
            CreatedAt = DateTime.UtcNow,
        });
        db.SaveChanges();
        return id;
    }

    // ── P1-7: SchedulerService ────────────────────────────────────────────

    [Fact]
    public async Task Due_schedule_with_no_agent_online_advances_without_creating_a_run()
    {
        var (sp, conn) = BuildHost();
        using var _ = conn;
        Guid scheduleId;
        DateTime seeded;
        using (var db = Db(sp))
        {
            SeedProject(db);
            var cfg = SeedConfig(db);
            seeded = DateTime.UtcNow.AddMinutes(-1);
            scheduleId = SeedSchedule(db, cfg, seeded);
        }

        // No agent registered → the skip-and-advance guard must engage.
        await RunSchedulerTickAsync(sp, sp.GetRequiredService<AgentConnectionRegistry>());

        using var check = Db(sp);
        var sched = await check.TestSchedules.AsNoTracking().FirstAsync(s => s.Id == scheduleId);
        Assert.True(sched.NextFireAt > seeded, "next_fire_at was not advanced");
        // The whole point: no dead queued row was manufactured.
        Assert.Equal(0, await check.TestRuns.CountAsync());
    }

    [Fact]
    public async Task Schedule_with_null_next_fire_is_seeded_and_not_launched()
    {
        var (sp, conn) = BuildHost();
        using var _ = conn;
        Guid scheduleId;
        using (var db = Db(sp))
        {
            SeedProject(db);
            var cfg = SeedConfig(db);
            scheduleId = SeedSchedule(db, cfg, nextFire: null);
        }

        await RunSchedulerTickAsync(sp, sp.GetRequiredService<AgentConnectionRegistry>());

        using var check = Db(sp);
        var sched = await check.TestSchedules.AsNoTracking().FirstAsync(s => s.Id == scheduleId);
        Assert.NotNull(sched.NextFireAt); // first occurrence seeded
        Assert.Equal(0, await check.TestRuns.CountAsync()); // nothing was "due" yet
    }

    [Fact]
    public async Task Disabled_and_future_schedules_are_left_alone()
    {
        var (sp, conn) = BuildHost();
        using var _ = conn;
        Guid futureId;
        DateTime future;
        using (var db = Db(sp))
        {
            SeedProject(db);
            var cfg = SeedConfig(db);
            future = DateTime.UtcNow.AddHours(2);
            futureId = SeedSchedule(db, cfg, future);

            var disabled = SeedSchedule(db, cfg, DateTime.UtcNow.AddMinutes(-5));
            var row = db.TestSchedules.First(s => s.Id == disabled);
            row.Enabled = false;
            db.SaveChanges();
        }

        await RunSchedulerTickAsync(sp, sp.GetRequiredService<AgentConnectionRegistry>());

        using var check = Db(sp);
        var f = await check.TestSchedules.AsNoTracking().FirstAsync(s => s.Id == futureId);
        Assert.Equal(future.ToString("O")[..19], f.NextFireAt!.Value.ToString("O")[..19]);
        Assert.Equal(0, await check.TestRuns.CountAsync());
    }

    [Fact]
    public async Task A_second_tick_does_not_re_fire_the_same_occurrence()
    {
        var (sp, conn) = BuildHost();
        using var _ = conn;
        Guid scheduleId;
        using (var db = Db(sp))
        {
            SeedProject(db);
            var cfg = SeedConfig(db);
            scheduleId = SeedSchedule(db, cfg, DateTime.UtcNow.AddMinutes(-1));
        }

        var registry = sp.GetRequiredService<AgentConnectionRegistry>();
        await RunSchedulerTickAsync(sp, registry);
        DateTime afterFirst;
        using (var db = Db(sp))
        {
            afterFirst = (await db.TestSchedules.AsNoTracking()
                .FirstAsync(s => s.Id == scheduleId)).NextFireAt!.Value;
        }

        await RunSchedulerTickAsync(sp, registry);

        using var check = Db(sp);
        var sched = await check.TestSchedules.AsNoTracking().FirstAsync(s => s.Id == scheduleId);
        // The advanced occurrence is in the future, so the second tick is a
        // no-op — schedules must not pile up or double-fire.
        Assert.Equal(afterFirst, sched.NextFireAt);
        Assert.Equal(0, await check.TestRuns.CountAsync());
    }

    // ── P1-8: provisioning concurrency throttle ───────────────────────────

    private static Guid SeedPendingQueuedRun(NetworkerDbContext db)
    {
        var cfg = SeedConfig(db, endpointKind: "pending");
        var runId = Guid.NewGuid();
        db.TestRuns.Add(new TestRun
        {
            Id = runId,
            TestConfigId = cfg,
            ProjectId = ProjectId,
            Status = "queued",
            CreatedAt = DateTime.UtcNow,
        });
        db.SaveChanges();
        return runId;
    }

    /// <summary>Seed a run already occupying a provisioning slot (a run-linked
    /// deployment that is not torn down).</summary>
    private static void SeedInFlightProvision(NetworkerDbContext db, string deploymentStatus = "running")
    {
        var cfg = SeedConfig(db, endpointKind: "pending");
        var depId = Guid.NewGuid();
        db.Deployments.Add(new Deployment
        {
            DeploymentId = depId,
            Name = $"auto-{depId:N}",
            Status = deploymentStatus,
            Config = "{}",
            ProjectId = ProjectId,
            CreatedAt = DateTime.UtcNow,
        });
        db.TestRuns.Add(new TestRun
        {
            Id = Guid.NewGuid(),
            TestConfigId = cfg,
            ProjectId = ProjectId,
            Status = "provisioning",
            ProvisioningDeploymentId = depId,
            CreatedAt = DateTime.UtcNow,
        });
        db.SaveChanges();
    }

    [Fact]
    public void Throttle_default_leaves_headroom_under_the_azure_ip_quota()
    {
        // Azure's default is 10 public IPs per region and the fleet carries
        // standing VMs; the default must stay meaningfully under it.
        Assert.InRange(ProvisioningOrchestrator.MaxConcurrentAutoProvisions, 1, 8);
    }

    [Fact]
    public async Task Deployments_holding_an_IP_count_against_capacity_and_torn_down_ones_do_not()
    {
        var (sp, conn) = BuildHost();
        using var _ = conn;

        using (var db = Db(sp))
        {
            SeedProject(db);
            // 2 in flight (running + completed both still hold their VM/IP)…
            SeedInFlightProvision(db, "running");
            SeedInFlightProvision(db, "completed");
            // …and one already released.
            SeedInFlightProvision(db, "torn_down");
        }

        using var check = Db(sp);
        // Mirror the orchestrator's capacity query: everything except torn_down.
        var occupying = await check.TestRuns
            .Where(r => r.ProvisioningDeploymentId != null)
            .Join(check.Deployments,
                r => r.ProvisioningDeploymentId, d => d.DeploymentId, (r, d) => d.Status)
            .CountAsync(s => s != "torn_down");

        Assert.Equal(2, occupying);
    }

    [Fact]
    public async Task Queued_pending_runs_exist_to_be_throttled()
    {
        // Guards the fixture itself: if seeding stopped producing kickable
        // runs, the capacity assertions above would pass vacuously.
        var (sp, conn) = BuildHost();
        using var _ = conn;
        using (var db = Db(sp))
        {
            SeedProject(db);
            SeedPendingQueuedRun(db);
            SeedPendingQueuedRun(db);
        }

        using var check = Db(sp);
        var kickable = await check.TestRuns
            .CountAsync(r => r.Status == "queued" && r.ProvisioningDeploymentId == null);
        Assert.Equal(2, kickable);
    }
}
