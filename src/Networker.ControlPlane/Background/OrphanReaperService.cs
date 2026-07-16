using System.Diagnostics;
using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.Data;
using Networker.Security;

namespace Networker.ControlPlane.Background;

/// <summary>
/// Cloud orphan reaper — the C# port of the Rust dashboard's
/// <c>cloud_orphan_reaper</c>
/// (<c>crates/networker-dashboard/src/services/cloud_orphan_reaper.rs</c>).
///
/// <para><b>Why:</b> failed tester / benchmark VM creations leave behind NICs,
/// public IPs, disks, and NSGs that reference each other but have <i>no</i> row in
/// our DB. Over time these pile up and hit cloud quotas (Azure defaults to ~10
/// public IPs / subscription / region). This service lists such resources, keeps
/// only those that (a) are <b>not</b> referenced by any DB row and (b) match a
/// conservative <b>owned-name allow-list</b> (<c>tester-*</c>, <c>ab-*</c>,
/// <c>nwk-ep-*</c>), then deletes them in dependency-safe order:
/// <c>VM → NIC → Disk → IP → NSG</c>. Each delete is best-effort — one failure
/// never stops the rest (soft-fail per resource).</para>
///
/// <para><b>Scope resolution (prod-critical):</b> the reaper resolves each Azure
/// (subscription, resource-group) scope from <b>both</b> active
/// <c>cloud_accounts</c> <i>and</i> <c>cloud_connections</c>, then de-dupes by
/// scope. Prod provisions testers via <c>cloud_accounts</c> (encrypted SP creds),
/// not <c>cloud_connections</c> — an earlier version keyed only on
/// <c>cloud_connections</c> and therefore <b>never ran</b> on prod, letting
/// orphans accumulate forever. For accounts we decrypt the credentials with the
/// <see cref="CredentialCipher"/> exactly as <c>TesterWriteEndpoints
/// .ResolveProviderCredentialsAsync</c> / <c>TesterPrecheckEndpoints</c> do, read
/// the <c>subscription_id</c>, resolve the resource-group the same way the
/// provisioner does (the account's <c>resource_group</c> claim, else the
/// <c>networker-testers</c> default — NOT the <c>DASHBOARD_AZURE_RG</c> env, which
/// only governs the legacy no-account fallback), and log in with the account's
/// service-principal creds in an isolated <c>AZURE_CONFIG_DIR</c> before any
/// list/delete — never relying on the control-plane host's ambient <c>az</c>
/// identity, exactly as <c>CliComputeProvisioner</c> does for create/delete.</para>
///
/// <para><b>Tick:</b> slower than the tester loops (~10min) — quota pressure
/// builds slowly and each pass fans out several cloud <c>list</c> calls.</para>
///
/// <para><b>CI-safe / no-op behaviour:</b> listing cloud resources needs cloud
/// access that CI and credential-less hosts don't have. If the CLI binary is
/// missing, an SP login fails, or a <c>list</c> call exits non-zero, the scope is
/// skipped (best-effort). AWS and GCP listing are stubs (return empty) exactly as
/// in the Rust source — only Azure has a real implementation.</para>
///
/// <para><b>Divergence from Rust — NSG collection:</b> <c>az vm create</c> leaves
/// a per-VM network security group named <c>&lt;vmname&gt;NSG</c>. The Rust reaper
/// never collected NSGs (they leaked on every create+delete). This C# port
/// additionally lists and reaps orphaned NSGs (after NICs, since an NSG can only
/// be deleted once no NIC references it). Intentional improvement — noted in the
/// CHANGELOG.</para>
///
/// <para><b>Scope discipline:</b> identical to <see cref="AutoShutdownService"/>
/// / <see cref="ReaperService"/> — <c>NetworkerDbContext</c> is resolved from a
/// fresh DI scope each tick; the hosted service is a singleton.</para>
/// </summary>
public sealed class OrphanReaperService : BackgroundService
{
    /// <summary>Sweep cadence. Slower than the tester loops — quota pressure
    /// builds slowly and each pass fans out several cloud list calls.</summary>
    private static readonly TimeSpan TickInterval = TimeSpan.FromMinutes(10);

    /// <summary>Hard ceiling for a single cloud CLI invocation (list or delete).
    /// A wedged CLI is killed rather than stalling the whole sweep.</summary>
    private static readonly TimeSpan CommandTimeout = TimeSpan.FromMinutes(3);

