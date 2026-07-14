namespace Networker.ControlPlane.Background;

/// <summary>
/// Single-replica safety gate for ALL hosted background loops (scheduler,
/// redispatch, watchdog, reaper, auto-shutdown, orphan reaper, provisioning
/// orchestrator). These services double-fire if two control-plane replicas run
/// them simultaneously (duplicate schedule fan-out, competing watchdog state
/// flips, double VM deallocation), so a second replica must be deployable
/// API-only: set <c>DASHBOARD_BACKGROUND_SERVICES=0</c> (or <c>false</c>) on it
/// and only the primary runs the loops.
///
/// <para>Default is ENABLED (unset/any other value). Each registration
/// extension consults <see cref="IsEnabled(string)"/>, which logs the chosen
/// mode so an operator can confirm from startup output which replica owns the
/// background work.</para>
///
/// <para>FOLLOW-UP (M6): this env switch is a manual ops gate, not real HA —
/// proper leader election via Postgres advisory locks (each loop takes
/// <c>pg_try_advisory_lock</c> per tick, so any replica can take over when the
/// leader dies) is the planned M6 replacement.</para>
/// </summary>
public static class BackgroundServicesGate
{
    public const string EnvVar = "DASHBOARD_BACKGROUND_SERVICES";

    /// <summary>
    /// Whether background services should be registered, logging the decision
    /// for the given service group (e.g. "scheduler/redispatch").
    /// </summary>
    public static bool IsEnabled(string serviceGroup)
    {
        var raw = Environment.GetEnvironmentVariable(EnvVar);
        var enabled = ParseEnabled(raw);
        Console.Error.WriteLine(enabled
            ? $"networker-controlplane: {serviceGroup} background services ENABLED " +
              $"({EnvVar}={(string.IsNullOrEmpty(raw) ? "<unset, default on>" : raw)})."
            : $"networker-controlplane: {serviceGroup} background services DISABLED via " +
              $"{EnvVar}={raw} — this replica is API-only; ensure exactly one replica runs them.");
        return enabled;
    }

    /// <summary>
    /// Pure parse (testable): only "0" or "false" (case-insensitive, trimmed)
    /// disable; unset/empty/anything else keeps the default-enabled behavior.
    /// </summary>
    public static bool ParseEnabled(string? raw)
    {
        var value = raw?.Trim();
        return !(value == "0" || string.Equals(value, "false", StringComparison.OrdinalIgnoreCase));
    }
}

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
    /// Skipped entirely when <see cref="BackgroundServicesGate"/> disables this
    /// replica (API-only mode).
    /// </summary>
    public static IServiceCollection AddNetworkerSchedulerServices(this IServiceCollection services)
    {
        if (!BackgroundServicesGate.IsEnabled("scheduler/redispatch"))
        {
            return services;
        }

        services.AddHostedService<SchedulerService>();
        services.AddHostedService<QueuedRunRedispatchService>();
        return services;
    }
}
