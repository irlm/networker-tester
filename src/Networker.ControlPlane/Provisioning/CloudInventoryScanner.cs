using System.Text.Json;
using System.Text.Json.Serialization;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// One VM as the inventory endpoint puts it on the wire. Snake_case is the
/// established contract (Rust <c>api/inventory.rs</c> → the dashboard's
/// SettingsPage table), pinned with explicit
/// <see cref="JsonPropertyNameAttribute"/> rather than relying on a naming
/// policy.
/// </summary>
public sealed record CloudVm(
    [property: JsonPropertyName("provider")] string Provider,
    [property: JsonPropertyName("name")] string Name,
    [property: JsonPropertyName("region")] string Region,
    [property: JsonPropertyName("status")] string Status,
    [property: JsonPropertyName("public_ip")] string? PublicIp,
    [property: JsonPropertyName("fqdn")] string? Fqdn,
    [property: JsonPropertyName("vm_size")] string? VmSize,
    [property: JsonPropertyName("os")] string? Os,
    [property: JsonPropertyName("resource_group")] string? ResourceGroup,
    [property: JsonPropertyName("managed")] bool Managed);

/// <summary>
/// A cloud account to enumerate: the provider, the account's display name (for
/// error messages — never its id or its secrets), the DECRYPTED credential map
/// exactly as <c>cloud_account.credentials_enc</c> stores it, and the account's
/// default region.
/// </summary>
public sealed record InventoryAccount(
    string Provider,
    string Name,
    IReadOnlyDictionary<string, string> Credentials,
    string? RegionDefault);

/// <summary>
/// The result of one inventory sweep. <see cref="Scanned"/> is what we actually
/// queried and <see cref="Errors"/> is what went wrong — together they let the
/// UI say "no VMs found — scanned azure, aws" instead of a silent empty table.
/// </summary>
public sealed record InventoryScan(
    IReadOnlyList<CloudVm> Vms,
    IReadOnlyList<string> Errors,
    IReadOnlyList<string> Scanned);

/// <summary>
/// Read-only enumeration of the VMs a project's cloud accounts can see —
/// the engine behind <c>GET /api/projects/{id}/inventory</c> ("scan all
/// providers" on the Settings page).
///
/// <para><b>This class only ever LISTS.</b> Every command it builds is an
/// enumeration (<c>az vm list</c>, <c>aws ec2 describe-instances</c>,
/// <c>gcloud compute instances list</c>) plus, for Azure only, the
/// <c>az login</c> that authenticates them. It must never grow a create,
/// start, stop, or delete — that is <see cref="CliComputeProvisioner"/>'s job,
/// and this endpoint is reachable by any project MEMBER.</para>
///
/// <para><b>Credentials.</b> Each account is authenticated per invocation, the
/// way the rest of the control plane does it: Azure signs the service principal
/// in to an isolated <c>AZURE_CONFIG_DIR</c> (never the host's ambient
/// <c>az</c> session — same as <see cref="Background.OrphanReaperService"/>),
/// GCP goes through <see cref="CliComputeProvisioner.BuildGcloudEnv"/>'s
/// <c>CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE</c> pointing at a 0600 key file in
/// a 0700 throwaway <c>CLOUDSDK_CONFIG</c> (#827), and AWS passes its keys as
/// process environment variables. Every temp file/dir is deleted in a finally.
/// Secrets never reach a log line, an argv we log, or an <c>errors[]</c> entry
/// (<see cref="CleanMessage"/> redacts them defensively).</para>
///
/// <para><b>Honesty rules</b> (this endpoint was a stub that returned an empty
/// list, so the owner reasonably concluded the button was dead):
/// <list type="bullet">
///   <item>Accounts are scanned <b>in parallel</b>; one provider's failure never
///   removes another's results.</item>
///   <item>Anything that stops a configured account from being enumerated —
///   missing CLI, sign-in failure, non-zero exit, timeout, unparseable output —
///   becomes an <c>errors[]</c> line naming the provider and the account.</item>
///   <item>A provider with <b>no</b> account configured is NOT an error (nothing
///   failed); it is simply absent from <see cref="InventoryScan.Scanned"/>, and
///   the UI says so.</item>
///   <item>No VM data is ever synthesised.</item>
/// </list></para>
///
/// <para><b>Scope divergence from the Rust original:</b> the Rust scan filtered
/// to resource groups / instance names containing <c>networker-endpoint</c> /
/// <c>networker-tester</c>. Those names no longer exist (today's VMs are
/// <c>nwk-a-*</c>, <c>nwk-ep-*</c>, <c>tester-*</c>, <c>ab-*</c>), so that filter
/// would have hidden every VM we create on AWS and GCP — the same silent-empty
/// failure this change exists to remove. The scan therefore enumerates
/// everything the credential itself is scoped to (the Azure subscription, the
/// GCP key's project, the AWS regions listed below) and lets <c>managed</c> mark
/// which rows are ours. <see cref="MaxVmsPerAccount"/> caps a huge account, with
/// an honest note when it bites.</para>
/// </summary>
public sealed class CloudInventoryScanner(ILogger<CloudInventoryScanner> logger)
{
    /// <summary>Providers this scanner knows how to enumerate, in display order.</summary>
    public static readonly string[] KnownProviders = ["azure", "aws", "gcp"];

