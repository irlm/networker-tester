using Microsoft.Data.Sqlite;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Networker.ControlPlane.Endpoints;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// #765: the SDK Endpoints page showed no reachability signal — a dead
/// endpoint rendered identically to a live one. The list/detail DTOs now carry
/// the latest sdkprobe run's outcome (last_run_at/status/success/failure),
/// loaded per config by <see cref="SdkEndpointsEndpoints.LoadLastRunsAsync"/>.
/// These tests pin that lookup: newest run wins, never-probed configs are
/// absent, and other configs' runs never bleed in.
/// </summary>
public class SdkEndpointsLastRunTests
{
    private const string ProjectId = "proj-sdklr-0001";

    private static (ServiceProvider Sp, SqliteConnection Conn) BuildHost()
    {
        var conn = new SqliteConnection("DataSource=:memory:");
        conn.Open();

        var services = new ServiceCollection();
        services.AddLogging();
        services.AddDbContext<NetworkerDbContext>(o => o.UseSqlite(conn));
        var sp = services.BuildServiceProvider();
        RunDispatcherTesterFkTests.CreateMinimalSchema(conn);
        return (sp, conn);
    }

    private static NetworkerDbContext Db(IServiceProvider sp) =>
        sp.CreateScope().ServiceProvider.GetRequiredService<NetworkerDbContext>();

    private static Guid SeedConfig(NetworkerDbContext db, string name)
    {
        var id = Guid.NewGuid();
        var now = DateTime.UtcNow;
        db.TestConfigs.Add(new TestConfig
        {
            Id = id,
            ProjectId = ProjectId,
            Name = name,
            EndpointKind = "network",
            EndpointRef = """{"kind":"network","host":"https://api.example.com/"}""",
            Workload = """{"modes":["sdkprobe"],"runs":10,"concurrency":1,"timeout_ms":30000}""",
            CreatedAt = now,
            UpdatedAt = now,
        });
        db.SaveChanges();
        return id;
    }

    private static void SeedRun(
        NetworkerDbContext db, Guid configId, DateTime createdAt,
        string status, int ok, int fail)
    {
        db.TestRuns.Add(new TestRun
        {
            Id = Guid.NewGuid(),
            TestConfigId = configId,
            ProjectId = ProjectId,
            Status = status,
            SuccessCount = ok,
            FailureCount = fail,
            CreatedAt = createdAt,
        });
        db.SaveChanges();
    }

    [Fact]
    public async Task Newest_run_wins_per_config()
    {
        var (sp, conn) = BuildHost();
        using var _ = conn;
        var db = Db(sp);
        var cfg = SeedConfig(db, "checkout");
        var t0 = new DateTime(2026, 8, 1, 12, 0, 0, DateTimeKind.Utc);
        SeedRun(db, cfg, t0, "completed", 10, 0);
        SeedRun(db, cfg, t0.AddHours(2), "failed", 0, 10);
        SeedRun(db, cfg, t0.AddHours(1), "completed", 9, 1);

        var last = await SdkEndpointsEndpoints.LoadLastRunsAsync(Db(sp), [cfg], CancellationToken.None);

        var info = Assert.Contains(cfg, (IReadOnlyDictionary<Guid, SdkEndpointsEndpoints.LastRunInfo>)last);
        Assert.Equal(t0.AddHours(2), info.CreatedAt);
        Assert.Equal("failed", info.Status);
        Assert.Equal(0, info.SuccessCount);
        Assert.Equal(10, info.FailureCount);
    }

    [Fact]
    public async Task Never_probed_config_is_absent_and_neighbours_do_not_bleed()
    {
        var (sp, conn) = BuildHost();
        using var _ = conn;
        var db = Db(sp);
        var probed = SeedConfig(db, "probed");
        var never = SeedConfig(db, "never");
        SeedRun(db, probed, DateTime.UtcNow, "completed", 10, 0);

        var last = await SdkEndpointsEndpoints.LoadLastRunsAsync(
            Db(sp), [probed, never], CancellationToken.None);

        Assert.True(last.ContainsKey(probed));
        Assert.False(last.ContainsKey(never));
    }
}
