using Microsoft.Extensions.DependencyInjection.Extensions;
using Networker.ControlPlane.Provisioning;

namespace Microsoft.Extensions.DependencyInjection;

/// <summary>
/// DI wiring for the compute provisioner. Registers the CLI shell-out
/// implementation as the default <see cref="IComputeProvisioner"/>.
/// </summary>
public static class ProvisioningExtensions
{
    /// <summary>
    /// Register <see cref="IComputeProvisioner"/> → <see cref="CliComputeProvisioner"/>.
    ///
    /// <para>
    /// Uses <c>TryAddSingleton</c> so a test host (or a future SDK-backed
    /// provisioner) can register its own <see cref="IComputeProvisioner"/>
    /// <b>before</b> calling this and win — the swap point for mocking cloud
    /// calls without touching real CLIs. Singleton is safe: the provisioner is
    /// stateless (each call spawns its own short-lived process).
    /// </para>
    /// </summary>
    public static IServiceCollection AddComputeProvisioner(this IServiceCollection services)
    {
        services.TryAddSingleton<IComputeProvisioner, CliComputeProvisioner>();
        return services;
    }
}