    /// <summary>Ceiling for a single list/sign-in CLI invocation. A cloud CLI
    /// that hangs on a network call is killed and reported as a timeout, never
    /// left to stall the HTTP request. Instance state (not a static) so tests
    /// can shorten it without mutating process-global state.</summary>
    internal TimeSpan CommandTimeout { get; set; } = TimeSpan.FromSeconds(45);

    /// <summary>Ceiling for one account's whole scan (AWS spends it across
    /// several regions). Exhausting it is an <c>errors[]</c> entry, not a stuck
    /// request.</summary>
    internal TimeSpan AccountBudget { get; set; } = TimeSpan.FromSeconds(90);

    /// <summary>Most VMs returned per account. A shared corporate subscription
    /// can hold thousands; the table is not virtualised and the point of the
    /// panel is our fleet. Truncation is reported.</summary>
    internal const int MaxVmsPerAccount = 500;

    /// <summary>Longest error text kept from a CLI's stderr.</summary>
    private const int MaxMessageLength = 300;

    /// <summary>
    /// Regions swept for an AWS account (EC2's describe-instances is
    /// per-region). The account's own <c>region_default</c> is always swept
    /// first; these are the Rust scan's defaults for everything else.
    /// </summary>
    internal static readonly string[] DefaultAwsRegions =
        ["us-east-1", "us-west-2", "eu-west-1", "eu-central-1", "ap-southeast-1"];

    /// <summary>
    /// Enumerate every account in parallel and fold the results into one
    /// response. Never throws — a cancelled request or an exhausted budget
    /// becomes an <c>errors[]</c> line so the caller always has something honest
    /// to render.
    /// </summary>
    public async Task<InventoryScan> ScanAsync(
        IReadOnlyList<InventoryAccount> accounts,
        IReadOnlyList<string> managedHosts,
        CancellationToken ct)
    {
        var results = await Task.WhenAll(
            accounts.Select(a => ScanAccountAsync(a, managedHosts, ct))).ConfigureAwait(false);

        var vms = new List<CloudVm>();
        var errors = new List<string>();
        var scanned = new List<string>();

        // Fold in KnownProviders order so the table and the "scanned …" line are
        // stable across runs regardless of task completion order.
        foreach (var provider in KnownProviders.Concat(
                     accounts.Select(a => a.Provider).Where(p => !KnownProviders.Contains(p)).Distinct()))
        {
            var forProvider = results.Where(r => r.Provider == provider).ToList();
            if (forProvider.Count == 0)
            {
                continue;
            }
            scanned.Add(provider);
            foreach (var r in forProvider)
            {
                vms.AddRange(r.Vms);
                errors.AddRange(r.Errors);
            }
        }

        return new InventoryScan(vms, errors, scanned);
    }

