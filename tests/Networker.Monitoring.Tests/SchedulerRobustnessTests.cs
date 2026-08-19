using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging.Abstractions;
using Networker.Monitoring.Data;
using Networker.Monitoring.Data.Entities;
using Networker.Monitoring.Probe;
using Networker.Monitoring.Scheduling;
using MonitorEntity = Networker.Monitoring.Data.Entities.Monitor;

namespace Networker.Monitoring.Tests;

[Collection(MonitoringCollection.Name)]
public sealed class SchedulerRobustnessTests(MonitoringFixture fixture)
{
    [Fact]
    public async Task One_failing_lease_does_not_abort_the_rest_of_the_batch()
    {
        var now = DateTimeOffset.UtcNow;
        var poisonId = Guid.NewGuid();
        var healthyId = Guid.NewGuid();
        Guid locationId;
        await using (var seed = fixture.NewDbContext())
        {
            locationId = await seed.MonitorLocations.Select(row => row.LocationId).SingleAsync();
            seed.Monitors.AddRange(
                NewMonitor(poisonId, "scheduler poison", locationId, now),
                NewMonitor(healthyId, "scheduler healthy", locationId, now));
            await seed.SaveChangesAsync();
        }

        var services = new ServiceCollection();
        services.AddLogging();
        services.AddSingleton(TimeProvider.System);
        services.AddDbContext<MonitoringDbContext>(options => options.UseNpgsql(fixture.ConnectionString));
        // The poison executor returns a check whose location FK cannot exist,
        // so MonitorCheckRunner's own SaveChangesAsync throws — the failure
        // mode the per-lease isolation in RunTickAsync must contain.
        services.AddSingleton<IMonitorProbeExecutor>(new PoisonExecutor(poisonId));
        services.AddScoped<MonitorCheckRunner>();
        await using var provider = services.BuildServiceProvider();

        var scheduler = new MonitorScheduler(
            provider.GetRequiredService<IServiceScopeFactory>(),
            TimeProvider.System,
            new ConfigurationBuilder().Build(),
            NullLogger<MonitorScheduler>.Instance);

        await scheduler.RunTickAsync(CancellationToken.None);

        await using var verify = fixture.NewDbContext();
        Assert.Equal(1, await verify.MonitorChecks.CountAsync(check => check.MonitorId == healthyId));
        Assert.Equal(0, await verify.MonitorChecks.CountAsync(check => check.MonitorId == poisonId));

        // Double-fire semantics untouched: both monitors were leased and had
        // next_check_at advanced past the tick regardless of run outcome.
        var monitors = await verify.Monitors
            .Where(row => row.MonitorId == poisonId || row.MonitorId == healthyId)
            .ToListAsync();
        Assert.All(monitors, row => Assert.True(row.NextCheckAt > now));
    }

    private static MonitorEntity NewMonitor(Guid monitorId, string name, Guid locationId, DateTimeOffset now) => new()
    {
        MonitorId = monitorId,
        ProjectId = $"scheduler-{Guid.NewGuid():N}",
        Name = name,
        TargetUrl = "https://example.com/health",
        Method = "GET",
        AssertionConfig = "{}",
        IntervalSeconds = 60,
        TimeoutMs = 1000,
        Enabled = true,
        Criticality = "reporting",
        RetentionPolicy = "{}",
        NextCheckAt = now.AddMinutes(-1),
        CreatedAt = now,
        UpdatedAt = now,
        LocationAssignments =
        [
            new MonitorLocationAssignment
            {
                MonitorId = monitorId,
                LocationId = locationId,
                CreatedAt = now,
            },
        ],
    };

    private sealed class PoisonExecutor(Guid poisonMonitorId) : IMonitorProbeExecutor
    {
        public Task<MonitorCheck> ExecuteAsync(
            MonitorEntity monitor,
            Guid locationId,
            Guid checkId,
            DateTimeOffset scheduledAt,
            CancellationToken cancellationToken)
        {
            var now = DateTimeOffset.UtcNow;
            return Task.FromResult(new MonitorCheck
            {
                CheckId = checkId,
                MonitorId = monitor.MonitorId,
                // A nonexistent location violates the FK during the runner's
                // persist for the poison monitor only.
                LocationId = monitor.MonitorId == poisonMonitorId ? Guid.NewGuid() : locationId,
                ScheduledAt = scheduledAt,
                StartedAt = now,
                FinishedAt = now,
                Outcome = "healthy",
                TotalMs = 1,
                AssertionResults = "{}",
                CreatedAt = now,
            });
        }
    }
}