    /// <summary>Default Azure resource group when a cloud_account doesn't carry a
    /// <c>resource_group</c> claim. Mirrors the provisioner default in
    /// <c>TesterWriteEndpoints.ResolveProviderCredentialsAsync</c> — keep the two
    /// in sync so the reaper scans the RG where testers actually get created.
    /// </summary>
    internal const string DefaultAzureResourceGroup = "networker-testers";

    /// <summary>
    /// Allow-list of name prefixes this reaper is willing to touch. Anything
    /// else is left alone regardless of whether its resource id is in the
    /// known-set — defence-in-depth against destroying other tenants' resources
    /// in a shared subscription / resource group. Mirrors the Rust
    /// <c>OWNED_NAME_PREFIXES</c>.
    /// </summary>
    internal static readonly string[] OwnedNamePrefixes = ["tester-", "ab-", "nwk-ep-"];

    /// <summary>Delete order: a VM delete releases its NIC lease; deleting the
    /// NIC releases the IP and frees any NSG that referenced the NIC; the disk is
    /// safe to delete at any point. Rust used
    /// <c>["vm", "nic", "disk", "public_ip"]</c>; we append <c>"nsg"</c> (see the
    /// class-level NSG divergence note) — it must come AFTER <c>nic</c> because an
    /// NSG can't be deleted while a NIC still references it.</summary>
    internal static readonly string[] DeleteOrder = ["vm", "nic", "disk", "public_ip", "nsg"];

    private readonly IServiceScopeFactory _scopeFactory;
    private readonly ILogger<OrphanReaperService> _logger;
    private readonly PgAdvisoryLeaderLock? _leader;
    private readonly TickMonitor _monitor;

    public OrphanReaperService(
        IServiceScopeFactory scopeFactory,
        ILogger<OrphanReaperService> logger,
        PgAdvisoryLeaderLock? leaderLock = null,
        TickMonitor? tickMonitor = null)
    {
        _scopeFactory = scopeFactory;
        _logger = logger;
        // M6 ops infra (AddOpsInfrastructure); optional for bare test hosts.
        _leader = leaderLock;
        _monitor = tickMonitor ?? new TickMonitor();
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        _logger.LogInformation(
            "Cloud orphan-reaper background service started (tick every {Minutes}min)",
            TickInterval.TotalMinutes);
        _monitor.ReportStarted(OpsServiceNames.OrphanReaper);

        using var timer = new PeriodicTimer(TickInterval);
        while (await timer.WaitForNextTickAsync(stoppingToken).ConfigureAwait(false))
        {
            try
            {
                var ranAsLeader = await _leader
                    .TryRunGuardedAsync(LeaderLockKeys.OrphanReaper, SweepAsync, stoppingToken)
                    .ConfigureAwait(false);
                if (!ranAsLeader)
                {
                    _logger.LogDebug("Cloud orphan-reaper sweep skipped — another replica holds the leader lock");
                }
            }
            catch (OperationCanceledException) when (stoppingToken.IsCancellationRequested)
            {
                break;
            }
            catch (Exception ex)
            {
                // Never let one bad sweep kill the loop.
                _monitor.ReportError(OpsServiceNames.OrphanReaper, ex);
                _logger.LogError(ex, "Cloud orphan-reaper sweep failed");
            }
        }
    }