    /// <summary>One account's outcome — kept per account so a second account of
    /// the same provider still contributes its VMs when the first one fails.</summary>
    private sealed record AccountScan(string Provider, List<CloudVm> Vms, List<string> Errors);

    private async Task<AccountScan> ScanAccountAsync(
        InventoryAccount account, IReadOnlyList<string> managedHosts, CancellationToken ct)
    {
        var scan = new AccountScan(account.Provider, [], []);
        using var budget = CancellationTokenSource.CreateLinkedTokenSource(ct);
        budget.CancelAfter(AccountBudget);

        try
        {
            switch (account.Provider)
            {
                case "azure":
                    await ScanAzureAsync(account, managedHosts, scan, budget.Token).ConfigureAwait(false);
                    break;
                case "aws":
                    await ScanAwsAsync(account, managedHosts, scan, budget.Token).ConfigureAwait(false);
                    break;
                case "gcp":
                    await ScanGcpAsync(account, managedHosts, scan, budget.Token).ConfigureAwait(false);
                    break;
                default:
                    scan.Errors.Add(Describe(account, $"unsupported provider '{account.Provider}' — nothing was scanned"));
                    break;
            }
        }
        catch (OperationCanceledException)
        {
            // Total by design: the budget firing (or the caller going away) is a
            // reportable outcome, not an exception the endpoint has to handle.
            scan.Errors.Add(Describe(account, ct.IsCancellationRequested
                ? "the scan was cancelled before it finished"
                : $"scan timed out after {AccountBudget.TotalSeconds:0}s"));
        }
        catch (Exception ex)
        {
            logger.LogWarning(ex, "Inventory: {Provider} account scan threw", account.Provider);
            scan.Errors.Add(Describe(account, CleanMessage(ex.Message, Secrets(account))));
        }

        Truncate(account, scan);
        return scan;
    }

    private static void Truncate(InventoryAccount account, AccountScan scan)
    {
        if (scan.Vms.Count <= MaxVmsPerAccount)
        {
            return;
        }
        scan.Vms.RemoveRange(MaxVmsPerAccount, scan.Vms.Count - MaxVmsPerAccount);
        scan.Errors.Add(Describe(account, $"listing truncated to the first {MaxVmsPerAccount} VMs"));
    }

    // ── Azure ────────────────────────────────────────────────────────────────

    /// <summary>JMESPath projection for <c>az vm list --show-details</c> — the
    /// fields the inventory table renders, and nothing else (the raw record is
    /// ~40 fields per VM). Kept identical to the Rust scan's projection minus
    /// its stale resource-group filter.</summary>
    internal const string AzureVmQuery =
        "[].{name:name, rg:resourceGroup, location:location, powerState:powerState, "
        + "publicIps:publicIps, fqdns:fqdns, size:hardwareProfile.vmSize, os:storageProfile.osDisk.osType}";

