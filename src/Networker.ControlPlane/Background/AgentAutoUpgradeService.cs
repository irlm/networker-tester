using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Endpoints;
using Networker.ControlPlane.Provisioning;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Background;

/// <summary>
/// Runner auto-upgrade sweep — closes the gap found 2026-08-12: nothing ever
/// upgraded a deployed runner, so agents drifted arbitrarily stale (the eastus
/// runner sat on 0.28.126 for ~70 releases and its probes were missing a month
/// of measurement fixes). <see cref="VersionRefreshService"/> only feeds the
/// version endpoints; the reinstall machinery existed solely behind the manual
/// <c>POST /testers/{id}/upgrade</c>.
///
/// <para>Every ~10 min (leader-gated), compare each ONLINE agent's
/// self-reported version against the control plane's own version — the floor
/// every deploy establishes; deliberately NOT the GitHub "latest" cache, so a
/// runner is never upgraded past what the control plane itself runs. Stale
/// agents whose tester rows satisfy the manual-upgrade guards (azure,
/// <c>power_state=running</c>, <c>allocation=idle</c>, no non-terminal runs)
/// get <see cref="TesterInstallScripts.ReinstallScript"/> over cloud
/// run-command — the exact script the manual endpoint runs.</para>
///
/// <para><b>Safety posture:</b></para>
/// <list type="bullet">
///   <item>At most <see cref="MaxUpgradesPerSweep"/> upgrades per sweep — a
///   fleet converges over a few ticks instead of restarting every runner's
///   agent at once.</item>
///   <item>Guarded CAS <c>running → upgrading</c> before touching the VM
///   (same transition discipline as <see cref="AutoShutdownService"/>); the
///   row always lands back on <c>running</c> — the VM is never powered off by
///   an upgrade.</item>
///   <item>Per-tester failure backoff (<see cref="FailureBackoff"/>, in-memory)
///   so a wedged VM or missing CLI doesn't get hammered every tick. A leader
///   restart clears it — acceptable, the guards re-apply.</item>
///   <item>Same dispatch race as the manual endpoint: a run handed to the
///   agent in the seconds before <c>systemctl restart</c> dies with the agent
///   and is recovered by the redispatch/watchdog loops.</item>
/// </list>
/// </summary>
public sealed class AgentAutoUpgradeService : BackgroundService
{
    /// <summary>Sweep cadence. Upgrades are rare and run-command is slow
    /// (~1-2 min per VM) — no reason to tick at the 60s lifecycle rate.</summary>
    private static readonly TimeSpan TickInterval = TimeSpan.FromMinutes(10);

    /// <summary>Upper bound on upgrades started in one sweep.</summary>
    internal const int MaxUpgradesPerSweep = 2;

    /// <summary>How long a tester that failed an auto-upgrade is left alone
    /// before the sweep retries it.</summary>
    internal static readonly TimeSpan FailureBackoff = TimeSpan.FromHours(6);

    private readonly IServiceScopeFactory _scopeFactory;
    private readonly ILogger<AgentAutoUpgradeService> _logger;
    private readonly PgAdvisoryLeaderLock? _leader;
    private readonly TickMonitor _monitor;
    private readonly Dictionary<Guid, DateTime> _failedAtUtc = new();

    public AgentAutoUpgradeService(
        IServiceScopeFactory scopeFactory,
        ILogger<AgentAutoUpgradeService> logger,
        PgAdvisoryLeaderLock? leaderLock = null,
        TickMonitor? tickMonitor = null)
    {
        _scopeFactory = scopeFactory;
        _logger = logger;
        _leader = leaderLock;
        _monitor = tickMonitor ?? new TickMonitor();
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        _logger.LogInformation(
            "Agent auto-upgrade service started (tick every {Minutes}min, floor {Floor})",
            TickInterval.TotalMinutes, VersionEndpoints.DashboardVersion);
        _monitor.ReportStarted(OpsServiceNames.AgentAutoUpgrade);

        using var timer = new PeriodicTimer(TickInterval);
        while (await timer.WaitForNextTickAsync(stoppingToken).ConfigureAwait(false))
        {
            try
            {
                await _leader
                    .TryRunGuardedAsync(LeaderLockKeys.AgentAutoUpgrade, SweepAsync, stoppingToken)
                    .ConfigureAwait(false);
            }
            catch (OperationCanceledException) when (stoppingToken.IsCancellationRequested)
            {
                break;
            }
            catch (Exception ex)
            {
                _monitor.ReportError(OpsServiceNames.AgentAutoUpgrade, ex);
                _logger.LogError(ex, "Agent auto-upgrade sweep failed");
            }
        }
    }