    private async Task SweepAsync(CancellationToken ct)
    {
        using var scope = _scopeFactory.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
        // Optional: bare test hosts may not register the cipher. Without it we
        // simply can't decrypt account creds, so account scopes are skipped.
        var cipher = scope.ServiceProvider.GetService<CredentialCipher>();

        // Known-set: every resource id we've ever recorded on a tester. Anything
        // not in here (and matching an owned prefix) is a candidate orphan. We
        // intentionally over-collect (vm_resource_id) — the allow-list prefix
        // guard is the real safety net.
        var knownIds = await db.ProjectTesters
            .Where(t => t.VmResourceId != null)
            .Select(t => t.VmResourceId!)
            .ToListAsync(ct)
            .ConfigureAwait(false);
        var knownSet = new HashSet<string>(knownIds, StringComparer.OrdinalIgnoreCase);

        // Known VM-NAME prefixes: a tester's NIC / disk / public-IP / NSG are
        // named "<vm_name>VMNic" / "<vm_name>_OsDisk…" / "<vm_name>PublicIP" /
        // "<vm_name>NSG" — DISTINCT resource ids that are NOT in knownSet (only
        // the VM's own id is). Without this guard the reaper mis-identifies every
        // LIVE tester's child resources as orphans and tries to delete them each
        // tick (harmless only because Azure blocks deleting attached resources —
        // noisy + fragile). A resource is retained if its name starts with the
        // vm_name of any tester that still has a DB row (row exists ⇒ not deleted,
        // incl. stopped/deallocated testers that will be restarted).
        var knownVmNames = await db.ProjectTesters
            .Where(t => t.VmName != null)
            .Select(t => t.VmName!)
            .ToListAsync(ct)
            .ConfigureAwait(false);
        var knownNamePrefixes = knownVmNames
            .Where(n => n.Length > 0)
            .ToList();

        // Resolve every distinct Azure (subscription, resource-group) scope from
        // BOTH active cloud_accounts and cloud_connections, de-duped. Prod uses
        // cloud_accounts (encrypted SP creds) — keying only on cloud_connections
        // is why the reaper never ran on prod.
        var scopes = await ResolveAzureScopesAsync(db, cipher, ct).ConfigureAwait(false);

        if (scopes.Count == 0)
        {
            // No configured Azure scope → nothing to list. AWS/GCP are stubs
            // (Rust returns empty Vec), so the sweep is a no-op.
            _logger.LogDebug(
                "Orphan-reaper: no Azure cloud_account or cloud_connection configured; nothing to scan");
            _monitor.ReportTick(OpsServiceNames.OrphanReaper, 0, "no azure cloud_account or connection configured");
            return;
        }

        var totalDeleted = 0;
        var totalFailed = 0;
        var totalWouldReap = 0;

        foreach (var scopeInfo in scopes)
        {
            var (deleted, failed, wouldReap) =
                await ReapAzureScopeAsync(scopeInfo, knownSet, knownNamePrefixes, ct).ConfigureAwait(false);
            totalDeleted += deleted;
            totalFailed += failed;
            totalWouldReap += wouldReap;
        }

        if (totalWouldReap > 0 || totalDeleted > 0 || totalFailed > 0)
        {
            _logger.LogInformation(
                "Orphan-reaper sweep: {WouldReap} orphan(s) identified, {Deleted} deleted, {Failed} failed",
                totalWouldReap, totalDeleted, totalFailed);
        }

        _monitor.ReportTick(
            OpsServiceNames.OrphanReaper,
            totalDeleted,
            $"scanned {scopes.Count} scope(s): identified={totalWouldReap} deleted={totalDeleted} failed={totalFailed}");
    }

    /// <summary>
    /// An Azure scope to sweep, with the credentials needed to authenticate the
    /// <c>az</c> calls. <paramref name="ServicePrincipal"/> is non-null for
    /// cloud_account-derived scopes (we log in with those SP creds in an isolated
    /// AZURE_CONFIG_DIR); null for cloud_connection-derived scopes (managed
    /// identity / ambient session, same as the provisioner).
    /// </summary>
    internal readonly record struct AzureScope(
        string Subscription, string ResourceGroup, AzureServicePrincipal? ServicePrincipal, string Source);

    /// <summary>Service-principal creds decrypted from a cloud_account, used to
    /// <c>az login --service-principal</c> before listing/deleting.</summary>
    internal readonly record struct AzureServicePrincipal(string ClientId, string ClientSecret, string TenantId);