    private async Task ScanAzureAsync(
        InventoryAccount account, IReadOnlyList<string> managedHosts, AccountScan scan, CancellationToken ct)
    {
        var creds = account.Credentials;
        var subscription = creds.GetValueOrDefault("subscription_id", string.Empty);
        var clientId = creds.GetValueOrDefault("client_id", string.Empty);
        var clientSecret = creds.GetValueOrDefault("client_secret", string.Empty);
        var tenantId = creds.GetValueOrDefault("tenant_id", string.Empty);

        if (subscription.Length == 0)
        {
            scan.Errors.Add(Describe(account, "the stored credentials carry no subscription_id — nothing to list"));
            return;
        }
        if (clientId.Length == 0 || clientSecret.Length == 0 || tenantId.Length == 0)
        {
            scan.Errors.Add(Describe(account,
                "the stored credentials carry no service principal (client_id / client_secret / tenant_id) — cannot sign in to list VMs"));
            return;
        }

        // Isolated config dir: signing this account in must never touch the
        // host's shared az session (0700 — the token cache is as sensitive as
        // the secret that minted it).
        var configDir = Path.Combine(Path.GetTempPath(), $"inventory-az-{Guid.NewGuid():N}");
        SecretFile.CreateDir0700(configDir);
        try
        {
            var env = new Dictionary<string, string>(StringComparer.Ordinal)
            {
                ["AZURE_CONFIG_DIR"] = configDir,
                ["PYTHONWARNINGS"] = "ignore",
            };

            // Args carry the client secret — never logged (see RunAsync).
            var login = await RunAsync(CloudCli.AzBin(),
                [
                    "login", "--service-principal", "-u", clientId, "-p", clientSecret,
                    "--tenant", tenantId, "--output", "none",
                ],
                env, ct, sensitiveArgs: true).ConfigureAwait(false);
            if (Failure(account, login, "az sign-in") is { } loginError)
            {
                scan.Errors.Add(loginError);
                return;
            }

            var list = await RunAsync(CloudCli.AzBin(),
                ["vm", "list", "--show-details", "--subscription", subscription,
                    "--query", AzureVmQuery, "--output", "json"],
                env, ct).ConfigureAwait(false);
            if (Failure(account, list, "az vm list") is { } listError)
            {
                scan.Errors.Add(listError);
                return;
            }

            if (ParseAzureVms(list.StdOut, managedHosts) is { } vms)
            {
                scan.Vms.AddRange(vms);
            }
            else
            {
                scan.Errors.Add(Describe(account, "az vm list returned output that is not the expected JSON array"));
            }
        }
        finally
        {
            TryDeleteTree(configDir);
        }
    }

    /// <summary>
    /// Project <c>az vm list --show-details</c> output (already narrowed by
    /// <see cref="AzureVmQuery"/>) into wire rows. Null when the document is not
    /// a JSON array — the caller reports that rather than showing an empty table.
    /// </summary>
    internal static List<CloudVm>? ParseAzureVms(string json, IReadOnlyList<string> managedHosts)
    {
        if (Root(json) is not { ValueKind: JsonValueKind.Array } root)
        {
            return null;
        }

        var vms = new List<CloudVm>();
        foreach (var vm in root.EnumerateArray())
        {
            if (vm.ValueKind != JsonValueKind.Object)
            {
                continue;
            }
            var fqdn = NonEmpty(Str(vm, "fqdns"));
            var publicIp = NonEmpty(Str(vm, "publicIps"));
            vms.Add(new CloudVm(
                Provider: "azure",
                Name: Str(vm, "name") ?? string.Empty,
                Region: Str(vm, "location") ?? string.Empty,
                // "VM running" → "running" (the Rust normalisation the table's
                // status colours key on).
                Status: (Str(vm, "powerState") ?? "unknown").Replace("VM ", "", StringComparison.Ordinal).ToLowerInvariant(),
                PublicIp: publicIp,
                Fqdn: fqdn,
                VmSize: Str(vm, "size"),
                Os: Str(vm, "os")?.ToLowerInvariant(),
                ResourceGroup: Str(vm, "rg"),
                Managed: IsManaged(fqdn, publicIp, managedHosts)));
        }
        return vms;
    }

    // ── AWS ──────────────────────────────────────────────────────────────────

    /// <summary>JMESPath projection for <c>aws ec2 describe-instances</c>.</summary>
    internal const string AwsInstanceQuery =
        "Reservations[].Instances[].{name:Tags[?Key=='Name']|[0].Value, id:InstanceId, state:State.Name, "
        + "ip:PublicIpAddress, dns:PublicDnsName, type:InstanceType, az:Placement.AvailabilityZone, platform:Platform}";

