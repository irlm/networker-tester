using Microsoft.EntityFrameworkCore;
using Networker.Monitoring.Data.Entities;
using Networker.Monitoring.Scheduling;
using MonitorEntity = Networker.Monitoring.Data.Entities.Monitor;

namespace Networker.Monitoring.Tests;

[Collection(MonitoringCollection.Name)]
public sealed class MonitorLeaseTests(MonitoringFixture fixture)
{
    [Fact]
    public async Task Due_monitor_is_leased_once_and_next_schedule_advances()
    {
        var now = new DateTimeOffset(2026, 8, 18, 20, 0, 0, TimeSpan.Zero);
        var monitorId = Guid.NewGuid();
        await using (var seed = fixture.NewDbContext())
        {
            var locationId = await seed.MonitorLocations.Select(row => row.LocationId).SingleAsync();
            seed.Monitors.Add(new MonitorEntity
            {
                MonitorId = monitorId,
                ProjectId = $"lease-{Guid.NewGuid():N}",
                Name = "lease test",
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
            });
            await seed.SaveChangesAsync();
        }

        await using var firstDb = fixture.NewDbContext();
        var first = await MonitorLeaseService.LeaseDueAsync(
            firstDb, "worker-a", now, 10, TimeSpan.FromSeconds(45), CancellationToken.None);
        await using var secondDb = fixture.NewDbContext();
        var second = await MonitorLeaseService.LeaseDueAsync(
            secondDb, "worker-b", now, 10, TimeSpan.FromSeconds(45), CancellationToken.None);

        var lease = Assert.Single(first);
        Assert.Equal(monitorId, lease.Monitor.MonitorId);
        Assert.Empty(second);

        await using var verify = fixture.NewDbContext();
        var stored = await verify.Monitors.SingleAsync(row => row.MonitorId == monitorId);
        Assert.Equal("worker-a", stored.LeaseOwner);
        Assert.Equal(now.AddSeconds(60), stored.NextCheckAt);
    }
}
