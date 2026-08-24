using Microsoft.Data.Sqlite;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Provisioning;
using Networker.ControlPlane.Realtime;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// The teardown sweep takes a bounded batch of finished auto-provisioned runs
/// per tick. It used to take that batch with no ORDER BY — EF Core says so out
/// loud ("The query uses a row limiting operator ('Skip'/'Take') without an
/// 'OrderBy' operator"), and the warning showed up in production the moment the
/// service_log sink was switched on (2026-08-24).
///
/// <para>Unordered paging is not cosmetic here. The batch is the reaper's whole
/// budget for the tick, several of its branches skip a candidate without tearing
/// anything down, and the row order the planner happens to pick is stable while
/// the heap is unchanged — so a deployment can sit outside the batch tick after
/// tick while its VM and public IP keep billing. That is precisely the leak
/// <c>TeardownFinishedRunsAsync</c> was written to stop (see its docstring: ten
/// B2s VMs leaked per launch, 2026-08-01).</para>
///
/// <para>The candidates are seeded NEWEST FIRST, so insertion order — what an
/// unordered scan returns — is the exact opposite of the correct order. A batch
/// taken without the fix tears down the newest and starves the oldest; the
/// assertions below fail in that case rather than passing by luck.</para>
/// </summary>
public class TeardownBatchOrderingTests
{
    private const string ProjectId = "proj-teardown-01";

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

        var sp = services.BuildServiceProvider();
        RunDispatcherTesterFkTests.CreateMinimalSchema(conn);
        return (sp, conn);
    }

    private static ProvisioningOrchestrator Orchestrator(IServiceProvider sp) => new(
        sp.GetRequiredService<IServiceScopeFactory>(),
        new DeployRunner(
            sp.GetRequiredService<IServiceScopeFactory>(),
            sp.GetRequiredService<EventBus>(),
            sp.GetRequiredService<ILogger<DeployRunner>>()),
        sp.GetRequiredService<ILogger<ProvisioningOrchestrator>>());

    private static NetworkerDbContext Db(IServiceProvider sp) =>
        sp.CreateScope().ServiceProvider.GetRequiredService<NetworkerDbContext>();

    /// <summary>One finished auto-provisioned run whose deployment has no
    /// registered endpoint — the branch that marks torn_down without calling a
    /// cloud provider, so the sweep stays hermetic.</summary>
    private static Guid SeedFinishedCandidate(NetworkerDbContext db, DateTime finishedAt)
    {
        var now = DateTime.UtcNow;
        if (!db.Projects.Any(p => p.ProjectId == ProjectId))
        {
            db.Projects.Add(new Project
            {
                ProjectId = ProjectId,
                Name = "teardown",
                Slug = "teardown",
                Settings = "{}",
                CreatedAt = now,
                UpdatedAt = now,
            });
        }

        var configId = Guid.NewGuid();
        db.TestConfigs.Add(new TestConfig
        {
            Id = configId,
            ProjectId = ProjectId,
            Name = $"cell-{configId:N}",
            EndpointKind = "pending",
            EndpointRef = """{"kind": "pending", "proxy_stack": "nginx"}""",
            Workload = "{}",
            MaxDurationSecs = 600,
            CreatedAt = now,
            UpdatedAt = now,
        });

        var deploymentId = Guid.NewGuid();
        db.Deployments.Add(new Deployment
        {
            DeploymentId = deploymentId,
            Name = $"auto-cell-{deploymentId:N}",
            Status = "completed",
            Config = "{}",          // no provider -> no cloud call
            EndpointIps = null,     // no hosts     -> "marked torn_down" branch
            EndpointHosts = null,
            FinishedAt = finishedAt,
            CreatedAt = finishedAt,
            ProjectId = ProjectId,
        });

        db.TestRuns.Add(new TestRun
        {
            Id = Guid.NewGuid(),
            TestConfigId = configId,
            ProjectId = ProjectId,
            Status = "completed",
            ProvisioningDeploymentId = deploymentId,
            CreatedAt = finishedAt,
            FinishedAt = finishedAt,
        });

        db.SaveChanges();   // per-candidate, so rowid order == insertion order
        return deploymentId;
    }

    [Fact]
    public async Task Teardown_batch_takes_the_oldest_finished_runs_not_an_arbitrary_slice()
    {
        var batch = ProvisioningOrchestrator.KickBatchLimit;
        var overflow = 5;
        var total = batch + overflow;

        var (sp, conn) = BuildHost();
        using var _ = conn;

        // Seeded newest-first: index 0 finished most recently, index total-1
        // longest ago. All are well past TeardownGrace.
        var deploymentsNewestFirst = new List<Guid>();
        using (var db = Db(sp))
        {
            for (var i = 0; i < total; i++)
            {
                deploymentsNewestFirst.Add(
                    SeedFinishedCandidate(db, DateTime.UtcNow - TimeSpan.FromMinutes(31 + i)));
            }
        }

        int torn;
        using (var db = Db(sp))
        {
            torn = await Orchestrator(sp).TeardownFinishedRunsAsync(db, CancellationToken.None);
        }

        Assert.Equal(batch, torn);

        using var check = Db(sp);
        var status = check.Deployments
            .AsNoTracking()
            .ToDictionary(d => d.DeploymentId, d => d.Status);

        // The oldest `batch` candidates are the tail of a newest-first list.
        var expectedTorn = deploymentsNewestFirst.Skip(overflow).ToList();
        var expectedLeft = deploymentsNewestFirst.Take(overflow).ToList();

        Assert.All(expectedTorn, id => Assert.Equal(
            ProvisioningOrchestrator.DeploymentTornDown, status[id]));

        // The newest few wait for the next tick. Without the ordering these are
        // exactly the ones an unordered scan would have taken first, starving
        // the oldest — the money leak this pins down.
        Assert.All(expectedLeft, id => Assert.Equal("completed", status[id]));
    }

    [Fact]
    public async Task A_second_tick_drains_what_the_first_batch_left()
    {
        // Ordering must not merely be stable, it must make PROGRESS: the rows
        // the first tick tore down are excluded from the next candidate set, so
        // the remainder is picked up rather than re-skipped forever.
        var total = ProvisioningOrchestrator.KickBatchLimit + 5;

        var (sp, conn) = BuildHost();
        using var _ = conn;

        using (var db = Db(sp))
        {
            for (var i = 0; i < total; i++)
            {
                SeedFinishedCandidate(db, DateTime.UtcNow - TimeSpan.FromMinutes(31 + i));
            }
        }

        using (var db = Db(sp))
        {
            await Orchestrator(sp).TeardownFinishedRunsAsync(db, CancellationToken.None);
        }
        using (var db = Db(sp))
        {
            var second = await Orchestrator(sp).TeardownFinishedRunsAsync(db, CancellationToken.None);
            Assert.Equal(5, second);
        }

        using var check = Db(sp);
        Assert.Equal(total, check.Deployments.Count(d =>
            d.Status == ProvisioningOrchestrator.DeploymentTornDown));
    }
}