    /// <summary>
    /// Resolve the distinct set of Azure (subscription, resource-group) scopes to
    /// sweep, from active cloud_accounts (decrypted via the cipher) AND
    /// cloud_connections. De-duped by (subscription, resource-group) — a
    /// connection and an account pointing at the same scope are scanned once, and
    /// the connection (managed identity) is preferred so we don't need SP login
    /// when the ambient identity already covers the scope.
    /// </summary>
    internal static async Task<List<AzureScope>> ResolveAzureScopesAsync(
        NetworkerDbContext db, CredentialCipher? cipher, CancellationToken ct)
    {
        // Keyed by (sub, rg), case-insensitive on both — Azure ids/names are
        // case-insensitive. First writer wins; connections are added first so an
        // ambient-identity connection isn't shadowed by an SP-login account.
        var byScope = new Dictionary<(string Sub, string Rg), AzureScope>(ScopeKeyComparer.Instance);

        // ── cloud_connections (managed identity / secretless config) ──────────
        var conns = await db.CloudConnections.AsNoTracking()
            .Where(c => c.Provider == "azure")
            .Select(c => new { c.ConnectionId, c.Config })
            .ToListAsync(ct)
            .ConfigureAwait(false);

        foreach (var conn in conns)
        {
            var (sub, rg) = ParseAzureScopeFromConfig(conn.Config);
            if (string.IsNullOrEmpty(sub) || string.IsNullOrEmpty(rg))
            {
                continue;
            }

            var key = (sub!, rg!);
            byScope.TryAdd(key, new AzureScope(sub!, rg!, ServicePrincipal: null, Source: "cloud_connection"));
        }

        // ── cloud_accounts (encrypted SP creds) ───────────────────────────────
        // These are the prod path. Without a cipher we can't decrypt, so skip.
        if (cipher is not null)
        {
            var accounts = await db.CloudAccounts.AsNoTracking()
                .Where(a => a.Provider == "azure" && a.Status == "active")
                .Select(a => new { a.AccountId, a.CredentialsEnc, a.CredentialsNonce })
                .ToListAsync(ct)
                .ConfigureAwait(false);

            foreach (var acct in accounts)
            {
                Dictionary<string, string> creds;
                try
                {
                    var plain = cipher.Decrypt(acct.CredentialsEnc, acct.CredentialsNonce);
                    creds = ParseCreds(plain);
                }
                catch (Exception)
                {
                    // Undecryptable account (key rotation, corrupt nonce) — skip
                    // rather than fail the sweep. Same soft-fail posture as the
                    // precheck endpoint's decrypt_failed path.
                    continue;
                }

                var sub = creds.GetValueOrDefault("subscription_id", string.Empty);
                if (string.IsNullOrEmpty(sub))
                {
                    continue;
                }

                // RG resolution: match the provisioner exactly — the account's
                // resource_group claim, else the networker-testers default. NOT
                // DASHBOARD_AZURE_RG (that governs only the no-account fallback).
                var rg = creds.TryGetValue("resource_group", out var g) && !string.IsNullOrEmpty(g)
                    ? g
                    : DefaultAzureResourceGroup;

                AzureServicePrincipal? sp = null;
                var clientId = creds.GetValueOrDefault("client_id", string.Empty);
                var clientSecret = creds.GetValueOrDefault("client_secret", string.Empty);
                var tenantId = creds.GetValueOrDefault("tenant_id", string.Empty);
                if (!string.IsNullOrEmpty(clientId)
                    && !string.IsNullOrEmpty(clientSecret)
                    && !string.IsNullOrEmpty(tenantId))
                {
                    sp = new AzureServicePrincipal(clientId, clientSecret, tenantId);
                }

                var key = (sub, rg);
                byScope.TryAdd(key, new AzureScope(sub, rg, sp, Source: "cloud_account"));
            }
        }

        return byScope.Values.ToList();
    }

    /// <summary>Case-insensitive comparer for (subscription, resource-group) scope
    /// keys — Azure subscription ids and RG names are case-insensitive, so a
    /// connection and account differing only in case must de-dupe to one scan.
    /// </summary>
    private sealed class ScopeKeyComparer : IEqualityComparer<(string Sub, string Rg)>
    {
        public static readonly ScopeKeyComparer Instance = new();

        public bool Equals((string Sub, string Rg) x, (string Sub, string Rg) y) =>
            string.Equals(x.Sub, y.Sub, StringComparison.OrdinalIgnoreCase)
            && string.Equals(x.Rg, y.Rg, StringComparison.OrdinalIgnoreCase);

        public int GetHashCode((string Sub, string Rg) obj) =>
            HashCode.Combine(
                obj.Sub.ToLowerInvariant(),
                obj.Rg.ToLowerInvariant());
    }

