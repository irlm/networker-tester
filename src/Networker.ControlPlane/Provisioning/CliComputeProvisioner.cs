using System.Diagnostics;
using System.Text.Json;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// <see cref="IComputeProvisioner"/> that shells out to the cloud vendor CLIs —
/// the faithful C# port of the Rust dashboard's
/// <c>services::cloud_provider</c> Azure/AWS/GCP providers.
///
/// <para>
/// The command strings are kept 1:1 with the Rust source so behaviour is
/// identical to the shipping dashboard:
/// <list type="bullet">
///   <item><b>Azure</b> (<c>az</c>): <c>vm start</c> / <c>vm deallocate</c> /
///     <c>vm delete --yes</c> / <c>vm show --show-details</c>, each with explicit
///     <c>--subscription</c> + <c>--resource-group</c> + <c>--ids &lt;resource_id&gt;</c>
///     (never ambient defaults), <c>PYTHONWARNINGS=ignore</c> set.</item>
///   <item><b>AWS</b> (<c>aws ec2</c>): <c>start-instances</c> /
///     <c>stop-instances</c> / <c>terminate-instances</c> /
///     <c>describe-instances</c> with <c>--instance-ids &lt;resource_id&gt;</c>.</item>
///   <item><b>GCP</b> (<c>gcloud compute instances</c>): <c>start</c> / <c>stop</c>
///     / <c>delete --quiet</c> / <c>describe --format json</c> with the instance
///     name + <c>--zone</c> parsed out of the selfLink resource id.</item>
/// </list>
/// </para>
///
/// <para>
/// Process handling reuses the hardened pattern from
/// <c>Networker.Agent.ProbeRunner</c>: streams drained concurrently and awaited
/// after exit (no pipe-buffer deadlock), a hard timeout that kills the whole
/// process tree, and <c>UseShellExecute=false</c> + <c>CreateNoWindow=true</c>.
/// </para>
///
/// <para>
/// No method throws for infrastructure failure — a missing CLI or non-zero exit
/// becomes a <see cref="ProvisionResult"/> with <see cref="ProvisionResult.Success"/>
/// == false. Delete treats "resource already gone" as success (idempotent),
/// matching the Rust <c>ResourceNotFound</c> / <c>InvalidInstanceID.NotFound</c>
/// / <c>was not found</c> handling.
/// </para>
/// </summary>
public sealed class CliComputeProvisioner(ILogger<CliComputeProvisioner> logger) : IComputeProvisioner
{
    /// <summary>Hard ceiling for a single CLI invocation. Azure control-plane
    /// calls (deallocate especially) can take a while; the tree-kill guard
    /// prevents a wedged CLI from leaking.</summary>
    private static readonly TimeSpan CommandTimeout = TimeSpan.FromMinutes(5);

    /// <summary>
    /// Ceiling for the create-path CLI invocations. The Rust <c>create_vm</c>
    /// runs <b>unbounded</b> (<c>Command::output().await</c> with no timeout);
    /// a Windows create synchronously installs CustomScriptExtension and can
    /// legitimately take 5-10+ minutes. We bound at 30 minutes instead of
    /// forever so a wedged CLI can't leak a background task — a deliberate,
    /// narrow divergence from the Rust source (documented in the PR).
    /// </summary>
    private static readonly TimeSpan CreateTimeout = TimeSpan.FromMinutes(30);

    public Task<ProvisionResult> StartAsync(
        ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct) =>
        DispatchAsync(tester, credentials, LifecycleOp.Start, ct);

    public Task<ProvisionResult> StopAsync(
        ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct) =>
        DispatchAsync(tester, credentials, LifecycleOp.Stop, ct);

    // Deallocate is the same wire op as Stop on every provider today (Azure's
    // stop == deallocate in the Rust source); kept as a distinct entry point.
    public Task<ProvisionResult> DeallocateAsync(
        ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct) =>
        DispatchAsync(tester, credentials, LifecycleOp.Stop, ct);

    public Task<ProvisionResult> DeleteAsync(
        ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct) =>
        DispatchAsync(tester, credentials, LifecycleOp.Delete, ct);

    public Task<ProvisionResult> ShowAsync(
        ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct) =>
        DispatchAsync(tester, credentials, LifecycleOp.Show, ct);

    private enum LifecycleOp { Start, Stop, Delete, Show }

    private async Task<ProvisionResult> DispatchAsync(
        ProjectTester tester, ProviderCredentials? credentials, LifecycleOp op, CancellationToken ct)
    {
        var resourceId = tester.VmResourceId;
        if (string.IsNullOrEmpty(resourceId))
        {
            return ProvisionResult.SpawnError(
                "tester has no vm_resource_id; nothing to act on at the cloud layer");
        }

        var cloud = tester.Cloud?.ToLowerInvariant();
        var (file, args, env) = cloud switch
        {
            "azure" => BuildAzure(op, resourceId, credentials),
            "aws" => BuildAws(op, resourceId, credentials),
            "gcp" => BuildGcp(op, resourceId),
            _ => (null, null, null),
        };

        if (file is null || args is null)
        {
            return ProvisionResult.Unsupported(tester.Cloud ?? "(null)");
        }

        var result = await RunAsync(file, args, env, ct).ConfigureAwait(false);

        // Idempotent delete: "already gone" is the desired end-state. Mirrors the
        // Rust delete_vm paths that swallow ResourceNotFound / NotFound / 404.
        if (op == LifecycleOp.Delete && !result.Success && LooksAlreadyGone(result.StdErr))
        {
            logger.LogInformation(
                "{Cloud} resource {ResourceId} already gone; treating delete as success",
                cloud, resourceId);
            result = ProvisionResult.Ok(result.ExitCode ?? 0, result.StdOut, result.StdErr);
        }

        // Azure self-cleaning cascade: `az vm create` makes a per-VM NSG
        // (`<vm_name>NSG`) and public IP (`<vm_name>PublicIP`) that the VM's
        // create-time --nic-delete-option / --os-disk-delete-option do NOT cover,
        // so `az vm delete` leaves them behind to bill until the (eventual,
        // account-scoped) OrphanReaperService gets them. Delete them immediately
        // here, best-effort — the VM delete outcome (already reflected in
        // `result`) stays authoritative; NSG/IP cleanup never changes it.
        // AWS terminate-instances / GCP delete already release their own resources.
        if (op == LifecycleOp.Delete && result.Success && cloud == "azure")
        {
            await CleanupAzureNetworkResourcesAsync(tester, credentials, ct).ConfigureAwait(false);
        }

        return result;
    }

