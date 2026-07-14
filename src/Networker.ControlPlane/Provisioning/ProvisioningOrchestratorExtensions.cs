using Microsoft.Extensions.DependencyInjection.Extensions;
using Networker.ControlPlane.Provisioning;

namespace Microsoft.Extensions.DependencyInjection;

/// <summary>
/// DI wiring for the M4 slice-2 provisioning path: the <see cref="DeployRunner"/>
/// (shell-out to <c>install.sh --deploy</c>) and the
/// <see cref="ProvisioningOrchestrator"/> hosted service that drives
/// <c>Pending</c>-endpoint runs through provisioning and re-queues them once the
/// VM is live.
/// </summary>
public static class ProvisioningOrchestratorExtensions
{
    /// <summary>
    /// Register the <see cref="DeployRunner"/> as a singleton (it is stateless —
    /// each call opens its own DI scope for DB access and spawns its own
    /// short-lived <c>install.sh</c> process) and the
    /// <see cref="ProvisioningOrchestrator"/> as a hosted background service.
    ///
    /// <para>Uses <c>TryAddSingleton</c> for the runner so a test host can
    /// substitute a fake before calling this. Both the write endpoints and the
    /// orchestrator resolve the same singleton runner, so a config with a
    /// <c>Pending</c> endpoint provisions the same way whether it is launched by
    /// the orchestrator's kick pass or by a direct deployment write.</para>
    /// </summary>
    public static IServiceCollection AddProvisioningOrchestrator(this IServiceCollection services)
    {
        services.TryAddSingleton<DeployRunner>();
        services.AddHostedService<ProvisioningOrchestrator>();
        return services;
    }
}