    /// <summary>
    /// List + reap orphans for a single Azure scope. Returns (deleted, failed,
    /// wouldReap). When the CLI is missing, an SP login fails, or a list call
    /// fails, the scope is skipped and (0,0,0) is returned — no throw.
    /// </summary>
    private async Task<(int Deleted, int Failed, int WouldReap)> ReapAzureScopeAsync(
        AzureScope scope, HashSet<string> knownIds, IReadOnlyList<string> knownNamePrefixes, CancellationToken ct)
    {
        // SP-login isolation: when the scope carries service-principal creds
        // (cloud_account path), log in to an isolated AZURE_CONFIG_DIR and thread
        // it through every list/delete — never the host's ambient az identity.
        // Managed-identity / connection scopes use the ambient session (env=null).
        var (env, spConfigDir) = await PrepareAzEnvAsync(scope, ct).ConfigureAwait(false);
        if (scope.ServicePrincipal is not null && env is null)
        {
            // SP login was required but failed — can't safely scan this scope.
            return (0, 0, 0);
        }

        try
        {
            // List every VM / NIC / public-IP / disk / NSG in the resource group.
            // A failed list (missing CLI, auth error) yields an empty set for that
            // kind — the scope simply contributes nothing this pass (best-effort).
            var raw = new List<RawResource>();
            raw.AddRange(await AzListAsync("vm", ["vm", "list"], scope, env, ct).ConfigureAwait(false));
            raw.AddRange(await AzListAsync("nic", ["network", "nic", "list"], scope, env, ct).ConfigureAwait(false));
            raw.AddRange(await AzListAsync("public_ip", ["network", "public-ip", "list"], scope, env, ct).ConfigureAwait(false));
            raw.AddRange(await AzListAsync("disk", ["disk", "list"], scope, env, ct).ConfigureAwait(false));
            // NSG list — divergence from Rust (which never reaped NSGs). Mirrors
            // the nic/public-ip list shape.
            raw.AddRange(await AzListAsync("nsg", ["network", "nsg", "list"], scope, env, ct).ConfigureAwait(false));

            // Pure filter (Rust filter_orphans): unknown id AND owned-name prefix
            // AND not a child resource of a live tester (vm_name prefix guard).
            var orphans = FilterOrphans(raw, knownIds, knownNamePrefixes);
            if (orphans.Count == 0)
            {
                return (0, 0, 0);
            }

            _logger.LogInformation(
                "Orphan-reaper: {Count} orphan(s) in {Sub}/{Rg} ({Source}) would be reaped: {Names}",
                orphans.Count, scope.Subscription, scope.ResourceGroup, scope.Source,
                string.Join(", ", orphans.Select(o => $"{o.Kind}:{o.Name}")));

            var deleted = 0;
            var failed = 0;

            // Dependency-safe order: VM → NIC → disk → IP → NSG. Soft-fail per
            // resource.
            foreach (var kind in DeleteOrder)
            {
                foreach (var o in orphans.Where(o => o.Kind == kind))
                {
                    ct.ThrowIfCancellationRequested();
                    var ok = await AzDeleteOneAsync(o.Kind, o.ResourceId, scope, env, ct).ConfigureAwait(false);
                    if (ok)
                    {
                        deleted++;
                        _logger.LogInformation("Orphan-reaper deleted {Kind} {Name} ({Id})", o.Kind, o.Name, o.ResourceId);
                    }
                    else
                    {
                        failed++;
                        _logger.LogWarning("Orphan-reaper failed to delete {Kind} {Name} ({Id})", o.Kind, o.Name, o.ResourceId);
                    }
                }
            }

            return (deleted, failed, orphans.Count);
        }
        finally
        {
            if (spConfigDir is not null)
            {
                TryDeleteDir(spConfigDir);
            }
        }
    }

    /// <summary>
    /// Build the env dict for the scope's <c>az</c> calls. For an SP scope, logs
    /// in with the account's service-principal creds into an isolated
    /// AZURE_CONFIG_DIR (never the ambient identity — same as
    /// <c>CliComputeProvisioner</c>). Returns (env, spConfigDir): a null env with
    /// a non-null SP on the scope means login failed; the caller then skips the
    /// scope. Connection scopes return (null, null) → ambient session.
    /// </summary>
    private async Task<(IReadOnlyDictionary<string, string>? Env, string? SpConfigDir)> PrepareAzEnvAsync(
        AzureScope scope, CancellationToken ct)
    {
        if (scope.ServicePrincipal is not { } sp)
        {
            return (null, null);
        }

        var spConfigDir = Path.Combine(Path.GetTempPath(), $"az-reaper-{Guid.NewGuid():N}");
        Directory.CreateDirectory(spConfigDir);

        var loginEnv = new Dictionary<string, string>
        {
            ["AZURE_CONFIG_DIR"] = spConfigDir,
            ["PYTHONWARNINGS"] = "ignore",
        };
        var (spawned, exit, _, _) = await RunAsync(
            AzBin(),
            new List<string>
            {
                "login", "--service-principal", "-u", sp.ClientId, "-p", sp.ClientSecret,
                "--tenant", sp.TenantId, "--output", "none",
            },
            loginEnv,
            ct,
            sensitiveArgs: true).ConfigureAwait(false);

        if (!spawned || exit != 0)
        {
            _logger.LogWarning(
                "Orphan-reaper: az login --service-principal failed for {Sub}/{Rg}; skipping scope",
                scope.Subscription, scope.ResourceGroup);
            TryDeleteDir(spConfigDir);
            return (null, spConfigDir: null);
        }

        return (loginEnv, spConfigDir);
    }