    /// <summary>
    /// Best-effort deletion of the per-VM Azure NSG (<c>&lt;vm_name&gt;NSG</c>) and
    /// public IP (<c>&lt;vm_name&gt;PublicIP</c>) that <c>az vm delete</c> leaves
    /// behind (the create-time delete-options only cover the NIC + OS disk).
    ///
    /// <para><b>Safety (the #419 reaper incident):</b> the orphan reaper once
    /// deleted live testers' children because it matched on a set instead of exact
    /// names. This path matches ONLY the two exact names derived from THIS tester's
    /// <see cref="ProjectTester.VmName"/> — never a prefix, wildcard, or list
    /// filter that could hit another tester. If <c>vm_name</c> is null/empty we
    /// can't derive the names safely, so we SKIP entirely rather than guess.</para>
    ///
    /// <para><b>Order:</b> IP before NSG. The NIC (which references the NSG) is
    /// already gone with the VM cascade, so neither strictly blocks the other, but
    /// IP-then-NSG is the conventionally safe order.</para>
    ///
    /// <para>Never throws and never fails the tester delete: a not-found /
    /// already-deleted resource (the reaper or a partial cascade may have gotten
    /// it) is logged at debug, other failures at info. The overall delete result
    /// reflects the VM delete, not this cleanup.</para>
    /// </summary>
    private async Task CleanupAzureNetworkResourcesAsync(
        ProjectTester tester, ProviderCredentials? creds, CancellationToken ct)
    {
        var vmName = tester.VmName;
        if (string.IsNullOrEmpty(vmName))
        {
            // No vm_name → can't derive `<vm_name>NSG` / `<vm_name>PublicIP` safely.
            logger.LogDebug(
                "Azure tester {ResourceId} has no vm_name; skipping NSG/IP cascade cleanup",
                tester.VmResourceId);
            return;
        }

        var file = CloudCli.AzBin();

        // The NSG/IP deletes target by --name, so they NEED an explicit
        // --subscription + --resource-group (unlike `az vm delete`, which uses
        // the self-describing --ids). In the real delete flow the creds object's
        // SubscriptionId/ResourceGroup are empty (scope lives in the resource id),
        // so derive them from the VM resource id first, falling back to creds.
        // Empty scope → az fails with a bogus API-version dump and the resources
        // leak to the reaper (found live 2026-07-21).
        var (idSub, idRg) = ParseAzureScope(tester.VmResourceId);
        var sub = FirstNonEmpty(idSub, creds?.SubscriptionId);
        var rg = FirstNonEmpty(idRg, creds?.ResourceGroup);
        if (string.IsNullOrEmpty(sub) || string.IsNullOrEmpty(rg))
        {
            logger.LogInformation(
                "Azure cascade: no subscription/resource-group for {VmName} (id={ResourceId}); leaving NSG/IP for the reaper",
                vmName, tester.VmResourceId);
            return;
        }
        var env = new Dictionary<string, string> { ["PYTHONWARNINGS"] = "ignore" };

        // IP first, then NSG (see method summary).
        await RunAzureNetworkDeleteAsync(
            file, BuildAzurePublicIpDeleteArgs(vmName, sub, rg), env, "public IP", $"{vmName}PublicIP", ct)
            .ConfigureAwait(false);
        await RunAzureNetworkDeleteAsync(
            file, BuildAzureNsgDeleteArgs(vmName, sub, rg), env, "NSG", $"{vmName}NSG", ct)
            .ConfigureAwait(false);
    }

    private static string FirstNonEmpty(string? a, string? b) =>
        !string.IsNullOrEmpty(a) ? a : (b ?? string.Empty);

    /// <summary>
    /// Parse the subscription id + resource group out of an Azure VM resource id
    /// (<c>/subscriptions/{sub}/resourceGroups/{rg}/providers/…</c>,
    /// case-insensitive). Returns empty strings if a segment is absent.
    /// </summary>
    internal static (string Subscription, string ResourceGroup) ParseAzureScope(string? resourceId)
    {
        if (string.IsNullOrEmpty(resourceId))
        {
            return (string.Empty, string.Empty);
        }

        var parts = resourceId.Split('/', StringSplitOptions.RemoveEmptyEntries);
        var sub = string.Empty;
        var rg = string.Empty;
        for (var i = 0; i + 1 < parts.Length; i++)
        {
            if (parts[i].Equals("subscriptions", StringComparison.OrdinalIgnoreCase))
            {
                sub = parts[i + 1];
            }
            else if (parts[i].Equals("resourceGroups", StringComparison.OrdinalIgnoreCase))
            {
                rg = parts[i + 1];
            }
        }

        return (sub, rg);
    }

    /// <summary>Cascade NSG/IP delete: total attempts including the first.</summary>
    internal static int NetworkDeleteMaxAttempts = 4;

    /// <summary>Backoff between cascade delete attempts (test-lowered to ~0).</summary>
    internal static TimeSpan NetworkDeleteRetryDelay = TimeSpan.FromSeconds(12);

    private async Task RunAzureNetworkDeleteAsync(
        string file, List<string> args, IReadOnlyDictionary<string, string> env,
        string kind, string name, CancellationToken ct)
    {
        ProvisionResult res = default!;
        for (var attempt = 1; attempt <= NetworkDeleteMaxAttempts; attempt++)
        {
            res = await RunAsync(file, args, env, ct).ConfigureAwait(false);
            if (res.Success || LooksAlreadyGone(res.StdErr))
            {
                logger.LogDebug("Azure cascade: {Kind} {Name} deleted (or already gone)", kind, name);
                return;
            }

            // The per-VM NSG/IP can't be deleted until Azure finishes tearing down
            // the NIC the `az vm delete` cascade removes asynchronously — the delete
            // races that teardown and fails "in use". Wait + retry so the cascade
            // wins immediately instead of deferring to the reaper's next sweep.
            if (IsRetryableInUse(res.StdErr) && attempt < NetworkDeleteMaxAttempts)
            {
                logger.LogDebug(
                    "Azure cascade: {Kind} {Name} still in use (attempt {Attempt}/{Max}); retrying after {Delay}s",
                    kind, name, attempt, NetworkDeleteMaxAttempts, NetworkDeleteRetryDelay.TotalSeconds);
                try
                {
                    await Task.Delay(NetworkDeleteRetryDelay, ct).ConfigureAwait(false);
                }
                catch (OperationCanceledException)
                {
                    break;
                }
                continue;
            }

            break;
        }

        // Exhausted retries (or a non-retryable failure) — best-effort: the
        // OrphanReaperService will collect it on its next sweep. Log at info (not
        // error): a leftover NSG/IP is a cost nuisance, not a failed delete, and
        // must never surface as a tester-delete error.
        logger.LogInformation(
            "Azure cascade: best-effort {Kind} {Name} delete did not succeed ({Err}); leaving for the orphan reaper",
            kind, name, res.Error ?? res.StdErr);
    }

    /// <summary>
    /// A cascade NSG/IP delete that failed because the resource is still attached
    /// to a NIC/VM Azure is asynchronously tearing down — retryable, unlike a
    /// permission or malformed-request error.
    /// </summary>
    internal static bool IsRetryableInUse(string stderr) =>
        stderr.Contains("in use", StringComparison.OrdinalIgnoreCase)
        || stderr.Contains("InUse", StringComparison.OrdinalIgnoreCase)
        || stderr.Contains("is being used", StringComparison.OrdinalIgnoreCase)
        || stderr.Contains("still allocated", StringComparison.OrdinalIgnoreCase)
        || stderr.Contains("cannot be deleted", StringComparison.OrdinalIgnoreCase)
        || stderr.Contains("referenced by", StringComparison.OrdinalIgnoreCase)
        || stderr.Contains("reference", StringComparison.OrdinalIgnoreCase);