    /// <summary>The regions swept for an account: its own default first (so a
    /// single-region user's VMs show up even if that region is exotic), then the
    /// common ones, de-duplicated and order-stable.</summary>
    internal static List<string> AwsRegionsFor(string? regionDefault)
    {
        var regions = new List<string>();
        if (!string.IsNullOrWhiteSpace(regionDefault))
        {
            regions.Add(regionDefault.Trim());
        }
        foreach (var r in DefaultAwsRegions)
        {
            if (!regions.Contains(r, StringComparer.OrdinalIgnoreCase))
            {
                regions.Add(r);
            }
        }
        return regions;
    }

    private async Task ScanAwsAsync(
        InventoryAccount account, IReadOnlyList<string> managedHosts, AccountScan scan, CancellationToken ct)
    {
        var creds = account.Credentials;
        var accessKey = creds.GetValueOrDefault("access_key_id", string.Empty);
        var secretKey = creds.GetValueOrDefault("secret_access_key", string.Empty);
        if (accessKey.Length == 0 || secretKey.Length == 0)
        {
            scan.Errors.Add(Describe(account,
                "the stored credentials carry no access_key_id / secret_access_key — nothing to list"));
            return;
        }

        var env = new Dictionary<string, string>(StringComparer.Ordinal)
        {
            ["AWS_ACCESS_KEY_ID"] = accessKey,
            ["AWS_SECRET_ACCESS_KEY"] = secretKey,
            // An interactive pager on a redirected stdout is a hang, not output.
            ["AWS_PAGER"] = string.Empty,
        };
        if (creds.GetValueOrDefault("session_token", string.Empty) is { Length: > 0 } sessionToken)
        {
            env["AWS_SESSION_TOKEN"] = sessionToken;
        }

        var regions = AwsRegionsFor(account.RegionDefault);
        // Regions are independent EC2 endpoints — sweep them concurrently so a
        // slow one costs latency, not the whole account's budget.
        var perRegion = await Task.WhenAll(regions.Select(async region =>
        {
            var res = await RunAsync(CloudCli.AwsBin(),
                ["ec2", "describe-instances", "--region", region,
                    "--query", AwsInstanceQuery, "--output", "json"],
                env, ct).ConfigureAwait(false);

            if (Failure(account, res, "aws ec2 describe-instances") is { } error)
            {
                return (Region: region, Vms: (List<CloudVm>?)null, Error: error);
            }
            var vms = ParseAwsInstances(res.StdOut, region, managedHosts);
            return (Region: region, Vms: vms,
                Error: vms is null
                    ? Describe(account, "aws ec2 describe-instances returned output that is not the expected JSON array")
                    : null);
        })).ConfigureAwait(false);

        foreach (var (_, vms, _) in perRegion.Where(r => r.Vms is not null))
        {
            scan.Vms.AddRange(vms!);
        }

        scan.Errors.AddRange(SummarizeRegionFailures(
            perRegion.Where(r => r.Error is not null).Select(r => (r.Region, Message: r.Error!)).ToList(),
            regions.Count));
    }

    /// <summary>
    /// Collapse per-region failures into what a human needs. A bad key fails
    /// every region with the same message, and five copies of it is noise, not
    /// signal — so identical messages fold into one line that names the regions
    /// they hit (or says "all of them").
    /// </summary>
    internal static List<string> SummarizeRegionFailures(
        IReadOnlyList<(string Region, string Message)> failures, int regionCount)
    {
        var lines = new List<string>();
        foreach (var group in failures.GroupBy(f => f.Message, StringComparer.Ordinal))
        {
            var regions = group.Select(g => g.Region).ToList();
            lines.Add(regions.Count == regionCount && regionCount > 1
                ? $"{group.Key} — in all {regionCount} scanned regions"
                : $"{group.Key} — in {string.Join(", ", regions)}");
        }
        return lines;
    }