    /// <summary>A raw cloud resource record used by the filter logic. Broken out
    /// so the filter is testable without any cloud calls (Rust
    /// <c>RawResource</c>).</summary>
    internal readonly record struct RawResource(string ResourceId, string Name, string Kind, string Provider);

    /// <summary>A resource the reaper has identified as an orphan (Rust
    /// <c>OrphanResource</c>).</summary>
    internal readonly record struct OrphanResource(string ResourceId, string Name, string Kind, string Provider);

    /// <summary>
    /// Pure filter (Rust <c>filter_orphans</c>, plus the vm-name-prefix guard):
    /// keep a resource only when its id is NOT in <paramref name="knownIds"/>,
    /// its name matches an owned prefix (<see cref="NameIsOurs"/>), AND its name
    /// does NOT start with the <c>vm_name</c> of any live tester
    /// (<paramref name="knownVmNamePrefixes"/>). The last guard is essential: a
    /// tester's NIC / disk / public-IP / NSG are named <c>&lt;vm_name&gt;…</c>
    /// with distinct resource ids not in <paramref name="knownIds"/>, so without
    /// it every live tester's child resources look like orphans.
    /// </summary>
    internal static List<OrphanResource> FilterOrphans(
        IEnumerable<RawResource> resources,
        HashSet<string> knownIds,
        IReadOnlyList<string> knownVmNamePrefixes) =>
        resources
            .Where(r =>
                !knownIds.Contains(r.ResourceId)
                && NameIsOurs(r.Name)
                && !StartsWithKnownVmName(r.Name, knownVmNamePrefixes))
            .Select(r => new OrphanResource(r.ResourceId, r.Name, r.Kind, r.Provider))
            .ToList();

    /// <summary>True if <paramref name="name"/> starts with the vm_name of a live
    /// tester — i.e. it's a child resource (NIC/disk/IP/NSG) of infra still in
    /// use, not an orphan. Case-insensitive.</summary>
    internal static bool StartsWithKnownVmName(string name, IReadOnlyList<string> knownVmNamePrefixes)
    {
        foreach (var vm in knownVmNamePrefixes)
        {
            if (vm.Length > 0 && name.StartsWith(vm, StringComparison.OrdinalIgnoreCase))
            {
                return true;
            }
        }
        return false;
    }

    /// <summary>True if <paramref name="name"/> starts with one of our owned
    /// prefixes (case-insensitive). Mirrors the Rust <c>name_is_ours</c>.</summary>
    internal static bool NameIsOurs(string name)
    {
        var n = name.ToLowerInvariant();
        foreach (var p in OwnedNamePrefixes)
        {
            if (n.StartsWith(p, StringComparison.Ordinal))
            {
                return true;
            }
        }
        return false;
    }

    /// <summary>
    /// Extract (subscription_id, resource_group) from a cloud_connection config
    /// JSON. Same shape the tester endpoints read. Returns (null, null) when the
    /// config isn't parseable JSON or lacks the keys.
    /// </summary>
    internal static (string? Subscription, string? ResourceGroup) ParseAzureScopeFromConfig(string config)
    {
        try
        {
            using var doc = JsonDocument.Parse(config);
            var root = doc.RootElement;
            if (root.ValueKind != JsonValueKind.Object)
            {
                return (null, null);
            }
            string? sub = null, rg = null;
            if (root.TryGetProperty("subscription_id", out var s) && s.ValueKind == JsonValueKind.String)
            {
                sub = s.GetString();
            }
            if (root.TryGetProperty("resource_group", out var g) && g.ValueKind == JsonValueKind.String)
            {
                rg = g.GetString();
            }
            return (sub, rg);
        }
        catch (JsonException)
        {
            return (null, null);
        }
    }