    /// <summary>
    /// Pure argv builder for deleting the per-VM Azure NSG by its <b>exact</b> name
    /// (<c>&lt;vmName&gt;NSG</c>) in the given subscription + resource group. Uses
    /// <c>--name</c> (never a list/filter) so it can only ever target this tester's
    /// NSG. Testable without a process spawn.
    /// </summary>
    public static List<string> BuildAzureNsgDeleteArgs(string vmName, string subscription, string resourceGroup) =>
        new()
        {
            "network", "nsg", "delete",
            "--subscription", subscription,
            "--resource-group", resourceGroup,
            "--name", $"{vmName}NSG",
        };

    /// <summary>
    /// Pure argv builder for deleting the per-VM Azure public IP by its
    /// <b>exact</b> name (<c>&lt;vmName&gt;PublicIP</c>) in the given subscription +
    /// resource group. Uses <c>--name</c> (never a list/filter). Testable without a
    /// process spawn.
    /// </summary>
    public static List<string> BuildAzurePublicIpDeleteArgs(string vmName, string subscription, string resourceGroup) =>
        new()
        {
            "network", "public-ip", "delete",
            "--subscription", subscription,
            "--resource-group", resourceGroup,
            "--name", $"{vmName}PublicIP",
        };

    // ── Azure argv (az) ──────────────────────────────────────────────────────

    private static (string file, List<string> args, IReadOnlyDictionary<string, string>? env)
        BuildAzure(LifecycleOp op, string resourceId, ProviderCredentials? creds)
    {
        // `az` resolution honours the AZ_CMD override the Rust az_bin() uses so
        // the same dev shim works across both stacks (see CloudCli).
        var file = CloudCli.AzBin();

        var sub = creds?.SubscriptionId ?? string.Empty;
        var rg = creds?.ResourceGroup ?? string.Empty;

        var verb = op switch
        {
            LifecycleOp.Start => "start",
            LifecycleOp.Stop => "deallocate", // Rust stop_vm → az vm deallocate
            LifecycleOp.Delete => "delete",
            _ => "show",
        };

        var args = BuildAzureLifecycleArgs(
            verb, sub, rg, resourceId,
            isDelete: op == LifecycleOp.Delete,
            isShow: op == LifecycleOp.Show);

        // PYTHONWARNINGS=ignore keeps az's Python SyntaxWarnings out of stdout so
        // JSON parsing stays clean — same as the Rust az_cmd().
        var env = new Dictionary<string, string> { ["PYTHONWARNINGS"] = "ignore" };
        return (file, args, env);
    }

    /// <summary>
    /// Build the <c>az vm &lt;verb&gt;</c> argv for a power-lifecycle op on an
    /// existing VM (<c>--ids</c>). Passes <c>--subscription</c> /
    /// <c>--resource-group</c> ONLY when non-empty: under ambient CLI auth
    /// (managed identity / <c>az login</c>) the credentials are absent → sub/rg
    /// are "", and az errors before dispatch on an empty <c>--subscription ""</c>,
    /// so AutoShutdown treated the non-zero exit as a real failure and retried
    /// every 60 s tick forever (quality audit F9). The ARM <c>--ids</c> is
    /// self-describing (it encodes the subscription + resource group), so both
    /// scope flags are optional. Public so the arg-shape is unit-tested directly.
    /// </summary>
    public static List<string> BuildAzureLifecycleArgs(
        string verb, string subscription, string resourceGroup, string resourceId,
        bool isDelete, bool isShow)
    {
        var args = new List<string> { "vm", verb };
        if (!string.IsNullOrEmpty(subscription))
        {
            args.Add("--subscription");
            args.Add(subscription);
        }
        if (!string.IsNullOrEmpty(resourceGroup))
        {
            args.Add("--resource-group");
            args.Add(resourceGroup);
        }
        args.Add("--ids");
        args.Add(resourceId);
        if (isDelete)
        {
            args.Add("--yes");
        }
        if (isShow)
        {
            args.Add("--show-details");
            args.Add("--output");
            args.Add("json");
        }
        return args;
    }

    // ── AWS argv (aws ec2) ───────────────────────────────────────────────────

    private static (string file, List<string> args, IReadOnlyDictionary<string, string>? env)
        BuildAws(LifecycleOp op, string resourceId, ProviderCredentials? creds)
    {
        var verb = op switch
        {
            LifecycleOp.Start => "start-instances",
            LifecycleOp.Stop => "stop-instances",
            LifecycleOp.Delete => "terminate-instances",
            _ => "describe-instances",
        };

        var args = new List<string> { "ec2", verb, "--instance-ids", resourceId };
        if (op == LifecycleOp.Show)
        {
            args.Add("--query");
            args.Add("Reservations[0].Instances[0]");
            args.Add("--output");
            args.Add("json");
        }

        // Region + credentials via env, matching aws_cmd(). Access keys, if
        // present in Extra, are forwarded so a connection-scoped identity works;
        // otherwise the host's ambient profile is used.
        var env = new Dictionary<string, string>();
        if (creds?.Region is { Length: > 0 } region)
        {
            env["AWS_DEFAULT_REGION"] = region;
        }
        if (creds?.Extra is { } extra)
        {
            if (extra.TryGetValue("access_key_id", out var ak)) env["AWS_ACCESS_KEY_ID"] = ak;
            if (extra.TryGetValue("secret_access_key", out var sk)) env["AWS_SECRET_ACCESS_KEY"] = sk;
            if (extra.TryGetValue("session_token", out var st)) env["AWS_SESSION_TOKEN"] = st;
        }
        return (file: CloudCli.AwsBin(), args, env: env.Count > 0 ? env : null);
    }

    // ── GCP argv (gcloud compute instances) ──────────────────────────────────

    private static (string file, List<string> args, IReadOnlyDictionary<string, string>? env)
        BuildGcp(LifecycleOp op, string resourceId)
    {
        var (name, zone) = ParseGcpResourceId(resourceId);

        var verb = op switch
        {
            LifecycleOp.Start => "start",
            LifecycleOp.Stop => "stop",
            LifecycleOp.Delete => "delete",
            _ => "describe",
        };

        var args = new List<string> { "compute", "instances", verb, name, "--zone", zone };
        if (op == LifecycleOp.Delete)
        {
            args.Add("--quiet");
        }
        if (op == LifecycleOp.Show)
        {
            args.Add("--format");
            args.Add("json");
        }
        return (file: CloudCli.GcloudBin(), args, env: null);
    }

