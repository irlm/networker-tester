using Networker.Monitoring.Data;
using Networker.Monitoring.Data.Entities;
using Networker.Monitoring.Probe;

namespace Networker.Monitoring.Scheduling;

public sealed class MonitorCheckRunner(
    MonitoringDbContext db,
    IMonitorProbeExecutor probeExecutor,
    TimeProvider timeProvider,
    ILogger<MonitorCheckRunner> logger)
{
    public async Task RunAsync(LeasedMonitor lease, CancellationToken cancellationToken)
    {
        var checkId = Guid.NewGuid();
        MonitorCheck check;
        try
        {
            check = await probeExecutor.ExecuteAsync(
                lease.Monitor,
                lease.LocationId,
                checkId,
                lease.ScheduledAt,
                cancellationToken);
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            throw;
        }
        catch (Exception exception)
        {
            logger.LogError(exception, "Monitoring runner failed for monitor {MonitorId}", lease.Monitor.MonitorId);
            var now = timeProvider.GetUtcNow();
            check = new MonitorCheck
            {
                CheckId = checkId,
                MonitorId = lease.Monitor.MonitorId,
                LocationId = lease.LocationId,
                ScheduledAt = lease.ScheduledAt,
                StartedAt = now,
                FinishedAt = now,
                Outcome = "unknown",
                FailureKind = "runner",
                AssertionResults = "{\"runner\":{\"passed\":false}}",
                ErrorSummary = "The monitoring runner could not establish target health.",
                CreatedAt = now,
            };
        }

        db.Attach(lease.Monitor);
        db.MonitorChecks.Add(check);
        lease.Monitor.LeaseOwner = null;
        lease.Monitor.LeaseExpiresAt = null;
        lease.Monitor.UpdatedAt = timeProvider.GetUtcNow();
        await db.SaveChangesAsync(cancellationToken);
    }
}
