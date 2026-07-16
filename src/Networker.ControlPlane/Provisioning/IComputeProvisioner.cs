using Networker.Data.Entities;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// Provider-agnostic VM lifecycle abstraction — the C# port of the Rust
/// dashboard's <c>services::cloud_provider::CloudProvider</c> dispatch enum
/// (crates/networker-dashboard/src/services/cloud_provider.rs).
///
/// <para>
/// "Parity first, SDKs later": the default implementation
/// (<see cref="CliComputeProvisioner"/>) shells out to the same cloud CLIs the
/// Rust provider drives — <c>az</c> / <c>aws</c> / <c>gcloud</c> — dispatching
/// on <see cref="ProjectTester.Cloud"/>. A future milestone can swap in the
/// native Azure/AWS/GCP SDKs behind this same interface without touching the
/// endpoints.
/// </para>
///
/// <para>
/// M4 slice 1 covers only the <b>power-lifecycle</b> operations that operate on
/// an <i>already-provisioned</i> VM (identified by
/// <see cref="ProjectTester.VmResourceId"/>): start, stop (deallocate), delete,
/// and show (state query). Actual VM <b>creation</b> from a Pending row — image
/// resolution, key-pair / security-group / SSH-metadata setup, cloud-init
/// bootstrap — is the deploy-runner's job (M4 slice 2) and is intentionally
/// <i>not</i> part of this interface.
/// </para>
///
/// <para>
/// Every method is total: it never throws for an infrastructure failure.
/// A missing CLI, a non-zero exit, or a spawn error is captured in the returned
/// <see cref="ProvisionResult"/> (<see cref="ProvisionResult.Success"/> ==
/// <c>false</c>). This keeps the calling endpoints testable — they do the DB
/// transition and return 202 regardless of whether a real cloud CLI exists.
/// </para>
/// </summary>
public interface IComputeProvisioner
{
    /// <summary>Start a stopped/deallocated VM (Azure <c>vm start</c>, AWS
    /// <c>start-instances</c>, GCP <c>instances start</c>).</summary>
    Task<ProvisionResult> StartAsync(
        ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default);

    /// <summary>Stop a running VM. On Azure this <b>deallocates</b>
    /// (stop-billing), matching the Rust <c>stop_vm</c> → <c>az vm deallocate</c>.
    /// AWS <c>stop-instances</c>, GCP <c>instances stop</c>.</summary>
    Task<ProvisionResult> StopAsync(
        ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default);

    /// <summary>Explicit deallocate alias for Azure (identical to
    /// <see cref="StopAsync"/> — the Rust <c>stop_vm</c> is a deallocate). Kept
    /// distinct so a caller that specifically wants "stop but keep billing"
    /// could diverge later; today all three providers route it the same as
    /// <see cref="StopAsync"/>.</summary>
    Task<ProvisionResult> DeallocateAsync(
        ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default);

    /// <summary>Permanently destroy a VM and its cascade-deletable resources
    /// (Azure <c>vm delete --yes</c>, AWS <c>terminate-instances</c>, GCP
    /// <c>instances delete --quiet</c>). Idempotent: "already gone" counts as
    /// success.</summary>
    Task<ProvisionResult> DeleteAsync(
        ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default);

    /// <summary>Query the current power state / public IP (Azure
    /// <c>vm show --show-details</c>, AWS <c>describe-instances</c>, GCP
    /// <c>instances describe</c>). The raw JSON is returned in
    /// <see cref="ProvisionResult.StdOut"/>.</summary>
    Task<ProvisionResult> ShowAsync(
        ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default);

    /// <summary>
    /// Create a brand-new VM — the C# port of the Rust
    /// <c>CloudProvider::create_vm</c> ("the heavy call"): Azure
    /// <c>az vm create</c> (+ <c>vm extension set</c> CustomScriptExtension for
    /// Windows bootstraps), AWS AMI-resolve → key-pair → security-group →
    /// <c>run-instances</c> → public-IP poll, GCP
    /// <c>compute instances create</c> with a startup-script. Total like the
    /// other operations: infrastructure failures are captured in the returned
    /// <see cref="VmCreateResult"/>, never thrown.
    /// </summary>
    Task<VmCreateResult> CreateVmAsync(
        VmCreateRequest request, ProviderCredentials? credentials, CancellationToken ct = default);
}