    /// <summary>
    /// Parse a GCP selfLink resource id into (name, zone). Mirrors the Rust
    /// <c>parse_gcp_resource_id</c>. Format:
    /// <c>https://.../projects/PROJECT/zones/ZONE/instances/NAME</c>.
    /// </summary>
    internal static (string Name, string Zone) ParseGcpResourceId(string resourceId)
    {
        var parts = resourceId.Split('/');
        var name = parts.Length > 0 ? parts[^1] : string.Empty;
        var zone = string.Empty;
        for (var i = 0; i < parts.Length - 1; i++)
        {
            if (parts[i] == "zones")
            {
                zone = parts[i + 1];
                break;
            }
        }
        return (name, zone);
    }

    // ── VM create (Rust CloudProvider::create_vm) ────────────────────────────

    public async Task<VmCreateResult> CreateVmAsync(
        VmCreateRequest request, ProviderCredentials? credentials, CancellationToken ct = default)
    {
        try
        {
            return request.Cloud.ToLowerInvariant() switch
            {
                "azure" => await CreateAzureVmAsync(request, credentials, ct).ConfigureAwait(false),
                "aws" => await CreateAwsVmAsync(request, credentials, ct).ConfigureAwait(false),
                "gcp" => await CreateGcpVmAsync(request, credentials, ct).ConfigureAwait(false),
                _ => VmCreateResult.Fail($"unsupported cloud provider: {request.Cloud}"),
            };
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            throw;
        }
        catch (Exception ex)
        {
            // Total like every other provisioner method: tempfile / IO trouble
            // becomes a failed result, never an unhandled background exception.
            logger.LogError(ex, "CreateVmAsync for {Cloud} VM {Name} threw", request.Cloud, request.Name);
            return VmCreateResult.Fail(ex.Message);
        }
    }

    private static string? ExtraValue(ProviderCredentials? creds, string key) =>
        creds?.Extra is { } extra && extra.TryGetValue(key, out var v) && v.Length > 0 ? v : null;

    // ── Azure create ─────────────────────────────────────────────────────────

    private async Task<VmCreateResult> CreateAzureVmAsync(
        VmCreateRequest request, ProviderCredentials? creds, CancellationToken ct)
    {
        var azBin = CloudCli.AzBin();
        var sub = creds?.SubscriptionId ?? string.Empty;
        var rg = creds?.ResourceGroup ?? string.Empty;

        // ensure_sp_login: when the account carries service-principal creds,
        // log in to an isolated AZURE_CONFIG_DIR; managed identity uses the
        // host's ambient az session.
        string? spConfigDir = null;
        var identityType = ExtraValue(creds, "identity_type") ?? "managed_identity";
        if (identityType == "service_principal"
            && ExtraValue(creds, "client_id") is { } clientId
            && ExtraValue(creds, "client_secret") is { } clientSecret
            && ExtraValue(creds, "tenant_id") is { } tenantId)
        {
            // 0700 so the token cache az writes here (the access token minted
            // from the SP secret) isn't world-readable (quality audit F11).
            spConfigDir = Path.Combine(Path.GetTempPath(), $"az-sp-{Guid.NewGuid():N}");
            SecretFile.CreateDir0700(spConfigDir);

            // Feed the client secret through a 0600 file via az's @file loading
            // instead of putting it on argv (world-visible in ps/proc for the
            // whole login). NOT env vars: az CLI login reads -u/-p from argv, the
            // AZURE_CLIENT_* env vars are honoured only by the SDKs, not `az login`.
            var spSecretFile = Path.Combine(Path.GetTempPath(), $"az-sp-secret-{Guid.NewGuid():N}");
            await SecretFile.WriteAsync(spSecretFile, clientSecret, ct).ConfigureAwait(false);
            ProvisionResult login;
            try
            {
                login = await RunAsync(
                    azBin,
                    new List<string>
                    {
                        "login", "--service-principal", "-u", clientId, "-p", $"@{spSecretFile}",
                        "--tenant", tenantId, "--output", "none",
                    },
                    new Dictionary<string, string>
                    {
                        ["AZURE_CONFIG_DIR"] = spConfigDir,
                        ["PYTHONWARNINGS"] = "ignore",
                    },
                    ct,
                    CreateTimeout,
                    sensitiveArgs: true).ConfigureAwait(false);
            }
            finally
            {
                TryDeleteFile(spSecretFile);
            }
            if (!login.Success)
            {
                TryDeleteDir(spConfigDir);
                return VmCreateResult.Fail(
                    $"az login --service-principal failed: {login.Error ?? login.StdErr}");
            }
        }

        try
        {
            var env = new Dictionary<string, string> { ["PYTHONWARNINGS"] = "ignore" };
            if (spConfigDir is not null)
            {
                env["AZURE_CONFIG_DIR"] = spConfigDir;
            }

            var isWindows = request.Image.Contains("windows", StringComparison.OrdinalIgnoreCase);

            // Windows VMs require a password; Linux VMs use SSH keys. Same
            // generation rule as Rust (Azure: 12-72 chars, 3 character classes).
            var winPassword = isWindows
                ? $"Nx!{Guid.NewGuid():N}{new string(request.Name.Take(4).ToArray())}aZ9"
                : null;

            // The admin password goes through a 0600 file via az's @file loading
            // rather than onto argv (world-visible in ps for the whole create).
            string? winPasswordFile = null;
            if (winPassword is not null)
            {
                winPasswordFile = Path.Combine(Path.GetTempPath(), $"az-winpw-{Guid.NewGuid():N}");
                await SecretFile.WriteAsync(winPasswordFile, winPassword, ct).ConfigureAwait(false);
            }

            // The bootstrap script embeds the minted agent API key — 0600 it too.
            string? customDataPath = null;
            if (request.BootstrapScript is { } script)
            {
                customDataPath = Path.GetTempFileName();
                await SecretFile.WriteAsync(customDataPath, script, ct).ConfigureAwait(false);
            }

            try
            {
                var args = BuildAzureCreateArgs(request, sub, rg, winPasswordFile, customDataPath);
                // Windows argv carries --admin-password @file — never log it.
                var res = await RunAsync(azBin, args, env, ct, CreateTimeout, sensitiveArgs: isWindows)
                    .ConfigureAwait(false);
                if (!res.Success)
                {
                    return VmCreateResult.Fail($"az vm create failed: {res.Error ?? res.StdErr}");
                }

                // Strip any non-JSON prefix (az may print warnings before JSON).
                var stdout = res.StdOut;
                var jsonStart = stdout.IndexOf('{');
                if (jsonStart < 0)
                {
                    jsonStart = 0;
                }

                string? publicIp;
                string? resourceId;
                try
                {
                    using var doc = JsonDocument.Parse(stdout[jsonStart..]);
                    var root = doc.RootElement;
                    publicIp = root.TryGetProperty("publicIpAddress", out var ip) && ip.ValueKind == JsonValueKind.String
                        ? ip.GetString()
                        : null;
                    resourceId = root.TryGetProperty("id", out var id) && id.ValueKind == JsonValueKind.String
                        ? id.GetString()
                        : null;
                }
                catch (JsonException e)
                {
                    return VmCreateResult.Fail($"az vm create produced non-JSON output: {e.Message}");
                }

                if (publicIp is null)
                {
                    return VmCreateResult.Fail("az vm create: missing publicIpAddress");
                }

                if (resourceId is null)
                {
                    return VmCreateResult.Fail("az vm create: missing id");
                }

                // On Windows, --custom-data only DROPS the script at
                // C:\AzureData\CustomData.bin; wire CustomScriptExtension so the
                // bootstrap actually runs (Rust run_windows_bootstrap_extension).
                if (isWindows && request.BootstrapScript is not null)
                {
                    var extRes = await RunWindowsBootstrapExtensionAsync(
                        azBin, env, sub, rg, request.Name, ct).ConfigureAwait(false);
                    if (!extRes.Success)
                    {
                        // The VM already exists and bills — carry its known
                        // resourceId on the failure so the caller can record it
                        // for the reaper / operator cleanup instead of orphaning a
                        // billing VM with no DB record (quality audit F8).
                        return VmCreateResult.Fail(
                            $"az vm extension set failed: {extRes.Error ?? extRes.StdErr}",
                            resourceId);
                    }
                }

                return VmCreateResult.Created(resourceId, publicIp, request.Name);
            }
            finally
            {
                if (customDataPath is not null)
                {
                    TryDeleteFile(customDataPath);
                }
                if (winPasswordFile is not null)
                {
                    TryDeleteFile(winPasswordFile);
                }
            }
        }
        finally
        {
            if (spConfigDir is not null)
            {
                TryDeleteDir(spConfigDir);
            }
        }
    }

