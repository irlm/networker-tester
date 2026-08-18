using Microsoft.EntityFrameworkCore;
using Networker.Monitoring.Data;
using Networker.Monitoring.Data.Entities;
using MonitorEntity = Networker.Monitoring.Data.Entities.Monitor;

namespace Networker.Monitoring.Scheduling;

public sealed record LeasedMonitor(MonitorEntity Monitor, Guid LocationId, DateTimeOffset ScheduledAt);

public static class MonitorLeaseService
{
    public static async Task<IReadOnlyList<LeasedMonitor>> LeaseDueAsync(
        MonitoringDbContext db,
        string workerId,
        DateTimeOffset now,
        int batchSize,
        TimeSpan leaseDuration,
        CancellationToken cancellationToken)
    {
        await using var transaction = await db.Database.BeginTransactionAsync(cancellationToken);
        var due = await db.Monitors
            .FromSqlInterpolated($$"""
                SELECT * FROM monitor
                WHERE enabled = true
                  AND deleted_at IS NULL
                  AND next_check_at <= {{now}}
                  AND (lease_expires_at IS NULL OR lease_expires_at <= {{now}})
                ORDER BY next_check_at
                LIMIT {{batchSize}}
                FOR UPDATE SKIP LOCKED
                """)
            .ToListAsync(cancellationToken);

        if (due.Count == 0)
        {
            await transaction.CommitAsync(cancellationToken);
            return [];
        }

        var monitorIds = due.Select(monitor => monitor.MonitorId).ToArray();
        var locations = await db.MonitorLocationAssignments
            .Where(assignment => monitorIds.Contains(assignment.MonitorId) && assignment.Location.Enabled)
            .ToDictionaryAsync(
                assignment => assignment.MonitorId,
                assignment => assignment.LocationId,
                cancellationToken);

        var leases = new List<LeasedMonitor>(due.Count);
        foreach (var monitor in due)
        {
            if (!locations.TryGetValue(monitor.MonitorId, out var locationId))
            {
                monitor.NextCheckAt = now.AddSeconds(monitor.IntervalSeconds);
                monitor.UpdatedAt = now;
                continue;
            }

            var scheduledAt = monitor.NextCheckAt;
            monitor.LeaseOwner = workerId;
            monitor.LeaseExpiresAt = now.Add(leaseDuration);
            monitor.NextCheckAt = now.AddSeconds(monitor.IntervalSeconds);
            monitor.UpdatedAt = now;
            leases.Add(new LeasedMonitor(monitor, locationId, scheduledAt));
        }

        await db.SaveChangesAsync(cancellationToken);
        await transaction.CommitAsync(cancellationToken);
        return leases;
    }
}
