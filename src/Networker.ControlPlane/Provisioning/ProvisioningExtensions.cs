using Microsoft.Extensions.DependencyInjection.Extensions;
using Networker.ControlPlane.Provisioning;

namespace Microsoft.Extensions.DependencyInjection;

/// <summary>
/// DI wiring for the compute provisioner. Registers the CLI shell-out
/// implementation (az/aws/gcloud) plus the feature-flagged Docker (local)
/// provider behind a routing <see cref="IComputeProvisioner"/>.
/// </summary>
public static class ProvisioningExtensions
{
    /// <summary>
    /// Register <see cref="IComputeProvisioner"/> → <see cref="RoutingComputeProvisioner"/>
    /// (cloud "docker" → <see cref="DockerComputeProvisioner"/>, else
    /// <see cref="CliComputeProvisioner"/>), and <see cref="DockerProviderOptions"/>
    /// from the environment (<c>DASHBOARD_DOCKER_PROVIDER=1</c> enables the
    /// docker provider; default off).
    ///
    /// <para>
    /// Uses <c>TryAddSingleton</c> so a test host (or a future SDK-backed
    /// provisioner) can register its own <see cref="IComputeProvisioner"/>
    /// <b>before</b> calling this and win — the swap point for mocking cloud
    /// calls without touching real CLIs. Singleton is safe: the provisioners are
    /// stateless (each call spawns its own short-lived process).
    /// </para>
    /// </summary>
    public static IServiceCollection AddComputeProvisioner(this IServiceCollection services)
    {
        services.TryAddSingleton(_ => DockerProviderOptions.FromEnvironment());
        services.TryAddSingleton<CliComputeProvisioner>();
        services.TryAddSingleton<DockerComputeProvisioner>();
        services.TryAddSingleton<IComputeProvisioner, RoutingComputeProvisioner>();
        // Read-only cloud enumeration for GET /api/projects/{id}/inventory.
        // Singleton for the same reason as the provisioners: stateless, every
        // call spawns its own short-lived CLI processes.
        services.TryAddSingleton<CloudInventoryScanner>();
        return services;
    }
}