    /// <summary>Parse decrypted account credential JSON into a string→string map
    /// (string values as-is, non-strings as raw JSON). Same shape as
    /// <c>TesterPrecheckEndpoints.ParseCreds</c>.</summary>
    private static Dictionary<string, string> ParseCreds(byte[] plain)
    {
        var map = new Dictionary<string, string>(StringComparer.Ordinal);
        using var doc = JsonDocument.Parse(plain);
        if (doc.RootElement.ValueKind == JsonValueKind.Object)
        {
            foreach (var prop in doc.RootElement.EnumerateObject())
            {
                map[prop.Name] = prop.Value.ValueKind == JsonValueKind.String
                    ? prop.Value.GetString() ?? string.Empty
                    : prop.Value.GetRawText();
            }
        }
        return map;
    }

    // ── Azure CLI list/delete (az) — best-effort; missing CLI ⇒ empty/no-op ────

    private static string AzBin() =>
        Environment.GetEnvironmentVariable("AZ_CMD") is { Length: > 0 } o ? o : "az";

    /// <summary>
    /// Run <c>az &lt;subcommand&gt; list --subscription --resource-group --query
    /// [].{id:id,name:name} --output json</c> and project into
    /// <see cref="RawResource"/>. Returns an empty list on any failure (missing
    /// CLI, auth error, non-zero exit) — the caller treats "no results" as "list
    /// unavailable this pass", so nothing is reaped. Mirrors the Rust
    /// <c>az_list_json</c> (which also swallows failures via <c>unwrap_or_default</c>).
    /// </summary>
    private async Task<List<RawResource>> AzListAsync(
        string kind, string[] subcommand, AzureScope scope,
        IReadOnlyDictionary<string, string>? env, CancellationToken ct)
    {
        var args = new List<string>(subcommand)
        {
            "--subscription", scope.Subscription,
            "--resource-group", scope.ResourceGroup,
            "--query", "[].{id:id,name:name}",
            "--output", "json",
        };

        var (spawned, exit, stdout, _) = await RunAsync(AzBin(), args, env, ct).ConfigureAwait(false);
        if (!spawned)
        {
            // CI / credential-less host: az isn't installed. This is the no-op
            // path — log once per kind at debug and return empty.
            _logger.LogDebug("Orphan-reaper: az CLI unavailable; skipping {Kind} list (would list {Sub}/{Rg})",
                kind, scope.Subscription, scope.ResourceGroup);
            return [];
        }
        if (exit != 0 || string.IsNullOrWhiteSpace(stdout))
        {
            return [];
        }

        var results = new List<RawResource>();
        try
        {
            using var doc = JsonDocument.Parse(stdout);
            if (doc.RootElement.ValueKind == JsonValueKind.Array)
            {
                foreach (var el in doc.RootElement.EnumerateArray())
                {
                    if (el.ValueKind != JsonValueKind.Object)
                    {
                        continue;
                    }
                    var id = el.TryGetProperty("id", out var i) && i.ValueKind == JsonValueKind.String ? i.GetString() : null;
                    var name = el.TryGetProperty("name", out var n) && n.ValueKind == JsonValueKind.String ? n.GetString() : null;
                    if (!string.IsNullOrEmpty(id) && !string.IsNullOrEmpty(name))
                    {
                        results.Add(new RawResource(id!, name!, kind, "azure"));
                    }
                }
            }
        }
        catch (JsonException)
        {
            // Unparseable output → treat as no results (best-effort).
            return [];
        }
        return results;
    }

    /// <summary>
    /// Build the argv for deleting a single orphan by kind. Pure/testable — no
    /// process spawn. Returns null for an unknown kind. NSG delete
    /// (<c>az network nsg delete</c>) is the C# divergence from Rust.
    /// </summary>
    internal static List<string>? BuildDeleteArgs(string kind, string id, string subscription) =>
        kind switch
        {
            "vm" => ["vm", "delete", "--subscription", subscription, "--ids", id, "--yes"],
            "nic" => ["network", "nic", "delete", "--subscription", subscription, "--ids", id],
            "public_ip" => ["network", "public-ip", "delete", "--subscription", subscription, "--ids", id],
            "disk" => ["disk", "delete", "--subscription", subscription, "--ids", id, "--yes"],
            "nsg" => ["network", "nsg", "delete", "--subscription", subscription, "--ids", id],
            _ => null,
        };