    private async Task<ProvisionResult> RunWindowsBootstrapExtensionAsync(
        string azBin,
        IReadOnlyDictionary<string, string> env,
        string sub,
        string rg,
        string vmName,
        CancellationToken ct)
    {
        // Rename CustomData.bin → CustomData.ps1 so PowerShell recognises it,
        // then invoke it, teeing everything to bootstrap.log. Byte-identical
        // to the Rust command string.
        const string command = "powershell -ExecutionPolicy Bypass -NoProfile -Command "
            + "\"Copy-Item 'C:\\AzureData\\CustomData.bin' 'C:\\AzureData\\CustomData.ps1' -Force; "
            + "& 'C:\\AzureData\\CustomData.ps1' *> 'C:\\AzureData\\bootstrap.log'\"";
        var protectedSettings = JsonSerializer.Serialize(new { commandToExecute = command });

        var settingsPath = Path.GetTempFileName();
        // Protected settings carry secrets (bootstrap incl. the API key) — 0600.
        await SecretFile.WriteAsync(settingsPath, protectedSettings, ct).ConfigureAwait(false);
        try
        {
            return await RunAsync(
                azBin,
                new List<string>
                {
                    "vm", "extension", "set",
                    "--subscription", sub,
                    "--resource-group", rg,
                    "--vm-name", vmName,
                    "--name", "CustomScriptExtension",
                    "--publisher", "Microsoft.Compute",
                    "--version", "1.10",
                    "--protected-settings", $"@{settingsPath}",
                },
                env,
                ct,
                CreateTimeout).ConfigureAwait(false);
        }
        finally
        {
            TryDeleteFile(settingsPath);
        }
    }

    /// <summary>
    /// Pure argv builder for <c>az vm create</c> — the C# port of the Rust
    /// <c>AzureProvider::build_vm_create_args</c> (same flags, same order; tags
    /// omitted because every Rust create call site passes an empty tag map).
    ///
    /// <para><paramref name="adminPasswordFile"/> is the path to a 0600 file
    /// holding the Windows admin password; it is passed as
    /// <c>--admin-password @&lt;path&gt;</c> so the secret never appears on argv
    /// (quality audit F11). Null for Linux (SSH keys instead).</para>
    /// </summary>
    public static List<string> BuildAzureCreateArgs(
        VmCreateRequest request,
        string subscriptionId,
        string resourceGroup,
        string? adminPasswordFile,
        string? customDataPath)
    {
        var isWindows = request.Image.Contains("windows", StringComparison.OrdinalIgnoreCase);
        var args = new List<string>
        {
            "vm", "create",
            "--subscription", subscriptionId,
            "--resource-group", resourceGroup,
            "--name", request.Name,
            "--location", request.Region,
            "--image", request.Image,
            "--size", request.VmSize,
            "--public-ip-sku", "Standard",
            "--admin-username", request.SshUser,
            // Cascade-delete attached resources with the VM (otherwise the OS
            // disk, NIC and public IP keep billing after the tester is gone).
            "--os-disk-delete-option", "Delete",
            "--nic-delete-option", "Delete",
        };

        if (isWindows)
        {
            args.Add("--computer-name");
            args.Add(AzureWindowsComputerName(request.Name));
        }

        if (isWindows)
        {
            if (adminPasswordFile is not null)
            {
                // @file → az loads the password from the 0600 file; keeps it off
                // argv (quality audit F11).
                args.Add("--admin-password");
                args.Add($"@{adminPasswordFile}");
            }
        }
        else
        {
            args.Add("--generate-ssh-keys");
        }

        args.Add("--output");
        args.Add("json");

        if (customDataPath is not null)
        {
            args.Add("--custom-data");
            args.Add($"@{customDataPath}");
        }

        return args;
    }

    /// <summary>
    /// Windows-safe NetBIOS computer name — the C# port of the Rust
    /// <c>azure_windows_computer_name</c>: ≤15 chars, alphanumerics + hyphens,
    /// collapsed doubles, trailing hyphens stripped, "w" prefix when the result
    /// would be empty or all-numeric.
    /// </summary>
    public static string AzureWindowsComputerName(string name)
    {
        var s = new string(name.Select(c => char.IsAsciiLetterOrDigit(c) ? c : '-').ToArray());
        while (s.Contains("--", StringComparison.Ordinal))
        {
            s = s.Replace("--", "-", StringComparison.Ordinal);
        }

        if (s.Length > 15)
        {
            s = s[..15];
        }

        s = s.TrimEnd('-');
        if (s.Length == 0 || s.All(char.IsAsciiDigit))
        {
            s = $"w{s}";
            if (s.Length > 15)
            {
                s = s[..15];
            }
        }

        return s;
    }

    // ── AWS create ───────────────────────────────────────────────────────────

