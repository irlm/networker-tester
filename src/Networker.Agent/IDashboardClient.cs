using Networker.Contracts;

namespace Networker.Agent;

/// <summary>
/// Transport to the control plane. In Phase 2 this becomes a SignalR hub client
/// that streams results live and answers heartbeats. For now it is a stub so the
/// agent's probe → report seam is exercisable end-to-end without a dashboard.
/// </summary>
public interface IDashboardClient
{
    /// <summary>Report a completed probe run to the control plane.</summary>
    Task ReportResultAsync(ProbeRunResult result, CancellationToken cancellationToken = default);

    /// <summary>Signal liveness to the control plane.</summary>
    Task HeartbeatAsync(CancellationToken cancellationToken = default);
}

/// <summary>
/// No-op console implementation of <see cref="IDashboardClient"/>.
///
/// TODO(Phase 2): replace with a SignalR hub connection
/// (Microsoft.AspNetCore.SignalR.Client) that:
///   - connects to the dashboard hub URL with the agent API key,
///   - invokes "ReportResult" with the ProbeRunResult over the wire, and
///   - invokes "Heartbeat" on a timer / responds to server pings.
/// The interface above is deliberately the exact seam SignalR will slot into.
/// </summary>
public sealed class NoOpDashboardClient(ILogger<NoOpDashboardClient> logger) : IDashboardClient
{
    public Task ReportResultAsync(ProbeRunResult result, CancellationToken cancellationToken = default)
    {
        logger.LogInformation(
            "[stub dashboard] would report run {RunId} ({AttemptCount} attempts) to control plane",
            result.RunId,
            result.Attempts.Count);
        return Task.CompletedTask;
    }

    public Task HeartbeatAsync(CancellationToken cancellationToken = default)
    {
        logger.LogDebug("[stub dashboard] heartbeat");
        return Task.CompletedTask;
    }
}