    /// <summary>
    /// Project <c>aws ec2 describe-instances</c> output (narrowed by
    /// <see cref="AwsInstanceQuery"/>) into wire rows. Null when the document is
    /// not a JSON array.
    /// </summary>
    internal static List<CloudVm>? ParseAwsInstances(
        string json, string queriedRegion, IReadOnlyList<string> managedHosts)
    {
        if (Root(json) is not { ValueKind: JsonValueKind.Array } root)
        {
            return null;
        }

        var vms = new List<CloudVm>();
        foreach (var inst in root.EnumerateArray())
        {
            if (inst.ValueKind != JsonValueKind.Object)
            {
                continue;
            }
            var fqdn = NonEmpty(Str(inst, "dns"));
            var publicIp = NonEmpty(Str(inst, "ip"));
            var id = Str(inst, "id");
            vms.Add(new CloudVm(
                Provider: "aws",
                // Untagged instances still deserve a row — fall back to the id.
                Name: NonEmpty(Str(inst, "name")) ?? id ?? string.Empty,
                Region: RegionFromAvailabilityZone(Str(inst, "az")) ?? queriedRegion,
                Status: Str(inst, "state") ?? "unknown",
                PublicIp: publicIp,
                Fqdn: fqdn,
                VmSize: Str(inst, "type"),
                // EC2 reports Platform only for Windows; everything else is null.
                Os: Str(inst, "platform")?.ToLowerInvariant() ?? "linux",
                ResourceGroup: null,
                Managed: IsManaged(fqdn, publicIp, managedHosts)));
        }
        return vms;
    }

    /// <summary>"us-east-1a" → "us-east-1" (drop the AZ letter suffix). Null for
    /// a missing/empty AZ so the caller can fall back to the queried region.</summary>
    internal static string? RegionFromAvailabilityZone(string? az)
    {
        if (string.IsNullOrWhiteSpace(az))
        {
            return null;
        }
        var trimmed = az.TrimEnd(
            'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm',
            'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z');
        return trimmed.Length > 0 ? trimmed : az;
    }

    // ── GCP ──────────────────────────────────────────────────────────────────

    /// <summary>gcloud output projection: the fields the table renders, in the
    /// nested shape <see cref="ParseGcpInstances"/> reads.</summary>
    internal const string GcpInstanceFormat =
        "json(name,zone,status,networkInterfaces[0].accessConfigs[0].natIP,machineType)";

    private async Task ScanGcpAsync(
        InventoryAccount account, IReadOnlyList<string> managedHosts, AccountScan scan, CancellationToken ct)
    {
        var jsonKey = account.Credentials.GetValueOrDefault("json_key", string.Empty);
        if (jsonKey.Length == 0)
        {
            scan.Errors.Add(Describe(account, "the stored credentials carry no json_key — nothing to list"));
            return;
        }
        var projectId = CliComputeProvisioner.ParseGcpProjectId(jsonKey);
        if (string.IsNullOrEmpty(projectId))
        {
            scan.Errors.Add(Describe(account,
                "the stored service-account key carries no project_id — cannot tell gcloud which project to list"));
            return;
        }

        // One 0700 dir holds the 0600 key AND is the isolated gcloud config root
        // (#827/#833): one recursive delete cleans up both.
        var configDir = Path.Combine(Path.GetTempPath(), $"inventory-gcp-{Guid.NewGuid():N}");
        SecretFile.CreateDir0700(configDir);
        try
        {
            var keyFile = Path.Combine(configDir, "key.json");
            await SecretFile.WriteAsync(keyFile, jsonKey, ct).ConfigureAwait(false);

            var env = CliComputeProvisioner.BuildGcloudEnv(keyFile, projectId);
            env[GcpInstallerCredentials.ConfigDirVar] = configDir;
            // A prompt on a redirected stdin is a hang, not a question.
            env["CLOUDSDK_CORE_DISABLE_PROMPTS"] = "1";

            var list = await RunAsync(CloudCli.GcloudBin(),
                ["compute", "instances", "list", "--format", GcpInstanceFormat], env, ct).ConfigureAwait(false);
            if (Failure(account, list, "gcloud compute instances list") is { } error)
            {
                scan.Errors.Add(error);
                return;
            }

            if (ParseGcpInstances(list.StdOut, managedHosts) is { } vms)
            {
                scan.Vms.AddRange(vms);
            }
            else
            {
                scan.Errors.Add(Describe(account,
                    "gcloud compute instances list returned output that is not the expected JSON array"));
            }
        }
        finally
        {
            TryDeleteTree(configDir);
        }
    }