    private async Task<VmCreateResult> CreateAwsVmAsync(
        VmCreateRequest request, ProviderCredentials? creds, CancellationToken ct)
    {
        // aws_cmd(): credentials + default region via env.
        var env = new Dictionary<string, string>();
        if (ExtraValue(creds, "access_key_id") is { } ak)
        {
            env["AWS_ACCESS_KEY_ID"] = ak;
        }

        if (ExtraValue(creds, "secret_access_key") is { } sk)
        {
            env["AWS_SECRET_ACCESS_KEY"] = sk;
        }

        if (ExtraValue(creds, "session_token") is { } st)
        {
            env["AWS_SESSION_TOKEN"] = st;
        }

        env["AWS_DEFAULT_REGION"] = creds?.Region is { Length: > 0 } r ? r : request.Region;

        // 1. Resolve the AMI from the "aws:<os-variant>" marker.
        var (owner, nameFilter) = AwsAmiFilter(request.Image);
        var amiRes = await RunAsync(
            CloudCli.AwsBin(),
            new List<string>
            {
                "ec2", "describe-images",
                "--owners", owner,
                "--filters", $"Name=name,Values={nameFilter}", "Name=state,Values=available",
                "--query", "sort_by(Images, &CreationDate)[-1].ImageId",
                "--region", request.Region,
                "--output", "text",
            },
            env,
            ct,
            CreateTimeout).ConfigureAwait(false);
        if (!amiRes.Success)
        {
            return VmCreateResult.Fail($"aws ec2 describe-images failed: {amiRes.Error ?? amiRes.StdErr}");
        }

        var amiId = amiRes.StdOut.Trim();
        if (amiId.Length == 0 || amiId == "None")
        {
            return VmCreateResult.Fail($"No AMI found for '{request.Image}' in region {request.Region}");
        }

        // 2. Ensure key pair + security group (idempotent).
        var keyName = await EnsureAwsKeyPairAsync(env, request.Region, ct).ConfigureAwait(false);
        if (keyName.Error is not null)
        {
            return VmCreateResult.Fail(keyName.Error);
        }

        var sg = await EnsureAwsSecurityGroupAsync(env, request.Region, ct).ConfigureAwait(false);
        if (sg.Error is not null)
        {
            return VmCreateResult.Fail(sg.Error);
        }

        // 3. run-instances (user-data via tempfile when a bootstrap is set).
        string? userDataPath = null;
        if (request.BootstrapScript is { } script)
        {
            userDataPath = Path.GetTempFileName();
            // Cloud-init user-data embeds the minted agent API key — 0600.
            await SecretFile.WriteAsync(userDataPath, script, ct).ConfigureAwait(false);
        }

        ProvisionResult runRes;
        try
        {
            var args = BuildAwsRunInstancesArgs(request, amiId, keyName.Value!, sg.Value!, userDataPath);
            runRes = await RunAsync(CloudCli.AwsBin(), args, env, ct, CreateTimeout).ConfigureAwait(false);
        }
        finally
        {
            if (userDataPath is not null)
            {
                TryDeleteFile(userDataPath);
            }
        }

        if (!runRes.Success)
        {
            return VmCreateResult.Fail($"aws ec2 run-instances failed: {runRes.Error ?? runRes.StdErr}");
        }

        string? instanceId;
        try
        {
            using var doc = JsonDocument.Parse(runRes.StdOut);
            instanceId = doc.RootElement.TryGetProperty("InstanceId", out var iid)
                         && iid.ValueKind == JsonValueKind.String
                ? iid.GetString()
                : null;
        }
        catch (JsonException)
        {
            return VmCreateResult.Fail("aws ec2 run-instances produced non-JSON output");
        }

        if (instanceId is null)
        {
            return VmCreateResult.Fail("missing InstanceId");
        }

        // 4. Public IP isn't immediate — poll up to 60s (30 × 2s), soft-fail to
        //    empty like the Rust unwrap_or_default().
        var publicIp = string.Empty;
        for (var i = 0; i < 30; i++)
        {
            var ipRes = await RunAsync(
                CloudCli.AwsBin(),
                new List<string>
                {
                    "ec2", "describe-instances",
                    "--instance-ids", instanceId,
                    "--query", "Reservations[0].Instances[0].PublicIpAddress",
                    "--region", request.Region,
                    "--output", "text",
                },
                env,
                ct,
                CommandTimeout).ConfigureAwait(false);
            if (ipRes.Success)
            {
                var ip = ipRes.StdOut.Trim();
                if (ip.Length > 0 && ip != "None")
                {
                    publicIp = ip;
                    break;
                }
            }

            // A cancellation here (shutdown) must NOT discard the known
            // instanceId — the VM exists and bills. Treat it as created with
            // whatever public IP we captured so far so the caller records the
            // resource id (quality audit F8).
            try
            {
                await Task.Delay(TimeSpan.FromSeconds(2), ct).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                return VmCreateResult.Created(instanceId, publicIp, request.Name);
            }
        }

        return VmCreateResult.Created(instanceId, publicIp, request.Name);
    }

    /// <summary>Rust <c>resolve_ami</c> marker → (owner, name filter).</summary>
    public static (string Owner, string NameFilter) AwsAmiFilter(string marker) =>
        (marker.StartsWith("aws:", StringComparison.Ordinal) ? marker["aws:".Length..] : marker) switch
        {
            "ubuntu-24.04-server" => ("099720109477", "ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-amd64-server-*"),
            "ubuntu-22.04-server" => ("099720109477", "ubuntu/images/hvm-ssd/ubuntu-jammy-22.04-amd64-server-*"),
            "debian-12-server" => ("136693071363", "debian-12-amd64-*"),
            "windows-2022-server" => ("801119661308", "Windows_Server-2022-English-Full-Base-*"),
            _ => ("099720109477", "ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-amd64-server-*"),
        };

    /// <summary>
    /// Pure argv builder for <c>aws ec2 run-instances</c> — the C# port of the
    /// Rust <c>AwsProvider::build_run_instances_args</c>.
    /// </summary>
    public static List<string> BuildAwsRunInstancesArgs(
        VmCreateRequest request, string amiId, string keyName, string sgId, string? userDataPath)
    {
        var args = new List<string>
        {
            "ec2", "run-instances",
            "--image-id", amiId,
            "--instance-type", request.VmSize,
            "--region", request.Region,
            "--key-name", keyName,
            "--security-group-ids", sgId,
            "--associate-public-ip-address",
            "--tag-specifications", $"ResourceType=instance,Tags=[{{Key=Name,Value={request.Name}}}]",
            "--query", "Instances[0]",
            "--output", "json",
        };
        if (userDataPath is not null)
        {
            args.Add("--user-data");
            args.Add($"file://{userDataPath}");
        }

        return args;
    }