    /// <summary>
    /// Delete a single orphan by kind (Rust <c>az_delete_one</c>). Returns true on
    /// success (including "already gone"), false on a real failure or a missing
    /// CLI. Never throws.
    /// </summary>
    private async Task<bool> AzDeleteOneAsync(
        string kind, string id, AzureScope scope,
        IReadOnlyDictionary<string, string>? env, CancellationToken ct)
    {
        var args = BuildDeleteArgs(kind, id, scope.Subscription);
        if (args is null)
        {
            _logger.LogWarning("Orphan-reaper: unknown orphan kind '{Kind}'", kind);
            return false;
        }

        var (spawned, exit, _, stderr) = await RunAsync(AzBin(), args, env, ct).ConfigureAwait(false);
        if (!spawned)
        {
            // Missing CLI — no-op. (We only get here if a list somehow succeeded
            // but the CLI later vanished; treat as a soft failure.)
            return false;
        }
        if (exit == 0)
        {
            return true;
        }
        // "Already gone" is the desired end-state — idempotent delete.
        return LooksAlreadyGone(stderr);
    }

    private static bool LooksAlreadyGone(string stderr) =>
        stderr.Contains("ResourceNotFound", StringComparison.OrdinalIgnoreCase)
        || stderr.Contains("could not be found", StringComparison.OrdinalIgnoreCase)
        || stderr.Contains("does not exist", StringComparison.OrdinalIgnoreCase)
        || stderr.Contains("was not found", StringComparison.OrdinalIgnoreCase)
        || stderr.Contains("404", StringComparison.OrdinalIgnoreCase);

    /// <summary>
    /// Spawn a CLI, drain stdout/stderr concurrently, enforce a hard timeout with
    /// tree-kill. Total: a missing binary returns <c>spawned = false</c> rather
    /// than throwing — the CI no-op path. Ported from
    /// <see cref="Provisioning.CliComputeProvisioner"/>.
    /// </summary>
    private async Task<(bool Spawned, int ExitCode, string StdOut, string StdErr)> RunAsync(
        string file, List<string> args, IReadOnlyDictionary<string, string>? env,
        CancellationToken cancellationToken, bool sensitiveArgs = false)
    {
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
        // Keep az's Python warnings out of stdout so JSON parsing stays clean.
        psi.Environment["PYTHONWARNINGS"] = "ignore";
        if (env is not null)
        {
            foreach (var (k, v) in env)
            {
                psi.Environment[k] = v;
            }
        }

        if (sensitiveArgs)
        {
            _logger.LogDebug("Orphan-reaper spawning {File} (args redacted: contains credentials)", file);
        }

        using var timeoutCts = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeoutCts.CancelAfter(CommandTimeout);
        var ct = timeoutCts.Token;

        using var process = new Process { StartInfo = psi };
        try
        {
            process.Start();
        }
        catch (Exception ex)
        {
            // The common CI path: the cloud CLI isn't installed. Soft failure.
            _logger.LogDebug(ex, "Orphan-reaper: failed to launch CLI '{File}'", file);
            return (false, -1, string.Empty, string.Empty);
        }

        var stdoutTask = process.StandardOutput.ReadToEndAsync(ct);
        var stderrTask = process.StandardError.ReadToEndAsync(ct);
        try
        {
            await process.WaitForExitAsync(ct).ConfigureAwait(false);
            var stdout = (await stdoutTask.ConfigureAwait(false)).Trim();
            var stderr = (await stderrTask.ConfigureAwait(false)).Trim();
            return (true, process.ExitCode, stdout, stderr);
        }
        catch (OperationCanceledException) when (timeoutCts.IsCancellationRequested
                                                 && !cancellationToken.IsCancellationRequested)
        {
            KillTree(process);
            _logger.LogWarning("Orphan-reaper: CLI '{File}' timed out after {Secs}s and was killed",
                file, CommandTimeout.TotalSeconds);
            return (true, -1, string.Empty, "timed out");
        }
        catch (OperationCanceledException)
        {
            KillTree(process); // caller cancelled — don't leave the child running
            throw;
        }
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
}
