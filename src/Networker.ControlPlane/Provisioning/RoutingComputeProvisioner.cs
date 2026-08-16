using Networker.Data.Entities;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// The registered <see cref="IComputeProvisioner"/>: routes <c>cloud == "docker"</c>
/// to <see cref="DockerComputeProvisioner"/> (when the provider is enabled) and
/// everything else to <see cref="CliComputeProvisioner"/>. Every consumer keeps
/// depending on the one interface; the docker path is invisible unless
/// <c>DASHBOARD_DOCKER_PROVIDER=1</c>. With the flag off a docker-cloud row (only
/// possible if the flag was on when it was created) gets the same soft
/// "disabled" failure the cloud path yields for a missing CLI — never a crash.
/// </summary>
public sealed class RoutingComputeProvisioner(
    CliComputeProvisioner cli,
    DockerComputeProvisioner docker) : IComputeProvisioner
{
    private IComputeProvisioner For(string? cloud) =>
        DockerProviderOptions.IsDocker(cloud) ? docker : cli;

    public Task<ProvisionResult> StartAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default) =>
        For(tester.Cloud).StartAsync(tester, credentials, ct);

    public Task<ProvisionResult> StopAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default) =>
        For(tester.Cloud).StopAsync(tester, credentials, ct);

    public Task<ProvisionResult> DeallocateAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default) =>
        For(tester.Cloud).DeallocateAsync(tester, credentials, ct);

    public Task<ProvisionResult> DeleteAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default) =>
        For(tester.Cloud).DeleteAsync(tester, credentials, ct);

    public Task<ProvisionResult> ShowAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default) =>
        For(tester.Cloud).ShowAsync(tester, credentials, ct);

    public Task<ProvisionResult> RunCommandAsync(ProjectTester tester, ProviderCredentials? credentials, string script, CancellationToken ct = default) =>
        For(tester.Cloud).RunCommandAsync(tester, credentials, script, ct);

    public Task<VmCreateResult> CreateVmAsync(VmCreateRequest request, ProviderCredentials? credentials, CancellationToken ct = default) =>
        For(request.Cloud).CreateVmAsync(request, credentials, ct);

    public Task<ResolvedVm?> ResolveByEndpointAsync(string cloud, ProviderCredentials? credentials, string endpoint, CancellationToken ct = default) =>
        For(cloud).ResolveByEndpointAsync(cloud, credentials, endpoint, ct);
}
