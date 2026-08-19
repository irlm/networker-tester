using Networker.Monitoring.Data.Entities;
using MonitorEntity = Networker.Monitoring.Data.Entities.Monitor;

namespace Networker.Monitoring.Probe;

public interface IMonitorProbeExecutor
{
    Task<MonitorCheck> ExecuteAsync(
        MonitorEntity monitor,
        Guid locationId,
        Guid checkId,
        DateTimeOffset scheduledAt,
        CancellationToken cancellationToken);
}