    /// <summary>
    /// Project <c>gcloud compute instances list</c> output (narrowed by
    /// <see cref="GcpInstanceFormat"/>) into wire rows. Null when the document is
    /// not a JSON array.
    /// </summary>
    internal static List<CloudVm>? ParseGcpInstances(string json, IReadOnlyList<string> managedHosts)
    {
        if (Root(json) is not { ValueKind: JsonValueKind.Array } root)
        {
            return null;
        }

        var vms = new List<CloudVm>();
        foreach (var inst in root.EnumerateArray())
        {
            if (inst.ValueKind != JsonValueKind.Object)
            {
                continue;
            }
            var publicIp = NonEmpty(FirstNatIp(inst));
            vms.Add(new CloudVm(
                Provider: "gcp",
                Name: Str(inst, "name") ?? string.Empty,
                // zone and machineType come back as full selfLink URLs.
                Region: LastSegment(Str(inst, "zone")) ?? string.Empty,
                Status: (Str(inst, "status") ?? "unknown").ToLowerInvariant(),
                PublicIp: publicIp,
                // GCE has no per-instance public DNS name.
                Fqdn: null,
                VmSize: LastSegment(Str(inst, "machineType")),
                Os: "linux",
                ResourceGroup: null,
                Managed: IsManaged(null, publicIp, managedHosts)));
        }
        return vms;
    }

    /// <summary>The first NIC's first external (NAT) IP, or null for an
    /// instance with no public address.</summary>
    private static string? FirstNatIp(JsonElement instance)
    {
        if (!instance.TryGetProperty("networkInterfaces", out var nics) || nics.ValueKind != JsonValueKind.Array)
        {
            return null;
        }
        foreach (var nic in nics.EnumerateArray())
        {
            if (nic.ValueKind != JsonValueKind.Object
                || !nic.TryGetProperty("accessConfigs", out var acs)
                || acs.ValueKind != JsonValueKind.Array)
            {
                continue;
            }
            foreach (var ac in acs.EnumerateArray())
            {
                if (ac.ValueKind == JsonValueKind.Object && Str(ac, "natIP") is { Length: > 0 } ip)
                {
                    return ip;
                }
            }
        }
        return null;
    }

    // ── Shared helpers ───────────────────────────────────────────────────────

    /// <summary>
    /// True when the VM's fqdn or public_ip matches (exact or substring) any
    /// managed host — i.e. the VM is one this control plane deployed. Mirrors
    /// the Rust <c>is_managed</c>; <c>InventoryEndpoints.IsManaged</c> forwards
    /// here so the endpoint's long-standing unit tests still pin it.
    /// </summary>
    public static bool IsManaged(string? fqdn, string? publicIp, IReadOnlyList<string> managedHosts)
    {
        if (fqdn is { } dns && managedHosts.Any(h => h == dns || h.Contains(dns, StringComparison.Ordinal)))
        {
            return true;
        }
        if (publicIp is { } ip && managedHosts.Any(h => h == ip || h.Contains(ip, StringComparison.Ordinal)))
        {
            return true;
        }
        return false;
    }

    /// <summary>Run one CLI call for an account through the shared hardened
    /// runner. Argv is logged only when it cannot contain a secret.</summary>
    private async Task<CloudCliResult> RunAsync(
        string file,
        IReadOnlyList<string> args,
        IReadOnlyDictionary<string, string> env,
        CancellationToken ct,
        bool sensitiveArgs = false)
    {
        logger.LogDebug(
            "Inventory scan spawning {File} {Args}",
            file,
            sensitiveArgs ? "(args redacted: contains credentials)" : string.Join(' ', args));
        return await CloudCli.RunAsync(file, args, env, CommandTimeout, ct).ConfigureAwait(false);
    }