/// <summary>
/// Inputs for <see cref="IComputeProvisioner.CreateVmAsync"/> — the C# port of
/// the Rust <c>cloud_provider::VmConfig</c> (tags omitted: every Rust create
/// call site passes an empty tag map).
/// </summary>
/// <param name="Cloud">"azure" | "aws" | "gcp".</param>
/// <param name="Name">VM name (Rust <c>generate_vm_name</c> output).</param>
/// <param name="Region">Cloud region / location.</param>
/// <param name="VmSize">VM size / instance type / machine type.</param>
/// <param name="SshUser">Admin username (Rust <c>default_ssh_user</c>).</param>
/// <param name="Image">Provider-specific image ref (Azure URN / AWS marker /
/// GCP family) from <see cref="TesterCreateLogic.ResolveImage"/>.</param>
/// <param name="BootstrapScript">Optional cloud-init / user-data script —
/// Azure <c>--custom-data</c>, AWS <c>--user-data</c>, GCP
/// <c>--metadata-from-file startup-script=</c>.</param>
public sealed record VmCreateRequest(
    string Cloud,
    string Name,
    string Region,
    string VmSize,
    string SshUser,
    string Image,
    string? BootstrapScript);

/// <summary>
/// Result of a VM create — the Rust <c>VmInfo</c> folded into the total-result
/// convention (no throw for infrastructure failure).
/// </summary>
public sealed record VmCreateResult(
    bool Success,
    string? ResourceId,
    string? PublicIp,
    string? VmName,
    string? Error)
{
    public static VmCreateResult Created(string resourceId, string publicIp, string vmName) =>
        new(true, resourceId, publicIp, vmName, null);

    public static VmCreateResult Fail(string error) =>
        new(false, null, null, null, error);
}

/// <summary>
/// Optional per-connection credentials pulled from the <c>cloud_connection</c>
/// row's decrypted <c>config</c> JSON. When null the provisioner relies on the
/// host's ambient CLI auth (managed identity / instance profile / ADC) — the
/// same fallback the Rust providers use when no service-principal / key material
/// is present.
/// </summary>
/// <param name="Provider">"azure" | "aws" | "gcp" — normally equal to
/// <see cref="ProjectTester.Cloud"/>, carried separately so a mismatch can be
/// surfaced.</param>
/// <param name="SubscriptionId">Azure subscription id.</param>
/// <param name="ResourceGroup">Azure resource group.</param>
/// <param name="Region">AWS default region / GCP region.</param>
/// <param name="Extra">Raw remaining config values (access keys, tenant id,
/// json key, …) for future SDK use — not consumed by the CLI path today.</param>
public sealed record ProviderCredentials(
    string Provider,
    string? SubscriptionId = null,
    string? ResourceGroup = null,
    string? Region = null,
    IReadOnlyDictionary<string, string>? Extra = null);

/// <summary>
/// Typed result of a provisioner operation. Mirrors what the Rust code inspects
/// from <c>tokio::process::Command::output()</c>: the exit code plus captured
/// stdout/stderr. <see cref="Success"/> folds in the "already-gone is success"
/// idempotency the Rust delete paths implement.
/// </summary>
public sealed record ProvisionResult(
    bool Success,
    int? ExitCode,
    string StdOut,
    string StdErr,
    string? Error = null)
{
    /// <summary>Success carrying command output.</summary>
    public static ProvisionResult Ok(int exitCode, string stdOut, string stdErr) =>
        new(true, exitCode, stdOut, stdErr);

    /// <summary>A non-zero CLI exit.</summary>
    public static ProvisionResult Failed(int exitCode, string stdOut, string stdErr) =>
        new(false, exitCode, stdOut, stdErr, $"CLI exited with code {exitCode}");

    /// <summary>Could not spawn the CLI at all (missing binary, permission, …).
    /// The common CI case — treated as a soft failure the caller logs, never a
    /// request failure.</summary>
    public static ProvisionResult SpawnError(string message) =>
        new(false, null, string.Empty, string.Empty, message);

    /// <summary>An unsupported / unknown cloud provider.</summary>
    public static ProvisionResult Unsupported(string cloud) =>
        new(false, null, string.Empty, string.Empty, $"unsupported cloud provider: {cloud}");
}