    private async Task<(string? Value, string? Error)> EnsureAwsKeyPairAsync(
        IReadOnlyDictionary<string, string> env, string region, CancellationToken ct)
    {
        const string keyName = "alethedash-tester";

        var check = await RunAsync(
            CloudCli.AwsBin(),
            new List<string>
            {
                "ec2", "describe-key-pairs", "--key-names", keyName,
                "--region", region, "--output", "json",
            },
            env, ct, CommandTimeout).ConfigureAwait(false);
        if (check.Success)
        {
            return (keyName, null);
        }

        // Import the local public key if present. Home resolution survives a
        // systemd unit without $HOME (audit F3) — never a relative path.
        var home = CloudCli.HomeDirectory();
        var pubKeyPath = Path.Combine(home, ".ssh", "id_rsa.pub");
        if (File.Exists(pubKeyPath))
        {
            var import = await RunAsync(
                CloudCli.AwsBin(),
                new List<string>
                {
                    "ec2", "import-key-pair", "--key-name", keyName,
                    "--public-key-material", $"fileb://{pubKeyPath}",
                    "--region", region,
                },
                env, ct, CommandTimeout).ConfigureAwait(false);
            if (import.Success)
            {
                return (keyName, null);
            }

            logger.LogWarning(
                "AWS key import failed, will try create-key-pair: {Err}", import.Error ?? import.StdErr);
        }

        var create = await RunAsync(
            CloudCli.AwsBin(),
            new List<string>
            {
                "ec2", "create-key-pair", "--key-name", keyName,
                "--query", "KeyMaterial", "--region", region, "--output", "text",
            },
            env, ct, CommandTimeout).ConfigureAwait(false);
        if (!create.Success)
        {
            return (null, $"create-key-pair failed: {create.Error ?? create.StdErr}");
        }

        var pemPath = Path.Combine(home, ".ssh", $"{keyName}.pem");
        try
        {
            await File.WriteAllTextAsync(pemPath, create.StdOut, ct).ConfigureAwait(false);
            if (!OperatingSystem.IsWindows())
            {
                File.SetUnixFileMode(pemPath, UnixFileMode.UserRead | UnixFileMode.UserWrite);
            }
        }
        catch (Exception ex)
        {
            return (null, $"failed to persist {pemPath}: {ex.Message}");
        }

        return (keyName, null);
    }

    private async Task<(string? Value, string? Error)> EnsureAwsSecurityGroupAsync(
        IReadOnlyDictionary<string, string> env, string region, CancellationToken ct)
    {
        const string sgName = "alethedash-tester";

        var check = await RunAsync(
            CloudCli.AwsBin(),
            new List<string>
            {
                "ec2", "describe-security-groups", "--group-names", sgName,
                "--query", "SecurityGroups[0].GroupId",
                "--region", region, "--output", "text",
            },
            env, ct, CommandTimeout).ConfigureAwait(false);
        if (check.Success)
        {
            var existing = check.StdOut.Trim();
            if (existing.Length > 0 && existing != "None")
            {
                return (existing, null);
            }
        }

        var create = await RunAsync(
            CloudCli.AwsBin(),
            new List<string>
            {
                "ec2", "create-security-group", "--group-name", sgName,
                // Display-only description (brand = Networker). The SG *name* and
                // tags stay "alethedash-tester" — live deployments match on them.
                "--description", "Networker tester (SSH + diagnostic ports)",
                "--query", "GroupId", "--region", region, "--output", "text",
            },
            env, ct, CommandTimeout).ConfigureAwait(false);
        if (!create.Success)
        {
            return (null, $"create-security-group failed: {create.Error ?? create.StdErr}");
        }

        var sgId = create.StdOut.Trim();

        // Ingress: SSH (22), diagnostic TCP (8080/8443), UDP probes
        // (8443/9998/9999). Failures ignored like the Rust `let _ =`.
        foreach (var (proto, port) in new[]
                 {
                     ("tcp", "22"), ("tcp", "8080"), ("tcp", "8443"),
                     ("udp", "8443"), ("udp", "9998"), ("udp", "9999"),
                 })
        {
            _ = await RunAsync(
                CloudCli.AwsBin(),
                new List<string>
                {
                    "ec2", "authorize-security-group-ingress",
                    "--group-id", sgId,
                    "--protocol", proto,
                    "--port", port,
                    "--cidr", "0.0.0.0/0",
                    "--region", region,
                },
                env, ct, CommandTimeout).ConfigureAwait(false);
        }

        return (sgId, null);
    }

    // ── GCP create ───────────────────────────────────────────────────────────

    private async Task<VmCreateResult> CreateGcpVmAsync(
        VmCreateRequest request, ProviderCredentials? creds, CancellationToken ct)
    {
        // GcpProvider::from_config: json_key is required and must carry project_id.
        var jsonKey = ExtraValue(creds, "json_key");
        if (jsonKey is null)
        {
            return VmCreateResult.Fail("gcp config: missing json_key");
        }

        string? projectId;
        try
        {
            using var keyDoc = JsonDocument.Parse(jsonKey);
            projectId = keyDoc.RootElement.TryGetProperty("project_id", out var pid)
                        && pid.ValueKind == JsonValueKind.String
                ? pid.GetString()
                : null;
        }
        catch (JsonException)
        {
            return VmCreateResult.Fail("gcp config: json_key is not valid JSON");
        }

        if (projectId is null)
        {
            return VmCreateResult.Fail("gcp json_key: missing project_id");
        }

        var keyFile = Path.Combine(Path.GetTempPath(), $"gcp-key-{Guid.NewGuid():N}.json");
        // The GCP service-account key is a long-lived credential — 0600, never
        // world-readable in /tmp (quality audit F11).
        await SecretFile.WriteAsync(keyFile, jsonKey, ct).ConfigureAwait(false);

        var env = new Dictionary<string, string>
        {
            ["GOOGLE_APPLICATION_CREDENTIALS"] = keyFile,
            ["CLOUDSDK_CORE_PROJECT"] = projectId,
        };

        // GCP needs a zone, not just a region — first zone in the region.
        var zone = $"{request.Region}-a";

        // ssh-keys metadata from the dashboard host's local key, when present.
        // Home resolution survives a systemd unit without $HOME (audit F3).
        string? sshMetadataPath = null;
        var home = CloudCli.HomeDirectory();
        var pubKeyPath = Path.Combine(home, ".ssh", "id_rsa.pub");
        if (File.Exists(pubKeyPath))
        {
            var pubKey = (await File.ReadAllTextAsync(pubKeyPath, ct).ConfigureAwait(false)).Trim();
            sshMetadataPath = Path.Combine(Path.GetTempPath(), $"gcp-ssh-keys-{Guid.NewGuid():N}.txt");
            await File.WriteAllTextAsync(sshMetadataPath, $"{request.SshUser}:{pubKey}", ct).ConfigureAwait(false);
        }
        else
        {
            logger.LogWarning("No ~/.ssh/id_rsa.pub found — GCP SSH may not work");
        }

        string? startupScriptPath = null;
        if (request.BootstrapScript is { } script)
        {
            startupScriptPath = Path.GetTempFileName();
            // GCP startup script embeds the minted agent API key — 0600.
            await SecretFile.WriteAsync(startupScriptPath, script, ct).ConfigureAwait(false);
        }

        ProvisionResult res;
        try
        {
            var args = BuildGcpCreateArgs(request, zone, sshMetadataPath, startupScriptPath);
            res = await RunAsync(CloudCli.GcloudBin(), args, env, ct, CreateTimeout).ConfigureAwait(false);
        }
        finally
        {
            if (startupScriptPath is not null)
            {
                TryDeleteFile(startupScriptPath);
            }

            if (sshMetadataPath is not null)
            {
                TryDeleteFile(sshMetadataPath);
            }

            TryDeleteFile(keyFile);
        }

        if (!res.Success)
        {
            return VmCreateResult.Fail($"gcloud compute instances create failed: {res.Error ?? res.StdErr}");
        }

        try
        {
            using var doc = JsonDocument.Parse(res.StdOut);
            // GCP returns an array; take the first element (fall back to root).
            var inst = doc.RootElement.ValueKind == JsonValueKind.Array && doc.RootElement.GetArrayLength() > 0
                ? doc.RootElement[0]
                : doc.RootElement;

            string? resourceId =
                inst.TryGetProperty("selfLink", out var sl) && sl.ValueKind == JsonValueKind.String
                    ? sl.GetString()
                    : inst.TryGetProperty("id", out var id) && id.ValueKind == JsonValueKind.String
                        ? id.GetString()
                        : null;
            if (resourceId is null)
            {
                return VmCreateResult.Fail("missing instance id/selfLink");
            }

            var publicIp = string.Empty;
            if (inst.TryGetProperty("networkInterfaces", out var nics)
                && nics.ValueKind == JsonValueKind.Array && nics.GetArrayLength() > 0
                && nics[0].TryGetProperty("accessConfigs", out var acs)
                && acs.ValueKind == JsonValueKind.Array && acs.GetArrayLength() > 0
                && acs[0].TryGetProperty("natIP", out var nat)
                && nat.ValueKind == JsonValueKind.String)
            {
                publicIp = nat.GetString() ?? string.Empty;
            }

            return VmCreateResult.Created(resourceId, publicIp, request.Name);
        }
        catch (JsonException)
        {
            return VmCreateResult.Fail("gcloud create produced non-JSON output");
        }
    }

