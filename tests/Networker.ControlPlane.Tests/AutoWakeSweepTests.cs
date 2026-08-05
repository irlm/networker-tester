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
}
