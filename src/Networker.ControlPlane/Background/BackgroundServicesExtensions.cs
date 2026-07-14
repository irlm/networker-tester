namespace Networker.ControlPlane.Background;

/// <summary>
/// DI wiring for the M3 slice-2 background workers. Registers both hosted
/// services in one call so <c>Program.cs</c> only needs a single line.
/// </summary>
public static class BackgroundServicesExtensions
{
    /// <summary>
    /// Register the <see cref="SchedulerService"/> (schedule =&gt; run fan-out) and
    /// the <see cref="QueuedRunRedispatchService"/> (stuck-queued retry) as hosted
    /// background services. Both open their own DI scope per tick to resolve the
    /// scoped <c>IRunDispatcher</c> / <c>NetworkerDbContext</c>.
    /// </summary>
    public static IServiceCollection AddNetworkerSchedulerServices(this IServiceCollection services)
    {
        services.AddHostedService<SchedulerService>();
        services.AddHostedService<QueuedRunRedispatchService>();
        return services;
    }
}
