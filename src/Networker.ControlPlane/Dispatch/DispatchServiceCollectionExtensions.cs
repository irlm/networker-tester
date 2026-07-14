namespace Networker.ControlPlane.Dispatch;

/// <summary>
/// DI wiring for the M3 write-path run dispatcher.
/// </summary>
public static class DispatchServiceCollectionExtensions
{
    /// <summary>
    /// Register <see cref="IRunDispatcher"/> (→ <see cref="RunDispatcher"/>).
    ///
    /// <para>Scoped, because it consumes a scoped <c>NetworkerDbContext</c>. Its
    /// singleton collaborators (<c>AgentConnectionRegistry</c>, <c>EventBus</c>)
    /// are resolved fine from a scoped consumer. The write endpoints resolve it
    /// per-request; the M3-slice-2 scheduler <c>BackgroundService</c> must create
    /// its own scope per tick (<c>IServiceScopeFactory.CreateScope()</c>) before
    /// resolving it — the standard pattern for using a scoped service from a
    /// singleton hosted service.</para>
    /// </summary>
    public static IServiceCollection AddRunDispatcher(this IServiceCollection services)
    {
        services.AddScoped<IRunDispatcher, RunDispatcher>();
        return services;
    }
}
