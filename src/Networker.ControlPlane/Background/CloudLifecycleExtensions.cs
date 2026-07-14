using Networker.ControlPlane.Background;

namespace Microsoft.Extensions.DependencyInjection;

/// <summary>
/// DI wiring for the M4 slice-2 cloud lifecycle background loops — the C# port
/// of the Rust dashboard's <c>auto_shutdown_loop</c>
/// (<c>services::tester_scheduler</c>) and <c>cloud_orphan_reaper</c>.
///
/// <para>Registers both as <see cref="Microsoft.Extensions.Hosting.BackgroundService"/>
/// hosted services:</para>
/// <list type="bullet">
///   <item><see cref="AutoShutdownService"/> — ~60s tick; deallocates drained,
///     idle testers past their shutdown window to save cost.</item>
///   <item><see cref="OrphanReaperService"/> — ~10min tick; deletes dangling
///     NICs / public IPs / disks from failed VM creation. Best-effort / no-op
///     when no cloud CLI or credentials are present (CI-safe).</item>
/// </list>
///
/// <para>Both services depend only on <c>IServiceScopeFactory</c> (for a fresh
/// <c>NetworkerDbContext</c> scope per tick) and, for auto-shutdown, the
/// <c>IComputeProvisioner</c> registered by <c>AddComputeProvisioner()</c> — call
/// that alongside this if it isn't already wired.</para>
/// </summary>
public static class CloudLifecycleExtensions
{
    /// <summary>
    /// Register the auto-shutdown and cloud-orphan-reaper hosted services.
    /// </summary>
    public static IServiceCollection AddNetworkerCloudLifecycleServices(this IServiceCollection services)
    {
        services.AddHostedService<AutoShutdownService>();
        services.AddHostedService<OrphanReaperService>();
        return services;
    }
}
