using System.Diagnostics;
using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.Data;

namespace Networker.ControlPlane.Background;

/// <summary>
/// Cloud orphan reaper — the C# port of the Rust dashboard's
/// <c>cloud_orphan_reaper</c>
/// (<c>crates/networker-dashboard/src/services/cloud_orphan_reaper.rs</c>).
///
/// <para><b>Why:</b> failed tester / benchmark VM creations leave behind NICs,
/// public IPs, and disks that reference each other but have <i>no</i> row in our
/// DB. Over time these pile up and hit cloud quotas (Azure defaults to ~10 public
/// IPs / subscription / region). This service lists such resources, keeps only
/// those that (a) are <b>not</b> referenced by any DB row and (b) match a
/// conservative <b>owned-name allow-list</b> (<c>tester-*</c>, <c>ab-*</c>,
/// <c>nwk-ep-*</c>), then deletes them in dependency-safe order:
/// <c>VM → NIC → Disk → IP</c>. Each delete is best-effort — one failure never
/// stops the rest (soft-fail per resource).</para>
///
/// <para><b>Tick:</b> slower than the tester loops (~10min) — quota pressure
/// builds slowly and each pass fans out several cloud <c>list</c> calls.</para>
///
/// <para><b>CI-safe / no-op behaviour:</b> listing cloud resources needs cloud
/// access that CI and credential-less hosts don't have. The service is
/// structured faithfully (per-provider list of NICs/IPs/disks by prefix,
/// cross-reference DB, ordered delete, soft-fail) but the actual cloud calls are
/// <b>best-effort</b>: if the CLI binary is missing, or the required
/// subscription/resource-group (Azure) can't be resolved from a
/// <c>cloud_connection</c>, or a <c>list</c> call exits non-zero, the provider is
/// skipped and we log what we <i>would</i> reap rather than doing anything. AWS
/// and GCP listing are stubs (return empty) exactly as in the Rust source — only
/// Azure has a real implementation. The net effect in CI: zero cloud calls
/// succeed, zero deletes happen, and the loop stays healthy.</para>
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

    /// <summary>
    /// Allow-list of name prefixes this reaper is willing to touch. Anything
    /// else is left alone regardless of whether its resource id is in the
    /// known-set — defence-in-depth against destroying other tenants' resources
    /// in a shared subscription / resource group. Mirrors the Rust
    /// <c>OWNED_NAME_PREFIXES</c>.
    /// </summary>
    internal static readonly string[] OwnedNamePrefixes = ["tester-", "ab-", "nwk-ep-"];

    /// <summary>Delete order: a VM delete releases its NIC lease; deleting the
    /// NIC releases the IP; the disk is safe to delete at any point. Mirrors the
    /// Rust <c>["vm", "nic", "disk", "public_ip"]</c> ordering.</summary>
    private static readonly string[] DeleteOrder = ["vm", "nic", "disk", "public_ip"];

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

        // Each distinct Azure connection (subscription + resource group) is a
        // separate scope to list/reap. Connections whose config can't yield a
        // subscription+resource-group are skipped (nothing to scope a list to).
        var azureConns = await db.CloudConnections.AsNoTracking()
            .Where(c => c.Provider == "azure")
            .Select(c => new { c.ConnectionId, c.Config })
            .ToListAsync(ct)
            .ConfigureAwait(false);

        if (azureConns.Count == 0)
        {
            // No configured Azure scope → nothing to list. AWS/GCP are stubs
            // (Rust returns empty Vec), so the sweep is a no-op.
            _logger.LogDebug("Orphan-reaper: no Azure cloud_connection configured; nothing to scan");
            _monitor.ReportTick(OpsServiceNames.OrphanReaper, 0, "no azure cloud_connection configured");
            return;
        }

        var totalDeleted = 0;
        var totalFailed = 0;
        var totalWouldReap = 0;

        foreach (var conn in azureConns)
        {
            var (sub, rg) = ParseAzureScope(conn.Config);
            if (string.IsNullOrEmpty(sub) || string.IsNullOrEmpty(rg))
            {
                _logger.LogDebug(
                    "Orphan-reaper: Azure connection {ConnId} lacks subscription/resource-group in config; skipping",
                    conn.ConnectionId);
                continue;
            }

            var (deleted, failed, wouldReap) =
                await ReapAzureScopeAsync(sub!, rg!, knownSet, ct).ConfigureAwait(false);
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
            $"identified={totalWouldReap} deleted={totalDeleted} failed={totalFailed}");
    }

    /// <summary>
    /// List + reap orphans for a single Azure (subscription, resource-group)
    /// scope. Returns (deleted, failed, wouldReap). When the CLI is missing or a
    /// list call fails, the scope is skipped and (0,0,0) is returned — no throw.
    /// </summary>
    private async Task<(int Deleted, int Failed, int WouldReap)> ReapAzureScopeAsync(
        string subscription, string resourceGroup, HashSet<string> knownIds, CancellationToken ct)
    {
        // List every VM / NIC / public-IP / disk in the resource group. A failed
        // list (missing CLI, auth error) yields an empty set for that kind — the
        // scope simply contributes nothing this pass (best-effort).
        var raw = new List<RawResource>();
        raw.AddRange(await AzListAsync("vm", ["vm", "list"], subscription, resourceGroup, ct).ConfigureAwait(false));
        raw.AddRange(await AzListAsync("nic", ["network", "nic", "list"], subscription, resourceGroup, ct).ConfigureAwait(false));
        raw.AddRange(await AzListAsync("public_ip", ["network", "public-ip", "list"], subscription, resourceGroup, ct).ConfigureAwait(false));
        raw.AddRange(await AzListAsync("disk", ["disk", "list"], subscription, resourceGroup, ct).ConfigureAwait(false));

        // Pure filter (Rust filter_orphans): unknown id AND owned-name prefix.
        var orphans = FilterOrphans(raw, knownIds);
        if (orphans.Count == 0)
        {
            return (0, 0, 0);
        }

        // If we couldn't actually spawn the CLI this pass, we still ran the filter
        // over an empty list, so orphans is empty and we won't reach here. Any
        // orphans present mean the list calls succeeded → we may attempt deletes.
        _logger.LogInformation(
            "Orphan-reaper: {Count} orphan(s) in {Sub}/{Rg} would be reaped: {Names}",
            orphans.Count, subscription, resourceGroup,
            string.Join(", ", orphans.Select(o => $"{o.Kind}:{o.Name}")));

        var deleted = 0;
        var failed = 0;

        // Dependency-safe order: VM → NIC → disk → IP. Soft-fail per resource.
        foreach (var kind in DeleteOrder)
        {
            foreach (var o in orphans.Where(o => o.Kind == kind))
            {
                ct.ThrowIfCancellationRequested();
                var ok = await AzDeleteOneAsync(o.Kind, o.ResourceId, subscription, ct).ConfigureAwait(false);
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

    /// <summary>A raw cloud resource record used by the filter logic. Broken out
    /// so the filter is testable without any cloud calls (Rust
    /// <c>RawResource</c>).</summary>
    internal readonly record struct RawResource(string ResourceId, string Name, string Kind, string Provider);

    /// <summary>A resource the reaper has identified as an orphan (Rust
    /// <c>OrphanResource</c>).</summary>
    internal readonly record struct OrphanResource(string ResourceId, string Name, string Kind, string Provider);

    /// <summary>
    /// Pure filter (Rust <c>filter_orphans</c>): keep only resources whose id is
    /// NOT in <paramref name="knownIds"/> AND whose name matches an owned prefix.
    /// </summary>
    internal static List<OrphanResource> FilterOrphans(
        IEnumerable<RawResource> resources, HashSet<string> knownIds) =>
        resources
            .Where(r => !knownIds.Contains(r.ResourceId) && NameIsOurs(r.Name))
            .Select(r => new OrphanResource(r.ResourceId, r.Name, r.Kind, r.Provider))
            .ToList();

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
    private static (string? Subscription, string? ResourceGroup) ParseAzureScope(string config)
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
        string kind, string[] subcommand, string subscription, string resourceGroup, CancellationToken ct)
    {
        var args = new List<string>(subcommand)
        {
            "--subscription", subscription,
            "--resource-group", resourceGroup,
            "--query", "[].{id:id,name:name}",
            "--output", "json",
        };

        var (spawned, exit, stdout, _) = await RunAsync(AzBin(), args, ct).ConfigureAwait(false);
        if (!spawned)
        {
            // CI / credential-less host: az isn't installed. This is the no-op
            // path — log once per kind at debug and return empty.
            _logger.LogDebug("Orphan-reaper: az CLI unavailable; skipping {Kind} list (would list {Sub}/{Rg})",
                kind, subscription, resourceGroup);
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
    /// Delete a single orphan by kind (Rust <c>az_delete_one</c>). Returns true on
    /// success (including "already gone"), false on a real failure or a missing
    /// CLI. Never throws.
    /// </summary>
    private async Task<bool> AzDeleteOneAsync(string kind, string id, string subscription, CancellationToken ct)
    {
        List<string>? args = kind switch
        {
            "vm" => ["vm", "delete", "--subscription", subscription, "--ids", id, "--yes"],
            "nic" => ["network", "nic", "delete", "--subscription", subscription, "--ids", id],
            "public_ip" => ["network", "public-ip", "delete", "--subscription", subscription, "--ids", id],
            "disk" => ["disk", "delete", "--subscription", subscription, "--ids", id, "--yes"],
            _ => null,
        };
        if (args is null)
        {
            _logger.LogWarning("Orphan-reaper: unknown orphan kind '{Kind}'", kind);
            return false;
        }

        var (spawned, exit, _, stderr) = await RunAsync(AzBin(), args, ct).ConfigureAwait(false);
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
        string file, List<string> args, CancellationToken cancellationToken)
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
}