    internal async Task SweepAsync(CancellationToken ct)
    {
        using var scope = _scopeFactory.CreateScope();
        var sp = scope.ServiceProvider;
        var db = sp.GetRequiredService<NetworkerDbContext>();
        var provisioner = sp.GetRequiredService<IComputeProvisioner>();

        if (!Version.TryParse(VersionEndpoints.DashboardVersion, out var floor))
        {
            // Dev/test builds can carry non-triple versions; without a parseable
            // floor there is nothing meaningful to compare against.
            _monitor.ReportTick(OpsServiceNames.AgentAutoUpgrade, 0, "unparseable floor version");
            return;
        }

        // ONLINE agents only: version is self-reported over the live socket, so
        // an online agent's staleness is current fact, not a fossil. Offline
        // runners get their turn when they wake (they re-heartbeat as online).
        var onlineAgents = await db.Agents.AsNoTracking()
            .Where(a => a.Status == "online" && a.TesterId != null && a.Version != null)
            .Select(a => new { a.TesterId, a.Version, a.Name })
            .ToListAsync(ct)
            .ConfigureAwait(false);

        var staleTesterIds = onlineAgents
            .Where(a => Version.TryParse(a.Version, out var v) && v < floor)
            .Select(a => a.TesterId!.Value)
            .Distinct()
            .ToList();

        if (staleTesterIds.Count == 0)
        {
            _monitor.ReportTick(OpsServiceNames.AgentAutoUpgrade, 0, "all online agents at floor");
            return;
        }

        // Same eligibility guards as the manual upgrade endpoint.
        var candidates = await db.ProjectTesters
            .Where(t => staleTesterIds.Contains(t.TesterId)
                && t.Cloud == "azure"
                && t.PowerState == "running"
                && t.Allocation == "idle"
                && !db.TestRuns.Any(r =>
                    r.TesterId == t.TesterId
                    && (r.Status == "queued" || r.Status == "provisioning" || r.Status == "running")))
            .OrderBy(t => t.Name)
            .ToListAsync(ct)
            .ConfigureAwait(false);

        var skippedClouds = staleTesterIds.Count - candidates.Count;
        var upgraded = 0;
        var failed = 0;

        foreach (var tester in candidates)
        {
            ct.ThrowIfCancellationRequested();
            if (upgraded + failed >= MaxUpgradesPerSweep)
            {
                break;
            }
            if (_failedAtUtc.TryGetValue(tester.TesterId, out var lastFail)
                && DateTime.UtcNow - lastFail < FailureBackoff)
            {
                continue;
            }

            try
            {
                if (await UpgradeOneAsync(db, provisioner, tester, ct).ConfigureAwait(false))
                {
                    upgraded++;
                    _failedAtUtc.Remove(tester.TesterId);
                }
                else
                {
                    failed++;
                    _failedAtUtc[tester.TesterId] = DateTime.UtcNow;
                }
            }
            catch (Exception ex)
            {
                // One poison tester must never abort the batch.
                failed++;
                _failedAtUtc[tester.TesterId] = DateTime.UtcNow;
                _logger.LogWarning(ex,
                    "Auto-upgrade failed for {TesterId} ({Name})", tester.TesterId, tester.Name);
            }
        }

        _logger.LogInformation(
            "Agent auto-upgrade sweep: {Stale} stale agent(s), {Upgraded} upgraded, {Failed} failed, {Skipped} ineligible",
            staleTesterIds.Count, upgraded, failed, skippedClouds);
        _monitor.ReportTick(
            OpsServiceNames.AgentAutoUpgrade,
            staleTesterIds.Count,
            $"upgraded={upgraded} failed={failed} ineligible={skippedClouds}");
    }

