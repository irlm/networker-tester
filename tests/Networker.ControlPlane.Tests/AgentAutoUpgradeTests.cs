using Microsoft.Data.Sqlite;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Background;
using Networker.ControlPlane.Endpoints;
using Networker.ControlPlane.Provisioning;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pins the runner auto-upgrade sweep (v0.28.198) — the fix for the gap found
/// 2026-08-12: nothing ever upgraded deployed runners, so the eastus agent sat
/// on 0.28.126 for ~70 releases and its probes were missing a month of
/// measurement fixes. The sweep must (a) reinstall stale ONLINE agents' idle
/// azure runners via run-command, (b) leave current/busy/non-azure/offline
/// runners alone, (c) cap upgrades per sweep, and (d) back off after failures
/// instead of hammering a wedged VM every tick.
/// </summary>
public class AgentAutoUpgradeTests
{
    private const string ProjectId = "proj-upg-000001";

    /// <summary>A version guaranteed stale relative to the compile-time floor.</summary>
    private const string StaleVersion = "0.1.0";

    private sealed class FakeProvisioner : IComputeProvisioner
    {
        public readonly List<string> RunCommandScripts = new();
        public Func<ProvisionResult> RunCommandBehavior = () => new ProvisionResult(true, 0, "networker-tester x.y.z", "");

        public Task<ProvisionResult> RunCommandAsync(ProjectTester tester, ProviderCredentials? credentials, string script, CancellationToken ct = default)
        {
            RunCommandScripts.Add(script);
            return Task.FromResult(RunCommandBehavior());
        }

        public Task<ProvisionResult> StartAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default)
            => Task.FromResult(new ProvisionResult(true, 0, "", ""));

