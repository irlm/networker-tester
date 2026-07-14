namespace Networker.ControlPlane.Background;

/// <summary>
/// DI wiring for the M3 slice-2 background reconciliation services: the
/// <see cref="WatchdogService"/> (fails runs whose agent died / that sat queued
/// too long) and the <see cref="ReaperService"/> (marks dead agents offline).
///
/// <para>Both are registered via <c>AddHostedService</c> so the generic host
/// owns their lifetime. They depend on the singleton
/// <c>AgentConnectionRegistry</c> and <c>EventBus</c> (registered by the realtime
/// slice) and, per tick, on a scoped <c>NetworkerDbContext</c> resolved from a
/// fresh <see cref="IServiceScopeFactory"/> scope — so no extra registration is
/// required here beyond the two hosted services.</para>
/// </summary>
public static class ReconciliationExtensions
{
    /// <summary>
    /// Register the stale-job watchdog and the agent-status reaper as hosted
    /// <see cref="BackgroundService"/>s. Call from <c>Program.cs</c> after the
    /// realtime (registry + event bus) and data (DbContext) services are wired.
    /// Skipped when <see cref="BackgroundServicesGate"/> disables this replica
    /// (DASHBOARD_BACKGROUND_SERVICES=0 → API-only; the loops double-fire across
    /// replicas until the M6 pg-advisory-lock leader election lands).
    /// </summary>
    public static IServiceCollection AddNetworkerReconciliationServices(this IServiceCollection services)
    {
        if (!BackgroundServicesGate.IsEnabled("watchdog/reaper"))
        {
            return services;
        }

        services.AddHostedService<WatchdogService>();
        services.AddHostedService<ReaperService>();
        return services;
    }
}