    /// <summary>
    /// The <c>errors[]</c> line for a CLI call that did not succeed, or null
    /// when it did. Distinguishes the three ways a scan can fail so the reader
    /// knows whether to install a CLI, fix a credential, or retry.
    /// </summary>
    private string? Failure(InventoryAccount account, CloudCliResult res, string what)
    {
        if (res.Success)
        {
            return null;
        }
        if (!res.Spawned)
        {
            return Describe(account, res.LaunchError ?? $"{what}: the CLI could not be launched");
        }
        if (res.TimedOut)
        {
            return Describe(account, $"{what} timed out after {CommandTimeout.TotalSeconds:0}s and was killed");
        }

        var stderr = res.StdErr.Length > 0 ? res.StdErr : res.StdOut;
        var cleaned = account.Provider switch
        {
            // Reuse the validator's vendor-specific de-boilerplating so the
            // useful part of the message leads (the UI truncates from the left).
            "aws" => ProviderCredentialValidator.CleanAwsError(stderr),
            "gcp" => ProviderCredentialValidator.CleanGcloudError(stderr),
            _ => stderr,
        };
        var message = CleanMessage(cleaned, Secrets(account));
        return Describe(account, message.Length > 0
            ? $"{what} failed (exit {res.ExitCode}): {message}"
            : $"{what} failed (exit {res.ExitCode}) with no diagnostic output");
    }

    /// <summary>Prefix every error with the provider and the account NAME (never
    /// its id, never a credential) so a project with several accounts can tell
    /// which one needs attention.</summary>
    private static string Describe(InventoryAccount account, string message) =>
        $"{account.Provider} account '{account.Name}': {message}";

    /// <summary>The credential values that must never appear in an error line.</summary>
    private static IEnumerable<string> Secrets(InventoryAccount account) =>
        new[] { "client_secret", "secret_access_key", "session_token", "json_key", "private_key" }
            .Select(k => account.Credentials.GetValueOrDefault(k, string.Empty))
            .Where(v => v.Length >= 8);

    /// <summary>
    /// Make a CLI's stderr fit one line of UI: collapse whitespace, redact any
    /// credential value that leaked into it (defence in depth — a vendor CLI
    /// echoing a secret must not put it in an API response), and cap the length.
    /// </summary>
    internal static string CleanMessage(string raw, IEnumerable<string> secrets)
    {
        var text = string.Join(' ', (raw ?? string.Empty)
            .Split(['\r', '\n', '\t'], StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries));
        foreach (var secret in secrets)
        {
            if (secret.Length >= 8)
            {
                text = text.Replace(secret, "***", StringComparison.Ordinal);
            }
        }
        return text.Length > MaxMessageLength ? text[..MaxMessageLength] + "…" : text;
    }

    private static JsonElement? Root(string json)
    {
        if (string.IsNullOrWhiteSpace(json))
        {
            return null;
        }
        try
        {
            using var doc = JsonDocument.Parse(json);
            return doc.RootElement.Clone();
        }
        catch (JsonException)
        {
            return null;
        }
    }

    private static string? Str(JsonElement obj, string property) =>
        obj.TryGetProperty(property, out var el) && el.ValueKind == JsonValueKind.String ? el.GetString() : null;

    private static string? NonEmpty(string? value) => string.IsNullOrEmpty(value) ? null : value;

    /// <summary>Last path segment of a GCE selfLink URL
    /// (<c>…/zones/us-east1-b</c> → <c>us-east1-b</c>).</summary>
    private static string? LastSegment(string? url) =>
        string.IsNullOrEmpty(url) ? null : url[(url.LastIndexOf('/') + 1)..];

    private static void TryDeleteTree(string dir)
    {
        try
        {
            Directory.Delete(dir, recursive: true);
        }
        catch
        {
            // Best-effort temp cleanup: the dir is 0700 and holds nothing once
            // the CLI has exited, but a leftover must never fail a read-only scan.
        }
    }
}
