using Microsoft.EntityFrameworkCore;
using Networker.Monitoring.Data.Migrations;

namespace Networker.Monitoring.Tests;

[Collection(MonitoringCollection.Name)]
public sealed class MonitoringSchemaTests(MonitoringFixture fixture)
{
    [Fact]
    public void Fresh_database_applies_independent_migration_chain()
    {
        Assert.Equal([1], fixture.InitialMigration.Applied);
        Assert.Empty(fixture.InitialMigration.AlreadyApplied);
    }

    [Fact]
    public async Task Migration_is_idempotent_and_uses_its_own_ledger()
    {
        await using var db = fixture.NewDbContext();
        var connectionString = db.Database.GetConnectionString()!;

        var rerun = await MonitoringSchemaMigrator.MigrateAsync(connectionString);

        Assert.True(rerun.WasUpToDate);
        Assert.Equal([1], rerun.AlreadyApplied);
        Assert.Equal("_monitoring_migrations", MonitoringSchemaMigrator.BookkeepingTable);
    }

    [Fact]
    public async Task Ef_model_queries_every_foundation_entity()
    {
        await using var db = fixture.NewDbContext();

        _ = await db.Monitors.CountAsync();
        _ = await db.MonitorLocations.CountAsync();
        _ = await db.MonitorLocationAssignments.CountAsync();
        _ = await db.MonitorChecks.CountAsync();

        Assert.Single(await db.MonitorLocations.ToListAsync());
    }
}