        public Task<ProvisionResult> StopAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default)
            => Task.FromResult(new ProvisionResult(true, 0, "", ""));

        public Task<ProvisionResult> DeallocateAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default)
            => Task.FromResult(new ProvisionResult(true, 0, "", ""));

        public Task<ProvisionResult> DeleteAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default)
            => Task.FromResult(new ProvisionResult(true, 0, "", ""));

        public Task<ProvisionResult> ShowAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default)
            => Task.FromResult(new ProvisionResult(true, 0, "", ""));

        public Task<VmCreateResult> CreateVmAsync(VmCreateRequest request, ProviderCredentials? credentials, CancellationToken ct = default)
            => throw new NotSupportedException();

        public Task<ResolvedVm?> ResolveByEndpointAsync(string provider, ProviderCredentials? credentials, string endpoint, CancellationToken ct = default)
            => Task.FromResult<ResolvedVm?>(null);
    }

    private static (ServiceProvider Sp, SqliteConnection Conn, FakeProvisioner Prov) BuildHost()
    {
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

    private static AgentAutoUpgradeService Service(IServiceProvider sp) => new(
        sp.GetRequiredService<IServiceScopeFactory>(),
        sp.GetRequiredService<ILogger<AgentAutoUpgradeService>>());

    /// <summary>Seed a runner (default: azure, running, idle) with an online
    /// agent reporting <paramref name="agentVersion"/>. Returns the tester id.</summary>
    private static Guid SeedRunner(
        NetworkerDbContext db,
        string agentVersion = StaleVersion,
        string cloud = "azure",
        string powerState = "running",
        string agentStatus = "online",
        string name = "runner")
    {
        var now = DateTime.UtcNow;
        if (!db.Projects.Any(p => p.ProjectId == ProjectId))
        {
            db.Projects.Add(new Project
            {
                ProjectId = ProjectId,
                Name = "upg",
                Slug = "upg",
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
            Name = $"{name}-{testerId:N}",
            Cloud = cloud,
            Region = "eastus",
            VmSize = "Standard_B2s",
            SshUser = "azureuser",
            PowerState = powerState,
            Allocation = "idle",
            InstallerVersion = agentVersion,
            AutoShutdownEnabled = false,
            AutoShutdownLocalHour = 23,
            ShutdownDeferralCount = 0,
            AutoProbeEnabled = false,
            BenchmarkRunCount = 0,
            CreatedBy = Guid.NewGuid(),
            CreatedAt = now,
            UpdatedAt = now,
        });
        db.Agents.Add(new Agent
        {
            AgentId = Guid.NewGuid(),
            Name = $"agent-{testerId:N}",
            Status = agentStatus,
            Version = agentVersion,
            RegisteredAt = now,
            LastHeartbeat = now,
            ProjectId = ProjectId,
            TesterId = testerId,
        });
        db.SaveChanges();
        return testerId;
    }

    private static void SeedInFlightRun(NetworkerDbContext db, Guid testerId, string status = "running")
    {
        var now = DateTime.UtcNow;
        var configId = Guid.NewGuid();
        db.TestConfigs.Add(new TestConfig
        {
            Id = configId,
            ProjectId = ProjectId,
            Name = $"cfg-{configId:N}",
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
            Status = status,
            TesterId = testerId,
            CreatedAt = now,
        });
        db.SaveChanges();
    }

    [Fact]
    public async Task stale_online_agent_gets_reinstalled_and_row_records_it()
    {
        var (sp, conn, prov) = BuildHost();
        using var _ = conn;
        Guid testerId;
        using (var db = Db(sp))
        {
            testerId = SeedRunner(db);
        }

        await Service(sp).SweepAsync(CancellationToken.None);

        var script = Assert.Single(prov.RunCommandScripts);
        Assert.Contains(
            TesterInstallScripts.PreferredReleaseTag(VersionEndpoints.DashboardVersion), script);
        Assert.Contains("systemctl restart networker-agent", script);

        using var check = Db(sp);
        var tester = check.ProjectTesters.Single(t => t.TesterId == testerId);
        Assert.Equal("running", tester.PowerState);
        Assert.Contains("Auto-upgraded", tester.StatusMessage);
        Assert.Equal(VersionEndpoints.DashboardVersion, tester.InstallerVersion);
        Assert.NotNull(tester.LastInstalledAt);
    }

    [Fact]
    public async Task agent_at_floor_version_is_left_alone()
    {
        var (sp, conn, prov) = BuildHost();
        using var _ = conn;
        using (var db = Db(sp))
        {
            SeedRunner(db, agentVersion: VersionEndpoints.DashboardVersion);
        }

        await Service(sp).SweepAsync(CancellationToken.None);

        Assert.Empty(prov.RunCommandScripts);
    }

    [Theory]
    [InlineData("queued")]
    [InlineData("running")]
    public async Task busy_tester_is_never_upgraded(string runStatus)
    {
        var (sp, conn, prov) = BuildHost();
        using var _ = conn;
        using (var db = Db(sp))
        {
            var testerId = SeedRunner(db);
            SeedInFlightRun(db, testerId, runStatus);
        }

        await Service(sp).SweepAsync(CancellationToken.None);

        Assert.Empty(prov.RunCommandScripts);
    }

    [Fact]
    public async Task non_azure_and_offline_and_non_running_are_skipped()
    {
        var (sp, conn, prov) = BuildHost();
        using var _ = conn;
        using (var db = Db(sp))
        {
            SeedRunner(db, cloud: "aws", name: "aws");
            SeedRunner(db, agentStatus: "offline", name: "offline");
            SeedRunner(db, powerState: "stopped", name: "stopped");
        }

        await Service(sp).SweepAsync(CancellationToken.None);

        Assert.Empty(prov.RunCommandScripts);
    }

    [Fact]
    public async Task per_sweep_cap_limits_concurrent_restarts()
    {
        var (sp, conn, prov) = BuildHost();
        using var _ = conn;
        using (var db = Db(sp))
        {
            SeedRunner(db, name: "a");
            SeedRunner(db, name: "b");
            SeedRunner(db, name: "c");
        }

        await Service(sp).SweepAsync(CancellationToken.None);

        Assert.Equal(AgentAutoUpgradeService.MaxUpgradesPerSweep, prov.RunCommandScripts.Count);
    }

    [Fact]
    public async Task failed_reinstall_records_error_and_backs_off()
    {
        var (sp, conn, prov) = BuildHost();
        using var _ = conn;
        Guid testerId;
        using (var db = Db(sp))
        {
            testerId = SeedRunner(db);
        }
        prov.RunCommandBehavior = () => new ProvisionResult(false, 1, "", "curl: (22) 404");

        var svc = Service(sp);
        await svc.SweepAsync(CancellationToken.None);

        using (var check = Db(sp))
        {
            var tester = check.ProjectTesters.Single(t => t.TesterId == testerId);
            Assert.Equal("running", tester.PowerState);
            Assert.Contains("failed", tester.StatusMessage);
            Assert.Equal(StaleVersion, tester.InstallerVersion); // no fake success
        }

        // Second sweep on the same service instance: the failure backoff must
        // keep the wedged runner off the retry treadmill.
        await svc.SweepAsync(CancellationToken.None);
        Assert.Single(prov.RunCommandScripts);
    }

    [Fact]
    public async Task missing_cloud_cli_is_not_recorded_as_an_upgrade()
    {
        var (sp, conn, prov) = BuildHost();
        using var _ = conn;
        Guid testerId;
        using (var db = Db(sp))
        {
            testerId = SeedRunner(db);
        }
        // ExitCode == null is the provisioner's "CLI not present" soft failure:
        // nothing ran on the VM, so claiming success would fake an upgrade.
        prov.RunCommandBehavior = () => new ProvisionResult(false, null, "", "az not found");

        await Service(sp).SweepAsync(CancellationToken.None);

        using var check = Db(sp);
        var tester = check.ProjectTesters.Single(t => t.TesterId == testerId);
        Assert.Equal("running", tester.PowerState);
        Assert.Contains("CLI unavailable", tester.StatusMessage);
        Assert.Equal(StaleVersion, tester.InstallerVersion);
        Assert.Null(tester.LastInstalledAt);
    }
}
