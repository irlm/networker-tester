namespace Networker.ControlPlane.Background;

/// <summary>
/// DI wiring for the M5 workspace-inactivity lifecycle loop. One line in
/// Program.cs: <c>builder.Services.AddNetworkerInactivityService();</c>.
/// </summary>
public static class InactivityExtensions
{
    /// <summary>
    /// Register <see cref="InactivityService"/> as a hosted background service.
    /// Honors the same <see cref="BackgroundServicesGate"/> replica gate as the
    /// scheduler/reconciliation loops (<c>DASHBOARD_BACKGROUND_SERVICES=0</c>
    /// makes this replica API-only) — a double-firing lifecycle sweep would
    /// double-warn and race the hard-delete cascade.
    /// </summary>
    public static IServiceCollection AddNetworkerInactivityService(this IServiceCollection services)
    {
        if (!BackgroundServicesGate.IsEnabled("workspace-inactivity"))
        {
            return services;
        }

        services.AddHostedService<InactivityService>();
        return services;
    }
}
