using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.Mvc.Testing;
using Microsoft.EntityFrameworkCore;
using Networker.Monitoring.Data;
using Networker.Monitoring.Data.Migrations;
using Testcontainers.PostgreSql;

namespace Networker.Monitoring.Tests;

public sealed class MonitoringFixture : WebApplicationFactory<Program>, IAsyncLifetime
{
    private readonly PostgreSqlContainer _database = new PostgreSqlBuilder("postgres:16-alpine")
        .WithDatabase("networker_monitoring")
        .WithUsername("networker")
        .WithPassword("networker")
        .Build();

    public const string ApiKey = "monitoring-integration-test-key";
    public MonitoringMigrationResult InitialMigration { get; private set; } = null!;

    protected override void ConfigureWebHost(IWebHostBuilder builder)
    {
        builder.UseEnvironment("Development");
        builder.UseSetting("ConnectionStrings:Monitoring", _database.GetConnectionString());
        builder.UseSetting("MONITORING_API_KEY", ApiKey);
        builder.UseSetting("MONITORING_RUN_MIGRATIONS", "0");
        builder.UseSetting("MONITORING_BACKGROUND_SERVICES", "0");
    }

    public async Task InitializeAsync()
    {
        await _database.StartAsync();
        InitialMigration = await MonitoringSchemaMigrator.MigrateAsync(_database.GetConnectionString());
    }

    public new async Task DisposeAsync()
    {
        await base.DisposeAsync();
        await _database.DisposeAsync();
    }

    public string ConnectionString => _database.GetConnectionString();

    public MonitoringDbContext NewDbContext() =>
        new(new DbContextOptionsBuilder<MonitoringDbContext>()
            .UseNpgsql(_database.GetConnectionString())
            .Options);
}

[CollectionDefinition(Name)]
public sealed class MonitoringCollection : ICollectionFixture<MonitoringFixture>
{
    public const string Name = "monitoring integration";
}
