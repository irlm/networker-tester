using Microsoft.Data.Sqlite;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Networker.ControlPlane.Background;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// #765: pre-v0.28.227 tester deletes left agent rows behind
/// (agent_tester_id_fkey is ON DELETE SET NULL), and the retire sweep only
/// hid them (status='retired') — ~35 such rows accumulated in prod. The
/// reaper's purge deletes retired, tester-less, deployment-unreferenced rows.
/// These tests pin exactly which rows the purge may touch.
/// </summary>
public class AgentOrphanPurgeTests
{
    private const string ProjectId = "proj-purge-0001";

    private static (ServiceProvider Sp, SqliteConnection Conn) BuildHost()
    {
        var conn = new SqliteConnection("DataSource=:memory:");
        conn.Open();

        var services = new ServiceCollection();
        services.AddLogging();
        services.AddDbContext<NetworkerDbContext>(o => o.UseSqlite(conn));
        var sp = services.BuildServiceProvider();
        RunDispatcherTesterFkTests.CreateMinimalSchema(conn);

        using var scope = sp.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
        db.Projects.Add(new Project
        {
            ProjectId = ProjectId,
            Name = "purge",
            Slug = "purge",
            Settings = "{}",
            CreatedAt = DateTime.UtcNow,
            UpdatedAt = DateTime.UtcNow,
        });
        db.SaveChanges();
        return (sp, conn);
    }

    private static NetworkerDbContext Db(IServiceProvider sp) =>
        sp.CreateScope().ServiceProvider.GetRequiredService<NetworkerDbContext>();

    private static Guid SeedAgent(
        NetworkerDbContext db, string status, Guid? testerId = null, DateTime? lastHeartbeat = null)
    {
        var id = Guid.NewGuid();
        db.Agents.Add(new Agent
        {
            AgentId = id,
            Name = $"agent-{id:N}",
            Status = status,
            ProjectId = ProjectId,
            TesterId = testerId,
            LastHeartbeat = lastHeartbeat,
            RegisteredAt = DateTime.UtcNow.AddDays(-90),
        });
        db.SaveChanges();
        return id;
    }

    private static Guid SeedTester(NetworkerDbContext db)
    {
        var id = Guid.NewGuid();
        var now = DateTime.UtcNow;
        db.ProjectTesters.Add(new ProjectTester
        {
            TesterId = id,
            ProjectId = ProjectId,
            Name = $"tester-{id:N}",
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
        db.SaveChanges();
        return id;
    }

    [Fact]
    public async Task Purges_retired_testerless_agent()
    {
        var (sp, conn) = BuildHost();
        using var _ = conn;
        var db = Db(sp);
        var orphan = SeedAgent(db, "retired");

        var purged = await ReaperService.PurgeTesterOrphanedAgentsAsync(db, [], CancellationToken.None);

        Assert.Equal(1, purged);
        Assert.False(await Db(sp).Agents.AnyAsync(a => a.AgentId == orphan));
    }

    [Fact]
    public async Task Keeps_offline_rows_until_the_retire_sweep_promotes_them()
    {
        // The purge intentionally rides BEHIND the retire sweep: an offline
        // tester-less row (e.g. a freshly disconnected dev agent) is not
        // touched until it has been silent long enough to be retired.
        var (sp, conn) = BuildHost();
        using var _ = conn;
        var db = Db(sp);
        SeedAgent(db, "offline");

        var purged = await ReaperService.PurgeTesterOrphanedAgentsAsync(db, [], CancellationToken.None);

        Assert.Equal(0, purged);
        Assert.Equal(1, await Db(sp).Agents.CountAsync());
    }

    [Fact]
    public async Task Keeps_tester_linked_retired_rows()
    {
        // A retired row that still points at a live tester is history worth
        // keeping (the documented RETIRE-never-delete rule) — and its tester's
        // delete path reaps it when the tester goes (v0.28.227).
        var (sp, conn) = BuildHost();
        using var _ = conn;
        var db = Db(sp);
        var testerId = SeedTester(db);
        SeedAgent(db, "retired", testerId: testerId);

        var purged = await ReaperService.PurgeTesterOrphanedAgentsAsync(db, [], CancellationToken.None);

        Assert.Equal(0, purged);
        Assert.Equal(1, await Db(sp).Agents.CountAsync());
    }

    [Fact]
    public async Task Keeps_deployment_referenced_rows()
    {
        // deployment_agent_id_fkey is NO ACTION — deleting a referenced agent
        // would throw, and the deployment's agent link is audit data.
        var (sp, conn) = BuildHost();
        using var _ = conn;
        var db = Db(sp);
        var referenced = SeedAgent(db, "retired");
        db.Deployments.Add(new Deployment
        {
            DeploymentId = Guid.NewGuid(),
            Name = "old deploy",
            Status = "torn_down",
            Config = "{}",
            ProjectId = ProjectId,
            AgentId = referenced,
            CreatedAt = DateTime.UtcNow,
        });
        db.SaveChanges();
        var purgeable = SeedAgent(db, "retired");

        var purged = await ReaperService.PurgeTesterOrphanedAgentsAsync(db, [], CancellationToken.None);

        Assert.Equal(1, purged);
        var remaining = await Db(sp).Agents.Select(a => a.AgentId).ToListAsync();
        Assert.Equal([referenced], remaining);
        Assert.DoesNotContain(purgeable, remaining);
    }

    [Fact]
    public async Task Keeps_rows_whose_socket_is_live()
    {
        // Belt-and-braces: registry membership always wins, even if the DB
        // status column somehow says retired while a connection is live.
        var (sp, conn) = BuildHost();
        using var _ = conn;
        var db = Db(sp);
        var live = SeedAgent(db, "retired");

        var purged = await ReaperService.PurgeTesterOrphanedAgentsAsync(db, [live], CancellationToken.None);

        Assert.Equal(0, purged);
        Assert.Equal(1, await Db(sp).Agents.CountAsync());
    }
}
