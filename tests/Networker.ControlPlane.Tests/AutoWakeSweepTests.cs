using Microsoft.Data.Sqlite;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Background;
using Networker.ControlPlane.Provisioning;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// P0-6 (2026-08 audit): the auto-wake state machine (v0.28.140) shipped with
/// zero tests. These pin the wake arm of <see cref="AutoShutdownService"/>:
/// a stopped/deallocated tester with QUEUED runs assigned is started; the
/// claim is a guarded update (power_state → 'starting'); a StartAsync failure
/// rolls the state back for retry; and the watchdog's queued-run reaping
/// holds while any tester is 'starting' (covered in WatchdogService via the
/// anyWaking guard — asserted here through the same sweep-side state).
/// </summary>
public class AutoWakeSweepTests
{
    private const string ProjectId = "proj-wake-00001";

    private sealed class FakeProvisioner : IComputeProvisioner
    {
        public int StartCalls;
        public Func<ProvisionResult> StartBehavior = () => new ProvisionResult(true, 0, "", "");

        public Task<ProvisionResult> StartAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default)
        {
            StartCalls++;
            var result = StartBehavior();
            return Task.FromResult(result);
        }

        public Task<ProvisionResult> StopAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default)
            => Task.FromResult(new ProvisionResult(true, 0, "", ""));

        public Task<ProvisionResult> DeallocateAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default)
            => Task.FromResult(new ProvisionResult(true, 0, "", ""));

        public Task<ProvisionResult> DeleteAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default)
            => Task.FromResult(new ProvisionResult(true, 0, "", ""));

        public Task<ProvisionResult> ShowAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default)
            => Task.FromResult(new ProvisionResult(true, 0, "", ""));

        public Task<ProvisionResult> RunCommandAsync(ProjectTester tester, ProviderCredentials? credentials, string script, CancellationToken ct = default)
            => Task.FromResult(new ProvisionResult(true, 0, "", ""));

        public Task<VmCreateResult> CreateVmAsync(VmCreateRequest request, ProviderCredentials? credentials, CancellationToken ct = default)
            => throw new NotSupportedException();

        public Task<ResolvedVm?> ResolveByEndpointAsync(string provider, ProviderCredentials? credentials, string endpoint, CancellationToken ct = default)
            => Task.FromResult<ResolvedVm?>(null);
    }

    private static (ServiceProvider Sp, SqliteConnection Conn, FakeProvisioner Prov) BuildHost(string name)
    {
        // Kept-open in-memory connection so the schema survives across the DI
        // scopes the sweep opens. The full Postgres model can't be built on
        // Sqlite (Timescale sequence), so reuse the shared minimal schema that
        // RunDispatcherTesterFkTests maintains with real column names.
        var conn = new SqliteConnection("DataSource=:memory:");
        conn.Open();

        var prov = new FakeProvisioner();
        var services = new ServiceCollection();
        services.AddLogging();
        services.AddDbContext<NetworkerDbContext>(o => o.UseSqlite(conn));
        services.AddSingleton<IComputeProvisioner>(prov);

        var sp = services.BuildServiceProvider();
        RunDispatcherTesterFkTests.CreateMinimalSchema(conn);
        return (sp, conn, prov);
    }

    private static NetworkerDbContext Db(IServiceProvider sp) =>
        sp.CreateScope().ServiceProvider.GetRequiredService<NetworkerDbContext>();

    private static async Task RunSweepOnceAsync(IServiceProvider sp)
    {
        var svc = new AutoShutdownService(
            sp.GetRequiredService<IServiceScopeFactory>(),
            sp.GetRequiredService<ILogger<AutoShutdownService>>());
        var sweep = typeof(AutoShutdownService).GetMethod(
            "SweepAsync",
            System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic)!;
        await (Task)sweep.Invoke(svc, new object[] { CancellationToken.None })!;
    }

    private static Guid SeedStoppedTesterWithQueuedRun(NetworkerDbContext db, string powerState = "stopped")
    {
        var now = DateTime.UtcNow;
        if (!db.Projects.Any(p => p.ProjectId == ProjectId))
        {
            db.Projects.Add(new Project
            {
                ProjectId = ProjectId,
                Name = "wake",
                Slug = "wake",
                Settings = "{}",
                CreatedAt = now,
                UpdatedAt = now,
            });
        }
        var testerId = Guid.NewGuid();
        db.ProjectTesters.Add(new ProjectTester
        {
            TesterId = testerId,
            ProjectId = ProjectId,
            Name = $"wake-{testerId:N}",
            Cloud = "azure",
            Region = "eastus",
            VmSize = "Standard_B2s",
            SshUser = "azureuser",
            PowerState = powerState,
            Allocation = "idle",
            AutoShutdownEnabled = true,
            AutoShutdownLocalHour = 23,
            ShutdownDeferralCount = 0,
            AutoProbeEnabled = false,
            BenchmarkRunCount = 0,
            CreatedBy = Guid.NewGuid(),
            CreatedAt = now,
            UpdatedAt = now,
        });
        var configId = Guid.NewGuid();
        db.TestConfigs.Add(new TestConfig
        {
            Id = configId,
            ProjectId = ProjectId,
            Name = $"wake-cfg-{configId:N}",
            EndpointKind = "network",
            EndpointRef = "{}",
            Workload = "{}",
            MaxDurationSecs = 60,
            CreatedAt = now,
            UpdatedAt = now,
        });
        db.TestRuns.Add(new TestRun
        {
            Id = Guid.NewGuid(),
            TestConfigId = configId,
            ProjectId = ProjectId,
            Status = "queued",
            TesterId = testerId,
            CreatedAt = now,
        });
        db.SaveChanges();
        return testerId;
    }

    [Theory]
    [InlineData("stopped")]
    [InlineData("deallocated")]
    public async Task Stopped_tester_with_queued_run_is_woken(string powerState)
    {
        var (sp, conn, prov) = BuildHost(nameof(Stopped_tester_with_queued_run_is_woken) + powerState);
        using var _ = conn;
        Guid testerId;
        using (var db = Db(sp))
        {
            testerId = SeedStoppedTesterWithQueuedRun(db, powerState);
        }

        await RunSweepOnceAsync(sp);

        using var check = Db(sp);
        var tester = await check.ProjectTesters.AsNoTracking().FirstAsync(t => t.TesterId == testerId);
        Assert.Equal(1, prov.StartCalls);
        // Success leaves 'starting'; the heartbeat reconcile completes the flip.
        Assert.Equal("starting", tester.PowerState);
    }

    [Fact]
    public async Task Start_failure_rolls_power_state_back_for_retry()
    {
        var (sp, conn, prov) = BuildHost(nameof(Start_failure_rolls_power_state_back_for_retry));
        using var _ = conn;
        prov.StartBehavior = () => new ProvisionResult(false, 1, "", "az exploded");
        Guid testerId;
        using (var db = Db(sp))
        {
            testerId = SeedStoppedTesterWithQueuedRun(db);
        }

        await RunSweepOnceAsync(sp);

        using var check = Db(sp);
        var tester = await check.ProjectTesters.AsNoTracking().FirstAsync(t => t.TesterId == testerId);
        Assert.Equal(1, prov.StartCalls);
        // Genuine CLI failure (non-null exit code) → rolled back so the next
        // tick retries; NOT left wedged in 'starting'.
        Assert.Equal("stopped", tester.PowerState);
    }

    [Fact]
    public async Task Cli_less_host_soft_failure_still_counts_as_started()
    {
        var (sp, conn, prov) = BuildHost(nameof(Cli_less_host_soft_failure_still_counts_as_started));
        using var _ = conn;
        // Missing cloud CLI: Success=false with ExitCode=null — same
        // convergence posture as the deallocate path.
        prov.StartBehavior = () => new ProvisionResult(false, null, "", "");
        Guid testerId;
        using (var db = Db(sp))
        {
            testerId = SeedStoppedTesterWithQueuedRun(db);
        }

        await RunSweepOnceAsync(sp);

        using var check = Db(sp);
        var tester = await check.ProjectTesters.AsNoTracking().FirstAsync(t => t.TesterId == testerId);
        Assert.Equal("starting", tester.PowerState);
    }

    [Fact]
    public async Task Running_tester_with_queued_run_is_not_touched()
    {
        var (sp, conn, prov) = BuildHost(nameof(Running_tester_with_queued_run_is_not_touched));
        using var _ = conn;
        Guid testerId;
        using (var db = Db(sp))
        {
            testerId = SeedStoppedTesterWithQueuedRun(db, powerState: "running");
        }

        await RunSweepOnceAsync(sp);

        using var check = Db(sp);
        var tester = await check.ProjectTesters.AsNoTracking().FirstAsync(t => t.TesterId == testerId);
        Assert.Equal(0, prov.StartCalls);
        Assert.Equal("running", tester.PowerState);
    }

    [Fact]
    public async Task Stopped_tester_without_queued_work_stays_stopped()
    {
        var (sp, conn, prov) = BuildHost(nameof(Stopped_tester_without_queued_work_stays_stopped));
        using var _ = conn;
        Guid testerId;
        using (var db = Db(sp))
        {
            testerId = SeedStoppedTesterWithQueuedRun(db);
            // Remove the queued run — no work, no wake.
            var run = db.TestRuns.First(r => r.TesterId == testerId);
            db.TestRuns.Remove(run);
            db.SaveChanges();
        }

        await RunSweepOnceAsync(sp);

        using var check = Db(sp);
        var tester = await check.ProjectTesters.AsNoTracking().FirstAsync(t => t.TesterId == testerId);
        Assert.Equal(0, prov.StartCalls);
        Assert.Equal("stopped", tester.PowerState);
    }
    // ── Unpinned arm (2026-08-11: URL-probe class — auto-pick runs carry no
    // TesterId, so the pinned arm never woke anything and the probe sat
    // queued to watchdog death) ──────────────────────────────────────────

    private static void SeedUnpinnedQueuedRun(NetworkerDbContext db, string projectId)
    {
        var now = DateTime.UtcNow;
        if (!db.Projects.Any(p => p.ProjectId == projectId))
        {
            db.Projects.Add(new Project
            {
                ProjectId = projectId,
                Name = "wake-u",
                Slug = "wake-u-" + projectId[^4..],
                Settings = "{}",
                CreatedAt = now,
                UpdatedAt = now,
            });
        }
        var configId = Guid.NewGuid();
        db.TestConfigs.Add(new TestConfig
        {
            Id = configId,
            ProjectId = projectId,
            Name = $"probe-cfg-{configId:N}",
            EndpointKind = "network",
            EndpointRef = "{}",
            Workload = "{}",
            MaxDurationSecs = 60,
            CreatedAt = now,
            UpdatedAt = now,
        });
        db.TestRuns.Add(new TestRun
        {
            Id = Guid.NewGuid(),
            TestConfigId = configId,
            ProjectId = projectId,
            Status = "queued",
            TesterId = null, // auto-pick — the incident shape
            CreatedAt = now,
        });
        db.SaveChanges();
    }

    private static Guid SeedIdleStoppedTester(NetworkerDbContext db, string projectId, string name = "u-tester")
    {
        var now = DateTime.UtcNow;
        var testerId = Guid.NewGuid();
        db.ProjectTesters.Add(new ProjectTester
        {
            TesterId = testerId,
            ProjectId = projectId,
            Name = name,
            Cloud = "azure",
            Region = "eastus",
            VmSize = "Standard_B2s",
            SshUser = "azureuser",
            PowerState = "stopped",
            Allocation = "idle",
            AutoShutdownEnabled = true,
            AutoShutdownLocalHour = 23,
            ShutdownDeferralCount = 0,
            AutoProbeEnabled = false,
            BenchmarkRunCount = 0,
            CreatedBy = Guid.NewGuid(),
            CreatedAt = now,
            UpdatedAt = now,
        });
        db.SaveChanges();
        return testerId;
    }

    [Fact]
    public async Task Unpinned_queued_run_with_no_online_agent_wakes_one_idle_tester()
    {
        const string pid = "proj-wake-unpin1";
        var (sp, conn, prov) = BuildHost(nameof(Unpinned_queued_run_with_no_online_agent_wakes_one_idle_tester));
        using var _ = conn;
        Guid testerId;
        using (var db = Db(sp))
        {
            SeedUnpinnedQueuedRun(db, pid);
            testerId = SeedIdleStoppedTester(db, pid, "aaa-first");
            SeedIdleStoppedTester(db, pid, "bbb-second");
        }

        await RunSweepOnceAsync(sp);

        Assert.Equal(1, prov.StartCalls); // exactly ONE tester woken, not the fleet
        using (var db = Db(sp))
        {
            Assert.Equal("starting", db.ProjectTesters.Single(t => t.TesterId == testerId).PowerState);
            Assert.Equal("stopped", db.ProjectTesters.Single(t => t.Name == "bbb-second").PowerState);
        }
    }

    [Fact]
    public async Task Unpinned_queued_run_with_an_online_agent_does_not_wake()
    {
        const string pid = "proj-wake-unpin2";
        var (sp, conn, prov) = BuildHost(nameof(Unpinned_queued_run_with_an_online_agent_does_not_wake));
        using var _ = conn;
        using (var db = Db(sp))
        {
            SeedUnpinnedQueuedRun(db, pid);
            SeedIdleStoppedTester(db, pid);
            db.Agents.Add(new Agent
            {
                AgentId = Guid.NewGuid(),
                ProjectId = pid,
                Name = "online-agent",
                Status = "online",
                RegisteredAt = DateTime.UtcNow,
            });
            db.SaveChanges();
        }

        await RunSweepOnceAsync(sp);

        Assert.Equal(0, prov.StartCalls); // dispatch will handle it; no wake
    }

    [Fact]
    public async Task Unpinned_wake_skipped_while_another_tester_is_already_starting()
    {
        const string pid = "proj-wake-unpin3";
        var (sp, conn, prov) = BuildHost(nameof(Unpinned_wake_skipped_while_another_tester_is_already_starting));
        using var _ = conn;
        using (var db = Db(sp))
        {
            SeedUnpinnedQueuedRun(db, pid);
            SeedIdleStoppedTester(db, pid);
            var starting = SeedIdleStoppedTester(db, pid, "already-starting");
            db.ProjectTesters.Single(t => t.TesterId == starting).PowerState = "starting";
            db.SaveChanges();
        }

        await RunSweepOnceAsync(sp);

        Assert.Equal(0, prov.StartCalls); // a wake is already in flight
    }

    // ── Stuck 'starting' ──────────────────────────────────────────────────
    // 'starting' has no self-imposed exit: only the agent's heartbeat promotes
    // it to 'running'. A runner whose agent never connects therefore sat in
    // 'starting' forever — visible on prod 2026-08-24 as a runner permanently
    // "starting", which ALSO made the watchdog treat a wake as permanently in
    // flight and suppressed stuck-queued-run reaping deployment-wide.

    /// <summary>Age a tester's row so the sweep sees it as long-stuck.</summary>
    private static void BackdateTester(NetworkerDbContext db, Guid testerId, TimeSpan age)
    {
        var t = db.ProjectTesters.First(x => x.TesterId == testerId);
        t.PowerState = "starting";
        t.StatusMessage = "auto-shutdown completed";   // the stale message from the prod report
        t.UpdatedAt = DateTime.UtcNow - age;
        db.SaveChanges();
    }

    [Fact]
    public async Task A_wake_that_never_completed_is_released_back_to_stopped()
    {
        var (sp, conn, _) = BuildHost(nameof(A_wake_that_never_completed_is_released_back_to_stopped));
        using var _c = conn;
        Guid testerId;
        using (var db = Db(sp))
        {
            testerId = SeedStoppedTesterWithQueuedRun(db);
            // Drop the queued run: with work still pending the wake arm further
            // down the SAME sweep immediately restarts it (asserted separately
            // in A_released_runner_is_woken_again_in_the_same_sweep). Removing it
            // isolates the release itself.
            db.TestRuns.RemoveRange(db.TestRuns.Where(r => r.TesterId == testerId));
            db.SaveChanges();
            BackdateTester(db, testerId, AutoShutdownService.StuckStartingTimeout + TimeSpan.FromMinutes(5));
        }

        await RunSweepOnceAsync(sp);

        using var check = Db(sp);
        var tester = await check.ProjectTesters.AsNoTracking().FirstAsync(t => t.TesterId == testerId);
        // 'stopped', not 'error': this is the state a manual start accepts, so a
        // transient wake failure must not leave a runner nobody can start.
        Assert.Equal("stopped", tester.PowerState);
        Assert.Contains("agent never connected", tester.StatusMessage ?? "");
        // And the stale message that contradicted the badge is gone.
        Assert.DoesNotContain("auto-shutdown completed", tester.StatusMessage ?? "");
    }

    [Fact]
    public async Task A_wake_still_within_the_grace_window_is_left_alone()
    {
        // The guard that keeps the release from cancelling healthy boots. A cold
        // VM plus agent connect legitimately takes minutes.
        var (sp, conn, _) = BuildHost(nameof(A_wake_still_within_the_grace_window_is_left_alone));
        using var _c = conn;
        Guid testerId;
        using (var db = Db(sp))
        {
            testerId = SeedStoppedTesterWithQueuedRun(db);
            BackdateTester(db, testerId, AutoShutdownService.StuckStartingTimeout - TimeSpan.FromMinutes(5));
        }

        await RunSweepOnceAsync(sp);

        using var check = Db(sp);
        var tester = await check.ProjectTesters.AsNoTracking().FirstAsync(t => t.TesterId == testerId);
        Assert.Equal("starting", tester.PowerState);
    }

    [Fact]
    public async Task A_released_runner_is_woken_again_in_the_same_sweep()
    {
        // The release runs BEFORE the wake arms precisely so a stuck runner with
        // outstanding work recovers in one tick instead of waiting for the next.
        var (sp, conn, prov) = BuildHost(nameof(A_released_runner_is_woken_again_in_the_same_sweep));
        using var _c = conn;
        Guid testerId;
        using (var db = Db(sp))
        {
            testerId = SeedStoppedTesterWithQueuedRun(db);   // queued run still pending
            BackdateTester(db, testerId, AutoShutdownService.StuckStartingTimeout + TimeSpan.FromMinutes(5));
        }

        await RunSweepOnceAsync(sp);

        using var check = Db(sp);
        var tester = await check.ProjectTesters.AsNoTracking().FirstAsync(t => t.TesterId == testerId);
        Assert.Equal(1, prov.StartCalls);
        Assert.Equal("starting", tester.PowerState);
    }

    [Fact]
    public async Task Auto_wake_stamps_its_own_status_message()
    {
        // The badge and the message must agree. Before this, the wake set only
        // power_state, so a woken runner read "starting" beside the PREVIOUS
        // lifecycle's "auto-shutdown completed".
        var (sp, conn, _) = BuildHost(nameof(Auto_wake_stamps_its_own_status_message));
        using var _c = conn;
        Guid testerId;
        using (var db = Db(sp))
        {
            testerId = SeedStoppedTesterWithQueuedRun(db);
            var t = db.ProjectTesters.First(x => x.TesterId == testerId);
            t.StatusMessage = "auto-shutdown completed";
            db.SaveChanges();
        }

        await RunSweepOnceAsync(sp);

        using var check = Db(sp);
        var tester = await check.ProjectTesters.AsNoTracking().FirstAsync(t => t.TesterId == testerId);
        Assert.Equal("starting", tester.PowerState);
        Assert.StartsWith("auto-wake:", tester.StatusMessage ?? "");
        Assert.DoesNotContain("auto-shutdown completed", tester.StatusMessage ?? "");
    }

    [Fact]
    public async Task A_failed_wake_replaces_the_message_too_when_it_rolls_back()
    {
        var (sp, conn, prov) = BuildHost(nameof(A_failed_wake_replaces_the_message_too_when_it_rolls_back));
        using var _c = conn;
        prov.StartBehavior = () => new ProvisionResult(false, 1, "", "az exploded");
        Guid testerId;
        using (var db = Db(sp))
        {
            testerId = SeedStoppedTesterWithQueuedRun(db);
        }

        await RunSweepOnceAsync(sp);

        using var check = Db(sp);
        var tester = await check.ProjectTesters.AsNoTracking().FirstAsync(t => t.TesterId == testerId);
        Assert.Equal("stopped", tester.PowerState);
        Assert.Contains("auto-wake failed", tester.StatusMessage ?? "");
    }
}