    /// <summary>Reinstall one runner in place; returns true when the reinstall
    /// actually ran to success. The row always lands back on 'running'.</summary>
    private async Task<bool> UpgradeOneAsync(
        NetworkerDbContext db, IComputeProvisioner provisioner, ProjectTester tester, CancellationToken ct)
    {
        var tag = TesterInstallScripts.PreferredReleaseTag(VersionEndpoints.DashboardVersion);
        var target = TesterInstallScripts.ReleaseTarget(tester.OsArch ?? "x86_64");
        var script = TesterInstallScripts.ReinstallScript(tag, target);

        // Guarded claim; if anything else moved the row since the sweep query
        // (a run grabbed it, auto-shutdown fired), skip this cycle.
        var claimed = await db.ProjectTesters
            .Where(t => t.TesterId == tester.TesterId
                && t.PowerState == "running" && t.Allocation == "idle")
            .ExecuteUpdateAsync(
                s => s.SetProperty(t => t.PowerState, "upgrading")
                      .SetProperty(t => t.StatusMessage, $"Auto-upgrade to {tag}")
                      .SetProperty(t => t.UpdatedAt, DateTime.UtcNow),
                ct)
            .ConfigureAwait(false);
        if (claimed == 0)
        {
            _logger.LogDebug("Auto-upgrade skipped {TesterId}: no longer running+idle", tester.TesterId);
            return false;
        }

        _logger.LogInformation(
            "Auto-upgrading {Name} ({TesterId}) to {Tag} — agent reported a stale version",
            tester.Name, tester.TesterId, tag);

        var creds = await LoadCredentialsAsync(db, tester, ct).ConfigureAwait(false);
        var res = await provisioner.RunCommandAsync(tester, creds, script, ct).ConfigureAwait(false);

        // The VM stays powered on throughout — every outcome lands 'running'
        // (manual endpoint semantics). But unlike a power action, a missing
        // CLI means the reinstall did NOT happen, so it is not a success here:
        // record it and back off rather than claiming an upgrade.
        var ok = res.Success && res.ExitCode is not null;
        var message = ok
            ? $"Auto-upgraded to {tag}"
            : res.ExitCode is null
                ? $"Auto-upgrade to {tag} skipped: cloud CLI unavailable on this host"
                : $"Auto-upgrade to {tag} failed: {Truncate(res.Error ?? res.StdErr, 300)}";

        if (ok)
        {
            await db.ProjectTesters
                .Where(t => t.TesterId == tester.TesterId)
                .ExecuteUpdateAsync(
                    s => s.SetProperty(t => t.PowerState, "running")
                          .SetProperty(t => t.StatusMessage, message)
                          .SetProperty(t => t.InstallerVersion, VersionEndpoints.DashboardVersion)
                          .SetProperty(t => t.LastInstalledAt, DateTime.UtcNow)
                          .SetProperty(t => t.UpdatedAt, DateTime.UtcNow),
                    ct)
                .ConfigureAwait(false);
        }
        else
        {
            await db.ProjectTesters
                .Where(t => t.TesterId == tester.TesterId)
                .ExecuteUpdateAsync(
                    s => s.SetProperty(t => t.PowerState, "running")
                          .SetProperty(t => t.StatusMessage, message)
                          .SetProperty(t => t.UpdatedAt, DateTime.UtcNow),
                    ct)
                .ConfigureAwait(false);
        }

        if (ok)
        {
            _logger.LogInformation(
                "Auto-upgrade completed for {Name} ({TesterId}) → {Tag}; agent restarting",
                tester.Name, tester.TesterId, tag);
        }
        else
        {
            _logger.LogWarning(
                "Auto-upgrade did not complete for {Name} ({TesterId}): {Message}",
                tester.Name, tester.TesterId, message);
        }
        return ok;
    }

    private static string Truncate(string? s, int max)
    {
        s ??= "";
        return s.Length <= max ? s : s[..max];
    }

    /// <summary>Same per-connection credential resolution as
    /// <see cref="AutoShutdownService"/> / the write endpoints: null (ambient
    /// CLI auth) when the tester has no cloud connection.</summary>
    private static async Task<ProviderCredentials?> LoadCredentialsAsync(
        NetworkerDbContext db, ProjectTester tester, CancellationToken ct)
    {
        if (tester.CloudConnectionId is not { } connId)
        {
            return null;
        }

        var conn = await db.CloudConnections.AsNoTracking()
            .FirstOrDefaultAsync(c => c.ConnectionId == connId, ct)
            .ConfigureAwait(false);
        if (conn is null)
        {
            return null;
        }

        var extra = new Dictionary<string, string>(StringComparer.Ordinal);
        string? sub = null, rg = null, region = tester.Region;
        try
        {
            using var doc = System.Text.Json.JsonDocument.Parse(conn.Config);
            var root = doc.RootElement;
            if (root.ValueKind == System.Text.Json.JsonValueKind.Object)
            {
                foreach (var prop in root.EnumerateObject())
                {
                    if (prop.Value.ValueKind == System.Text.Json.JsonValueKind.String)
                    {
                        extra[prop.Name] = prop.Value.GetString() ?? string.Empty;
                    }
                }
            }
            extra.TryGetValue("subscription_id", out sub);
            extra.TryGetValue("resource_group", out rg);
            if (extra.TryGetValue("region", out var r) && !string.IsNullOrEmpty(r))
            {
                region = r;
            }
        }
        catch (System.Text.Json.JsonException)
        {
            return new ProviderCredentials(conn.Provider, Region: region);
        }

        return new ProviderCredentials(conn.Provider, sub, rg, region, extra);
    }
}