    /// <summary>
    /// Pure argv builder for <c>gcloud compute instances create</c> — the C#
    /// port of the Rust <c>GcpProvider::build_create_args</c>.
    /// </summary>
    public static List<string> BuildGcpCreateArgs(
        VmCreateRequest request, string zone, string? sshMetadataPath, string? startupScriptPath)
    {
        var imageProject = request.Image switch
        {
            var s when s.StartsWith("ubuntu", StringComparison.Ordinal) => "ubuntu-os-cloud",
            var s when s.StartsWith("debian", StringComparison.Ordinal) => "debian-cloud",
            var s when s.StartsWith("windows", StringComparison.Ordinal) => "windows-cloud",
            _ => "ubuntu-os-cloud",
        };

        var args = new List<string>
        {
            "compute", "instances", "create", request.Name,
            "--zone", zone,
            "--machine-type", request.VmSize,
            "--image-family", request.Image,
            "--image-project", imageProject,
            "--tags", "alethedash-tester",
            "--format", "json",
        };

        if (sshMetadataPath is not null)
        {
            args.Add("--metadata-from-file");
            args.Add($"ssh-keys={sshMetadataPath}");
        }

        if (startupScriptPath is not null)
        {
            args.Add("--metadata-from-file");
            args.Add($"startup-script={startupScriptPath}");
        }

        return args;
    }

    private static void TryDeleteFile(string path)
    {
        try
        {
            File.Delete(path);
        }
        catch
        {
            // Best-effort tempfile cleanup.
        }
    }

    private static void TryDeleteDir(string path)
    {
        try
        {
            Directory.Delete(path, recursive: true);
        }
        catch
        {
            // Best-effort temp-dir cleanup.
        }
    }

    private static bool LooksAlreadyGone(string stderr) =>
        stderr.Contains("ResourceNotFound", StringComparison.OrdinalIgnoreCase)
        || stderr.Contains("could not be found", StringComparison.OrdinalIgnoreCase)
        || stderr.Contains("InvalidInstanceID.NotFound", StringComparison.OrdinalIgnoreCase)
        || stderr.Contains("does not exist", StringComparison.OrdinalIgnoreCase)
        || stderr.Contains("was not found", StringComparison.OrdinalIgnoreCase)
        || stderr.Contains("404", StringComparison.OrdinalIgnoreCase);

    // ── Hardened process runner (ported from ProbeRunner) ────────────────────

    private async Task<ProvisionResult> RunAsync(
        string file,
        List<string> args,
        IReadOnlyDictionary<string, string>? env,
        CancellationToken cancellationToken,
        TimeSpan? timeout = null,
        bool sensitiveArgs = false)
    {
        var effectiveTimeout = timeout ?? CommandTimeout;
        var psi = new ProcessStartInfo
        {
            FileName = file,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true,
        };
        foreach (var a in args)
        {
            psi.ArgumentList.Add(a);
        }
        if (env is not null)
        {
            foreach (var (k, v) in env)
            {
                psi.Environment[k] = v;
            }
        }

        logger.LogInformation(
            "Provisioner spawning {File} {Args}",
            file,
            sensitiveArgs ? "(args redacted: contains credentials)" : string.Join(' ', args));

        using var timeoutCts = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeoutCts.CancelAfter(effectiveTimeout);
        var ct = timeoutCts.Token;

        using var process = new Process { StartInfo = psi };
        try
        {
            process.Start();
        }
        catch (Exception ex)
        {
            // The common CI path: the cloud CLI isn't installed. Still a soft
            // failure (the caller logs it and returns 202 with the DB transition
            // done), but the message now names the binary and its override env
            // var so the soft-fail is diagnosable, not silent (audit F12).
            var message = CloudCli.LaunchFailureMessage(file, ex.Message);
            logger.LogWarning(ex, "Failed to launch provisioner CLI '{File}': {Hint}", file, message);
            return ProvisionResult.SpawnError(message);
        }

        // Drain both streams concurrently, await AFTER exit (avoids the
        // pipe-buffer deadlock + the BeginOutputReadLine flush race).
        var stdoutTask = process.StandardOutput.ReadToEndAsync(ct);
        var stderrTask = process.StandardError.ReadToEndAsync(ct);

        string stdout, stderr;
        try
        {
            await process.WaitForExitAsync(ct).ConfigureAwait(false);
            stdout = (await stdoutTask.ConfigureAwait(false)).Trim();
            stderr = (await stderrTask.ConfigureAwait(false)).Trim();
        }
        catch (OperationCanceledException) when (timeoutCts.IsCancellationRequested
                                                 && !cancellationToken.IsCancellationRequested)
        {
            KillTree(process);
            return ProvisionResult.SpawnError(
                $"provisioner CLI '{file}' timed out after {effectiveTimeout.TotalSeconds:0}s and was killed");
        }
        catch (OperationCanceledException)
        {
            KillTree(process); // caller cancelled — don't leave the child running
            throw;
        }

        return process.ExitCode == 0
            ? ProvisionResult.Ok(process.ExitCode, stdout, stderr)
            : ProvisionResult.Failed(process.ExitCode, stdout, stderr);
    }

    private static void KillTree(Process process)
    {
        try
        {
            if (!process.HasExited)
            {
                process.Kill(entireProcessTree: true);
            }
        }
        catch
        {
            // Best-effort — may have exited between the check and the kill.
        }
    }
}
