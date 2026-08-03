using System.Text.Json;
using System.Text.Json.Nodes;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Background;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// The provisioning orchestrator — the C# port of the two halves of the Rust
/// dashboard's <c>provisioning::kick_provisioning</c>
/// (<c>crates/networker-dashboard/src/provisioning.rs</c>) and
/// <c>benchmark_worker</c> promote loop
/// (<c>crates/networker-dashboard/src/benchmark_worker.rs</c>), unified into one
/// ~5s-tick <see cref="BackgroundService"/>.
///
/// <para>M3's <see cref="Dispatch.RunDispatcher"/> deliberately leaves a run with a
/// <c>Pending</c> endpoint sitting in <c>queued</c> (it logs a deferral and does
/// NOT dispatch). This service closes that gap: it drives such runs through
/// their provisioning lifecycle and re-queues them once a VM is live, so the
/// dispatcher/redispatcher then assigns them to an agent normally.</para>
///
/// <para><b>Each tick, in a fresh DI scope:</b></para>
/// <list type="number">
///   <item><b>Kick</b> — find <c>queued</c> runs whose config endpoint is
///     <c>Pending</c> and which have no <c>provisioning_deployment_id</c> yet.
///     For each: resolve the provider from the cloud account, build the
///     <c>deploy.json</c>, insert a <c>deployment</c> row (status <c>pending</c>),
///     set <c>test_run.provisioning_deployment_id</c> + status
///     <c>provisioning</c>, and start the <see cref="DeployRunner"/> on a
///     detached background task (matching the Rust <c>tokio::spawn</c>).</item>
///   <item><b>Promote</b> — find <c>provisioning</c> runs whose deployment is
///     <c>completed</c>: rewrite the config's endpoint <c>Pending → Network{host,
///     port}</c> (host = first captured endpoint IP, port =
///     <c>proxy_https_port(proxy_stack)</c>) and flip the run back to
///     <c>queued</c>. If the deployment is <c>failed</c>, fail the run.</item>
/// </list>
///
/// <para><b>SHARED-CONFIG CAVEAT (matches Rust, flagged):</b> promote rewrites
/// the shared <c>test_config.endpoint_ref</c> row in place — exactly what the
/// Rust <c>promote()</c> does via <c>test_configs::update_endpoint</c>. If the
/// same <c>TestConfig</c> is reused by more than one run (e.g. a scheduled config
/// launched repeatedly), the last provision's host clobbers the template's
/// <c>Pending</c> endpoint for every future run. The scope doc's preferred design
/// is to store the resolved endpoint on the <b>run</b> (a per-run override
/// column) and leave the config template untouched. That needs a new column/DTO
/// path not present in this slice, so this port matches Rust's behaviour and
/// leaves the improvement as follow-up.</para>
/// </summary>
public sealed class ProvisioningOrchestrator : BackgroundService
{
    private static readonly TimeSpan TickInterval = TimeSpan.FromSeconds(5);
    private static readonly TimeSpan StartupDelay = TimeSpan.FromSeconds(2);

    // Wire status strings (Rust RunStatus / deployment.status are lowercase).
    private const string RunQueued = "queued";
    private const string RunProvisioning = "provisioning";
    private const string EndpointKindPending = "pending";
    private const string EndpointKindNetwork = "network";
    private const string DeploymentPending = "pending";
    private const string DeploymentCompleted = "completed";
    private const string DeploymentFailed = "failed";
    private const string DeploymentCancelled = "cancelled";

    /// <summary>Terminal state for a run-linked deployment whose cloud VM has
    /// been (or is being) torn down after its run finished. The row is KEPT —
    /// the deploy log is the primary diagnostic for failed provisions — but a
    /// torn-down deployment no longer counts toward the provisioning-capacity
    /// throttle.</summary>
    internal const string DeploymentTornDown = "torn_down";

    private const int KickBatchLimit = 25;

    /// <summary>Max auto-provision deployments in flight at once. Each holds a
    /// public IP, and Azure's default quota is 10 per region — a 10-cell
    /// comparison matrix plus the standing runner/target/control-plane VMs blew
    /// straight through it (PublicIPCountLimitReached, 2026-08-01). Queued runs
    /// past the cap stay queued and kick as slots free (each finished cell's
    /// teardown releases its IP). Overridable via
    /// NETWORKER_MAX_CONCURRENT_PROVISIONS for subscriptions with raised quota.</summary>
    internal static int MaxConcurrentAutoProvisions =
        int.TryParse(
            Environment.GetEnvironmentVariable("NETWORKER_MAX_CONCURRENT_PROVISIONS"),
            out var cap) && cap > 0
            ? cap
            : 6;

    // ── Readiness gate (E2E pass 2026-07-28 P1-3) ────────────────────────────
    // A freshly-provisioned VM's deployment flips to `completed` the moment
    // install.sh exits, but the proxy/endpoint services take a few more seconds
    // to bind their listen ports. Promoting immediately re-queued the run, which
    // the dispatcher then handed to an agent that hit a not-yet-listening port →
    // the whole run failed "connection refused" even though the target was
    // healthy seconds later. Gate the promote on a bounded TCP connect to
    // host:port: defer to the next tick until it answers, and fail terminally
    // only after a generous grace window so a genuinely-dead endpoint can't spin
    // the run in `provisioning` forever (the F3(d) permanent-condition lesson).
    private static readonly TimeSpan ReadinessProbeTimeout = TimeSpan.FromSeconds(3);
    internal static TimeSpan ReadinessGrace = TimeSpan.FromMinutes(6);

    private readonly IServiceScopeFactory _scopeFactory;
    private readonly DeployRunner _runner;
    private readonly ILogger<ProvisioningOrchestrator> _logger;
    private readonly PgAdvisoryLeaderLock? _leader;
    private readonly TickMonitor _monitor;

    public ProvisioningOrchestrator(
        IServiceScopeFactory scopeFactory,
        DeployRunner runner,
        ILogger<ProvisioningOrchestrator> logger,
        PgAdvisoryLeaderLock? leaderLock = null,
        TickMonitor? tickMonitor = null)
    {
        _scopeFactory = scopeFactory;
        _runner = runner;
        _logger = logger;
        // M6 ops infra (AddOpsInfrastructure); optional for bare test hosts.
        _leader = leaderLock;
        _monitor = tickMonitor ?? new TickMonitor();
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        try
        {
            await Task.Delay(StartupDelay, stoppingToken).ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            return;
        }

        _logger.LogInformation("Provisioning orchestrator started (tick every {Seconds}s)", TickInterval.TotalSeconds);
        _monitor.ReportStarted(OpsServiceNames.ProvisioningOrchestrator);

        using var timer = new PeriodicTimer(TickInterval);
        while (await timer.WaitForNextTickAsync(stoppingToken).ConfigureAwait(false))
        {
            try
            {
                var ranAsLeader = await _leader
                    .TryRunGuardedAsync(LeaderLockKeys.ProvisioningOrchestrator, TickAsync, stoppingToken)
                    .ConfigureAwait(false);
                if (!ranAsLeader)
                {
                    _logger.LogDebug("Provisioning orchestrator tick skipped — another replica holds the leader lock");
                }
            }
            catch (OperationCanceledException) when (stoppingToken.IsCancellationRequested)
            {
                break;
            }
            catch (Exception ex)
            {
                _monitor.ReportError(OpsServiceNames.ProvisioningOrchestrator, ex);
                _logger.LogError(ex, "Provisioning orchestrator tick failed");
            }
        }
    }

    private async Task TickAsync(CancellationToken ct)
    {
        using var scope = _scopeFactory.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();

        var kicked = await KickPendingRunsAsync(db, ct).ConfigureAwait(false);
        var resolved = await PromoteProvisioningRunsAsync(db, ct).ConfigureAwait(false);
        var torn = await TeardownFinishedRunsAsync(db, ct).ConfigureAwait(false);

        _monitor.ReportTick(
            OpsServiceNames.ProvisioningOrchestrator,
            kicked + resolved + torn,
            $"kicked={kicked} resolved={resolved} torn_down={torn}");
    }

    // ── Kick: queued + Pending + no deployment ⇒ start provisioning ──────────

    /// <returns>Number of runs whose provisioning was actually kicked off.</returns>
    private async Task<int> KickPendingRunsAsync(NetworkerDbContext db, CancellationToken ct)
    {
        // Concurrency throttle: what actually holds a public IP is a run-linked
        // deployment whose VM hasn't been torn down yet — a cell keeps its IP
        // through provisioning AND the run itself, until the teardown phase
        // releases it. Failed deployments COUNT TOO: a proxy-setup failure dies
        // AFTER the VM+IP exist, and excluding those rows let the rolling
        // window overshoot the quota (PublicIPCountLimitReached on the
        // 2026-08-03 relaunch — 2 failed-not-yet-swept cells + 6 in flight + 3
        // standing = 11 > 10). Only torn_down rows are known-released; the
        // teardown phase now processes failed/cancelled rows promptly.
        var active = await db.TestRuns
            .Where(r => r.ProvisioningDeploymentId != null)
            .Join(db.Deployments,
                r => r.ProvisioningDeploymentId, d => d.DeploymentId, (r, d) => d.Status)
            .CountAsync(s => s != DeploymentTornDown, ct)
            .ConfigureAwait(false);
        var capacity = MaxConcurrentAutoProvisions - active;
        if (capacity <= 0)
        {
            var waiting = await db.TestRuns
                .CountAsync(r => r.Status == RunQueued
                                 && r.ProvisioningDeploymentId == null
                                 && r.TestConfig.EndpointKind == EndpointKindPending, ct)
                .ConfigureAwait(false);
            if (waiting > 0)
            {
                _logger.LogInformation(
                    "Provisioning at capacity ({Active}/{Cap}) — {Waiting} queued run(s) waiting for a slot",
                    active, MaxConcurrentAutoProvisions, waiting);
            }
            return 0;
        }

        // queued runs, config endpoint is Pending, not yet linked to a deployment.
        var candidates = await db.TestRuns
            .Where(r => r.Status == RunQueued
                        && r.ProvisioningDeploymentId == null
                        && r.TestConfig.EndpointKind == EndpointKindPending)
            .OrderBy(r => r.CreatedAt)
            .Take(Math.Min(KickBatchLimit, capacity))
            .Select(r => new { Run = r, r.TestConfig })
            .ToListAsync(ct);

        var kicked = 0;
        foreach (var c in candidates)
        {
            ct.ThrowIfCancellationRequested();
            try
            {
                if (await KickOneAsync(db, c.Run, c.TestConfig, ct).ConfigureAwait(false))
                {
                    kicked++;
                }
            }
            catch (Exception ex)
            {
                _logger.LogError(ex, "Failed to kick provisioning for run {RunId}", c.Run.Id);
            }
        }

        return kicked;
    }

    // ── Teardown: terminal run + live deployment ⇒ release the cloud VM ──────

    /// <summary>Wait this long after a run finishes before tearing its endpoint
    /// down. Chained consumers re-target a just-finished cell's endpoint with a
    /// NEW config within seconds (the soak canary's phase 3 reuses phase 2's
    /// rust@nginx endpoint); an immediate teardown would race that hand-off.
    /// Static (not const) so tests can shrink it.</summary>
    internal static TimeSpan TeardownGrace = TimeSpan.FromMinutes(3);

    /// <summary>How long a FAILED/CANCELLED deployment keeps counting toward
    /// provisioning capacity before being marked torn down. Failed deploys
    /// usually have no registered hosts (registration happens at completion),
    /// so their VM+IP — if any survived the failure — are released by the
    /// orphan reaper's ~10-min sweep, not by a host-based teardown. Marking
    /// them torn_down immediately made the throttle undercount real IP usage.
    /// One reaper tick + margin.</summary>
    internal static TimeSpan FailedReleaseAllowance = TimeSpan.FromMinutes(12);

    /// <summary>
    /// Tear down the cloud VM of every auto-provisioned deployment whose run has
    /// reached a terminal state. Without this, every matrix cell's VM (and its
    /// public IP) lived until someone deleted it by hand — ten cells leaked ten
    /// B2s VMs per launch (2026-08-01), and the IP quota starved later launches.
    /// The deployment ROW is kept (status → <see cref="DeploymentTornDown"/>):
    /// its log is the primary diagnostic for failed provisions. The claim is an
    /// atomic status-guarded update so overlapping ticks can't double-spawn, and
    /// a deployment with no registered endpoint IPs (died before install
    /// registered them) is still marked torn down — its partial resources are
    /// the orphan reaper's job. Two defers protect endpoint reuse: the
    /// <see cref="TeardownGrace"/> window after the run finishes, and any
    /// non-terminal run whose config still points at one of the deployment's
    /// hosts (the canary's phase-3 pattern; also a user re-running a promoted
    /// cell config).
    /// </summary>
    private async Task<int> TeardownFinishedRunsAsync(NetworkerDbContext db, CancellationToken ct)
    {
        var graceCutoff = DateTime.UtcNow - TeardownGrace;
        var candidates = await db.TestRuns
            .Where(r => r.ProvisioningDeploymentId != null
                        && (r.Status == "completed" || r.Status == "failed" || r.Status == "cancelled")
                        && r.FinishedAt != null && r.FinishedAt < graceCutoff)
            .Join(db.Deployments,
                r => r.ProvisioningDeploymentId, d => d.DeploymentId,
                (r, d) => new { RunId = r.Id, RunFinishedAt = r.FinishedAt, Dep = d })
            // Failed/cancelled deployments are candidates too: a proxy-setup
            // failure leaves a full VM+IP behind (only creation-time failures
            // leave nothing), and those IPs must release promptly for the
            // quota throttle to be truthful. Hostless rows just get marked.
            .Where(x => x.Dep.Status != DeploymentTornDown)
            .Take(KickBatchLimit)
            .ToListAsync(ct)
            .ConfigureAwait(false);

        if (candidates.Count == 0)
        {
            return 0;
        }

        // Active runs' endpoint refs, fetched ONCE and matched client-side.
        // endpoint_ref is a JSONB column: a server-side Contains translates to
        // `LIKE` and Postgres has no `jsonb ~~ jsonb` operator — the original
        // per-host AnyAsync threw 42883 EVERY tick, which both killed teardown
        // and (with stale rows counting toward capacity) starved all kicks
        // (prod wedge, 2026-08-03). Active runs are bounded by the throttle +
        // queue, so the client-side scan is small.
        var activeRefs = await db.TestRuns
            .Where(r => r.Status != "completed" && r.Status != "failed" && r.Status != "cancelled")
            .Select(r => r.TestConfig.EndpointRef)
            .ToListAsync(ct)
            .ConfigureAwait(false);

        var torn = 0;
        foreach (var c in candidates)
        {
            ct.ThrowIfCancellationRequested();

            // Failed/cancelled deploys have no hosts to tear down — hold them
            // (counting toward capacity) until the reaper's sweep has had a
            // chance to release whatever the failure left behind.
            if ((c.Dep.Status == DeploymentFailed || c.Dep.Status == DeploymentCancelled)
                && c.RunFinishedAt > DateTime.UtcNow - FailedReleaseAllowance)
            {
                continue;
            }

            // Defer while ANY active run's config references one of this
            // deployment's hosts — its endpoint is being reused as a target.
            var candidateHosts = Endpoints.DeploymentWriteEndpoints.ParseHosts(c.Dep.EndpointIps);
            var referenced = candidateHosts.Any(h => activeRefs.Any(er =>
                er is not null && er.Contains(h, StringComparison.OrdinalIgnoreCase)));
            if (referenced)
            {
                continue; // re-checked next tick; tears down once the reuse ends
            }

            var priorStatus = c.Dep.Status;
            var claimed = await db.Deployments
                .Where(d => d.DeploymentId == c.Dep.DeploymentId && d.Status == priorStatus)
                .ExecuteUpdateAsync(s => s
                    .SetProperty(d => d.Status, DeploymentTornDown)
                    .SetProperty(d => d.FinishedAt, d => d.FinishedAt ?? DateTime.UtcNow), ct)
                .ConfigureAwait(false);
            if (claimed == 0)
            {
                continue; // another tick / an explicit API delete got here first
            }

            string? provider = null;
            string? region = null;
            if (c.Dep.CloudAccountId is { } accountId)
            {
                var acct = await db.CloudAccounts.AsNoTracking()
                    .Where(a => a.AccountId == accountId)
                    .Select(a => new { a.Provider, a.RegionDefault })
                    .FirstOrDefaultAsync(ct)
                    .ConfigureAwait(false);
                provider = acct?.Provider;
                region = acct?.RegionDefault;
            }
            provider ??= Endpoints.DeploymentWriteEndpoints.FirstProviderFromConfig(c.Dep.Config);

            if (!string.IsNullOrEmpty(provider) && candidateHosts.Count > 0)
            {
                Endpoints.DeploymentWriteEndpoints.SpawnVmTeardown(
                    _scopeFactory, _logger, c.Dep.DeploymentId, provider!, region, candidateHosts);
                _logger.LogInformation(
                    "Run {RunId} finished — tearing down deployment {DeploymentId} ({HostCount} endpoint(s))",
                    c.RunId, c.Dep.DeploymentId, candidateHosts.Count);
            }
            else
            {
                _logger.LogInformation(
                    "Run {RunId} finished — deployment {DeploymentId} marked torn_down (no registered endpoints; reaper will sweep partial resources)",
                    c.RunId, c.Dep.DeploymentId);
            }
            torn++;
        }

        return torn;
    }

    /// <returns><c>true</c> when this call kicked off the deployment.</returns>
    private async Task<bool> KickOneAsync(NetworkerDbContext db, TestRun run, TestConfig cfg, CancellationToken ct)
    {
        var pending = ParsePending(cfg.EndpointRef, _logger);
        if (pending is null)
        {
            _logger.LogWarning(
                "Run {RunId} config {ConfigId} is endpoint_kind=pending but endpoint_ref did not parse as Pending — skipping",
                run.Id, cfg.Id);
            return false;
        }

        // Resolve the concrete provider from the cloud account. install.sh has no
        // DB access, so `provider: "auto"` is never resolvable there — every
        // Pending deploy must carry the real provider. Mirrors kick_provisioning.
        var provider = await db.CloudAccounts
            .AsNoTracking()
            .Where(a => a.AccountId == pending.CloudAccountId)
            .Select(a => a.Provider)
            .FirstOrDefaultAsync(ct);
        if (string.IsNullOrEmpty(provider))
        {
            _logger.LogWarning(
                "Cloud account {AccountId} not found for run {RunId} — cannot provision",
                pending.CloudAccountId, run.Id);
            return false;
        }

        var deployJson = BuildDeployJson(pending, provider, cfg.Name, run.Id);
        var deployText = deployJson.ToJsonString();
        var providerSummary = BuildProviderSummary(deployJson);

        var deploymentId = Guid.NewGuid();
        var now = DateTime.UtcNow;
        db.Deployments.Add(new Deployment
        {
            DeploymentId = deploymentId,
            Name = $"auto-{cfg.Name}-{ShortId(run.Id)}",
            Status = DeploymentPending,
            Config = deployText,
            ProviderSummary = providerSummary,
            CreatedBy = cfg.CreatedBy,
            CreatedAt = now,
            ProjectId = cfg.ProjectId,
            CloudAccountId = pending.CloudAccountId,
        });

        // Link the run + flip to provisioning. Guard with a status/link check so
        // two overlapping ticks can't double-kick the same run (the ExecuteUpdate
        // is atomic; the second one affects 0 rows and we roll back the insert).
        await db.SaveChangesAsync(ct).ConfigureAwait(false);

        var linked = await db.TestRuns
            .Where(r => r.Id == run.Id && r.Status == RunQueued && r.ProvisioningDeploymentId == null)
            .ExecuteUpdateAsync(s => s
                .SetProperty(r => r.Status, RunProvisioning)
                .SetProperty(r => r.ProvisioningDeploymentId, deploymentId), ct)
            .ConfigureAwait(false);

        if (linked == 0)
        {
            // Lost the race — another tick already linked this run. Drop the
            // orphan deployment row we just created so it doesn't run twice.
            _logger.LogInformation("Run {RunId} was already linked to a deployment — discarding duplicate kick", run.Id);
            await db.Deployments
                .Where(d => d.DeploymentId == deploymentId)
                .ExecuteDeleteAsync(ct)
                .ConfigureAwait(false);
            return false;
        }

        _logger.LogInformation(
            "Provisioning kicked off for run {RunId} (config {ConfigName}, provider {Provider}, region {Region}, proxy {Proxy}) → deployment {DeploymentId}",
            run.Id, cfg.Name, provider, pending.Region, pending.ProxyStack, deploymentId);

        // Detached background deploy — matches the Rust tokio::spawn. Uses the
        // application lifetime (CancellationToken.None) so it isn't torn down by
        // the tick's scope; the runner opens its own DB scope.
        _ = Task.Run(async () =>
        {
            try
            {
                await _runner.RunDeploymentAsync(deploymentId, deployText, CancellationToken.None).ConfigureAwait(false);
            }
            catch (Exception ex)
            {
                _logger.LogError(ex, "Auto-provisioning deploy runner failed for deployment {DeploymentId}", deploymentId);
            }
        }, CancellationToken.None);

        return true;
    }

    // ── Promote: provisioning runs whose deployment finished ─────────────────

    /// <returns>Number of runs resolved this pass (promoted or failed).</returns>
    private async Task<int> PromoteProvisioningRunsAsync(NetworkerDbContext db, CancellationToken ct)
    {
        var pairs = await db.TestRuns
            .AsNoTracking()
            .Where(r => r.Status == RunProvisioning && r.ProvisioningDeploymentId != null)
            .Select(r => new { r.Id, r.TestConfigId, DeploymentId = r.ProvisioningDeploymentId!.Value })
            .ToListAsync(ct);

        var resolved = 0;
        foreach (var p in pairs)
        {
            ct.ThrowIfCancellationRequested();
            try
            {
                if (await HandleProvisioningRunAsync(db, p.Id, p.TestConfigId, p.DeploymentId, ct).ConfigureAwait(false))
                {
                    resolved++;
                }
            }
            catch (Exception ex)
            {
                _logger.LogError(ex,
                    "Orchestrator failed to handle provisioning run {RunId} (deployment {DeploymentId})",
                    p.Id, p.DeploymentId);
            }
        }

        return resolved;
    }

    /// <returns><c>true</c> when the run reached a resolution this pass
    /// (re-queued after promote, or failed); <c>false</c> when it is still
    /// waiting on its deployment.</returns>
    private async Task<bool> HandleProvisioningRunAsync(
        NetworkerDbContext db, Guid runId, Guid testConfigId, Guid deploymentId, CancellationToken ct)
    {
        var deployment = await db.Deployments
            .AsNoTracking()
            .FirstOrDefaultAsync(d => d.DeploymentId == deploymentId, ct);
        if (deployment is null)
        {
            _logger.LogWarning("Deployment {DeploymentId} vanished for provisioning run {RunId}", deploymentId, runId);
            return false;
        }

        switch (deployment.Status)
        {
            case DeploymentCompleted:
                await PromoteAsync(db, runId, testConfigId, deployment, ct).ConfigureAwait(false);
                return true;

            case DeploymentFailed:
                var msg = deployment.ErrorMessage ?? "deployment failed";
                await db.TestRuns
                    .Where(r => r.Id == runId)
                    .ExecuteUpdateAsync(s => s
                        .SetProperty(r => r.Status, "failed")
                        .SetProperty(r => r.ErrorMessage, $"Provisioning failed: {msg}")
                        .SetProperty(r => r.FinishedAt, DateTime.UtcNow), ct)
                    .ConfigureAwait(false);
                _logger.LogWarning(
                    "Run {RunId} failed: provisioning deployment {DeploymentId} failed ({Error})",
                    runId, deploymentId, msg);
                return true;

            case DeploymentCancelled:
                // A cancelled deployment is terminal — its run can never make
                // progress, so fail it instead of leaving it in `provisioning`
                // forever (quality audit F3(a)). Mirror the DeploymentFailed arm.
                await db.TestRuns
                    .Where(r => r.Id == runId)
                    .ExecuteUpdateAsync(s => s
                        .SetProperty(r => r.Status, "failed")
                        .SetProperty(r => r.ErrorMessage, "Provisioning cancelled")
                        .SetProperty(r => r.FinishedAt, DateTime.UtcNow), ct)
                    .ConfigureAwait(false);
                _logger.LogWarning(
                    "Run {RunId} failed: provisioning deployment {DeploymentId} was cancelled",
                    runId, deploymentId);
                return true;

            // pending / running — leave alone; re-check next tick.
            default:
                return false;
        }
    }

    /// <summary>Deployment completed: rewrite the config endpoint Pending→Network
    /// and re-queue the run for the dispatcher. Mirrors Rust <c>promote()</c>.</summary>
    private async Task PromoteAsync(
        NetworkerDbContext db, Guid runId, Guid testConfigId, Deployment deployment, CancellationToken ct)
    {
        var cfg = await db.TestConfigs.FirstOrDefaultAsync(c => c.Id == testConfigId, ct);
        if (cfg is null)
        {
            _logger.LogWarning("test_config {ConfigId} vanished while promoting run {RunId}", testConfigId, runId);
            return;
        }

        var pending = ParsePending(cfg.EndpointRef, _logger);
        if (pending is null)
        {
            // Already rewritten to Network{host,port} by an earlier tick (shared
            // config, or a prior promote). Still gate readiness on that endpoint
            // before re-queue — otherwise a shared config's second run skips the
            // gate entirely and races the services (E2E P1-3). If the ref isn't a
            // parseable Network endpoint, fall through to the old immediate
            // re-queue (behaviour-preserving).
            if (TryParseNetworkHostPort(cfg.EndpointRef, out var nhost, out var nport))
            {
                switch (await GateReadinessAsync(db, runId, deployment, nhost, nport, ct).ConfigureAwait(false))
                {
                    case ReadinessOutcome.Deferred:
                    case ReadinessOutcome.FailedTerminally:
                        return; // stay in provisioning / already failed
                    case ReadinessOutcome.Ready:
                        break;
                }
            }
            await db.TestRuns
                .Where(r => r.Id == runId)
                .ExecuteUpdateAsync(s => s
                    .SetProperty(r => r.Status, RunQueued)
                    // Claimability stamp: the watchdog's queued-age basis is
                    // COALESCE(last_heartbeat, created_at). Without this a run
                    // whose provisioning took >5min re-queues already past the
                    // no-claim cutoff and is reaped before any agent can claim
                    // it (2026-08-03 apache cell).
                    .SetProperty(r => r.LastHeartbeat, DateTime.UtcNow), ct)
                .ConfigureAwait(false);
            return;
        }

        var host = FirstEndpointHost(deployment.EndpointIps);
        if (host is null)
        {
            // A completed deployment with no captured endpoint IPs is a PERMANENT
            // condition — retrying every tick never produces a host, it only spins
            // the run in `provisioning` and spams the log forever (quality audit
            // F3(d)). Fail the run terminally instead.
            await db.TestRuns
                .Where(r => r.Id == runId)
                .ExecuteUpdateAsync(s => s
                    .SetProperty(r => r.Status, "failed")
                    .SetProperty(r => r.ErrorMessage, "Provisioning completed but captured no endpoint IPs")
                    .SetProperty(r => r.FinishedAt, DateTime.UtcNow), ct)
                .ConfigureAwait(false);
            _logger.LogWarning(
                "Deployment {DeploymentId} completed but captured no endpoint IPs — failing run {RunId} (permanent)",
                deployment.DeploymentId, runId);
            return;
        }

        var port = ProxyHttpsPort(pending.ProxyStack);

        // Readiness gate (E2E P1-3): don't rewrite the endpoint + re-queue until
        // the proxy/endpoint port actually answers. Defer to the next tick while
        // the just-provisioned services finish binding; fail only past the grace
        // window. Gating BEFORE the rewrite keeps ParsePending valid on the next
        // deferred tick (the pending endpoint is untouched until we know it's up).
        switch (await GateReadinessAsync(db, runId, deployment, host, port, ct).ConfigureAwait(false))
        {
            case ReadinessOutcome.Deferred:
            case ReadinessOutcome.FailedTerminally:
                return;
            case ReadinessOutcome.Ready:
                break;
        }

        // Rewrite endpoint_ref → Network{host,port} + endpoint_kind → network.
        // SHARED-CONFIG CAVEAT (see class doc): this mutates the shared template.
        var newEndpoint = new JsonObject
        {
            ["kind"] = EndpointKindNetwork,
            ["host"] = host,
            ["port"] = port,
        };
        cfg.EndpointRef = newEndpoint.ToJsonString();
        cfg.EndpointKind = EndpointKindNetwork;
        // E2E pass 2026-07-28 P1-14 (second half): a provisioned proxy-stack target
        // serves a SELF-SIGNED cert (CN=localhost, no SAN — install.sh generates it
        // per-VM). The dispatcher only injects workload.insecure for endpoint_kind
        // == "proxy"; once we rewrite the endpoint to `network` here that branch no
        // longer fires, so the tester would VALIDATE the self-signed cert and fail
        // every attempt at the TLS stage (confirmed live: TCP ok, no TLS phase,
        // 50/50 fail even with the CA:FALSE cert fix). Persist insecure=true on the
        // promoted config so the tester skips validation — provisioned targets are
        // self-signed by construction, so this is always correct here.
        cfg.Workload = WithInsecureWorkload(cfg.Workload);
        cfg.UpdatedAt = DateTime.UtcNow;
        await db.SaveChangesAsync(ct).ConfigureAwait(false);

        await db.TestRuns
            .Where(r => r.Id == runId)
            .ExecuteUpdateAsync(s => s
                .SetProperty(r => r.Status, RunQueued)
                // Claimability stamp — see the readiness-gate re-queue above.
                .SetProperty(r => r.LastHeartbeat, DateTime.UtcNow), ct)
            .ConfigureAwait(false);

        _logger.LogInformation(
            "Provisioning complete for run {RunId}: endpoint rewritten to {Host}:{Port} (proxy {Proxy}, insecure), run re-queued",
            runId, host, port, pending.ProxyStack);
    }

    /// <summary>
    /// Return the workload JSON with <c>insecure: true</c> set (P1-14 second half).
    /// Mirrors <c>RunDispatcher.WithInsecure</c>, but PERSISTED on the promoted
    /// config (the dispatcher applies it copy-on-write only for <c>proxy</c>-kind
    /// endpoints, which no longer fires once we've rewritten to <c>network</c>).
    /// A non-object / unparseable workload is returned unchanged (defensive; the
    /// tester then just validates as before rather than crashing the promote).
    /// </summary>
    internal static string WithInsecureWorkload(string workloadText)
    {
        try
        {
            if (JsonNode.Parse(workloadText) is JsonObject obj)
            {
                obj["insecure"] = true;
                return obj.ToJsonString();
            }
        }
        catch (JsonException)
        {
            // leave the workload untouched — better than failing the promote
        }
        return workloadText;
    }

    private enum ReadinessOutcome { Ready, Deferred, FailedTerminally }

    /// <summary>
    /// Bounded readiness gate for a promoted endpoint (E2E P1-3). TCP-connects to
    /// <paramref name="host"/>:<paramref name="port"/> and reports:
    /// <list type="bullet">
    ///   <item><b>Ready</b> — the port answered; the caller proceeds to re-queue.</item>
    ///   <item><b>Deferred</b> — not listening yet but still inside the grace
    ///     window; the caller leaves the run in <c>provisioning</c> for the next
    ///     tick (no state change).</item>
    ///   <item><b>FailedTerminally</b> — still unreachable past the grace window;
    ///     the run is failed here (permanent condition, mirroring the no-host arm
    ///     so a dead endpoint can't spin forever — F3(d)).</item>
    /// </list>
    /// Grace is measured from the deployment's completion (<c>FinishedAt</c>), so a
    /// deploy that finished long ago fails fast while a just-completed one gets the
    /// full window. The probe runs from the control plane — the same vantage the
    /// existing <c>/check</c> endpoint uses — and the endpoint's proxy ports are
    /// publicly reachable (that's how the agent reaches them too).
    /// </summary>
    private async Task<ReadinessOutcome> GateReadinessAsync(
        NetworkerDbContext db, Guid runId, Deployment deployment, string host, int port, CancellationToken ct)
    {
        if (await TcpReadyAsync(host, port, ReadinessProbeTimeout, ct).ConfigureAwait(false))
        {
            return ReadinessOutcome.Ready;
        }

        var since = deployment.FinishedAt ?? deployment.StartedAt ?? DateTime.UtcNow;
        if (DateTime.UtcNow - since <= ReadinessGrace)
        {
            _logger.LogInformation(
                "Run {RunId}: endpoint {Host}:{Port} not listening yet; deferring re-queue (within {Grace:0}m grace)",
                runId, host, port, ReadinessGrace.TotalMinutes);
            return ReadinessOutcome.Deferred;
        }

        await db.TestRuns
            .Where(r => r.Id == runId)
            .ExecuteUpdateAsync(s => s
                .SetProperty(r => r.Status, "failed")
                .SetProperty(r => r.ErrorMessage,
                    $"Provisioned endpoint {host}:{port} never became reachable within {ReadinessGrace.TotalMinutes:0}m")
                .SetProperty(r => r.FinishedAt, DateTime.UtcNow), ct)
            .ConfigureAwait(false);
        _logger.LogWarning(
            "Run {RunId}: endpoint {Host}:{Port} unreachable past {Grace:0}m grace — failing run (permanent)",
            runId, host, port, ReadinessGrace.TotalMinutes);
        return ReadinessOutcome.FailedTerminally;
    }

    private static async Task<bool> TcpReadyAsync(string host, int port, TimeSpan timeout, CancellationToken ct)
    {
        try
        {
            using var client = new System.Net.Sockets.TcpClient();
            using var cts = CancellationTokenSource.CreateLinkedTokenSource(ct);
            cts.CancelAfter(timeout);
            await client.ConnectAsync(host, port, cts.Token).ConfigureAwait(false);
            return client.Connected;
        }
        catch
        {
            // Refused / timed out / DNS / cancelled → not ready. Never throws:
            // the gate treats every failure as "not listening yet".
            return false;
        }
    }

    /// <summary>Read host+port out of an already-rewritten Network
    /// <c>endpoint_ref</c> (the shared-config promote path). Returns false for a
    /// non-Network or malformed ref, in which case the caller re-queues without
    /// gating (behaviour-preserving fallback).</summary>
    internal static bool TryParseNetworkHostPort(string endpointRef, out string host, out int port)
    {
        host = string.Empty;
        port = 0;
        try
        {
            using var doc = JsonDocument.Parse(endpointRef);
            var root = doc.RootElement;
            if (root.ValueKind != JsonValueKind.Object)
            {
                return false;
            }
            if (root.TryGetProperty("host", out var h) && h.ValueKind == JsonValueKind.String)
            {
                host = h.GetString()?.Trim() ?? string.Empty;
            }
            if (root.TryGetProperty("port", out var p)
                && p.ValueKind == JsonValueKind.Number && p.TryGetInt32(out var pv))
            {
                port = pv;
            }
            return !string.IsNullOrEmpty(host) && port > 0;
        }
        catch (JsonException)
        {
            return false;
        }
    }

    // ── deploy.json builder (port of build_deploy_json) ──────────────────────

    /// <summary>Build the deploy.json document handed to <c>install.sh --deploy</c>.
    /// Byte-shape-compatible with the Rust <c>build_deploy_json</c>: a
    /// per-provider endpoint block, <c>tester:{provider:"local"}</c>,
    /// <c>tests:{run_tests:false}</c>, <c>version:1</c>, and the concrete
    /// <c>cloud_account_id</c>.
    /// The VM label is derived from the RUN id, never the config name: all
    /// cells of a comparison group share a config-name prefix ("Azure/eastus
    /// …"), so a name-prefix label made every cell race to create the SAME
    /// Azure VM — nine lost with Conflict, the winner's endpoint was stomped
    /// by the other cells' installers (10-cell matrix, 2026-07-31). The run-id
    /// suffix matches the deployment row's name suffix for correlation.</summary>
    internal static JsonObject BuildDeployJson(PendingEndpoint p, string provider, string cfgName, Guid runId)
    {
        var vmLabel = SanitizeVmLabel($"nwk-a-{ShortId(runId)}");

        JsonObject providerBlock = provider switch
        {
            "aws" => new JsonObject
            {
                ["region"] = p.Region,
                ["instance_type"] = p.VmSize,
                ["os"] = p.Os,
                ["instance_name"] = vmLabel,
            },
            "gcp" => new JsonObject
            {
                ["region"] = p.Region,
                ["zone"] = $"{p.Region}-a",
                ["machine_type"] = p.VmSize,
                ["os"] = p.Os,
                ["instance_name"] = vmLabel,
            },
            // azure + default
            _ => new JsonObject
            {
                ["region"] = p.Region,
                ["vm_size"] = p.VmSize,
                ["os"] = p.Os,
                ["vm_name"] = vmLabel,
            },
        };

        var endpoint = new JsonObject
        {
            ["provider"] = provider,
            ["label"] = cfgName,
            ["http_stacks"] = new JsonArray(p.ProxyStack),
            [provider] = providerBlock,
        };
        if (!string.IsNullOrEmpty(p.Language))
        {
            endpoint["languages"] = new JsonArray(p.Language);
        }

        return new JsonObject
        {
            ["version"] = 1,
            ["tester"] = new JsonObject { ["provider"] = "local" },
            ["cloud_account_id"] = p.CloudAccountId.ToString(),
            ["endpoints"] = new JsonArray(endpoint),
            ["tests"] = new JsonObject { ["run_tests"] = false },
        };
    }

    /// <summary>Human-readable "provider region + ..." summary, mirroring the Rust
    /// <c>build_provider_summary</c> used on the deployment row.</summary>
    internal static string? BuildProviderSummary(JsonObject deployJson)
    {
        if (deployJson["endpoints"] is not JsonArray endpoints || endpoints.Count == 0)
        {
            return null;
        }

        var parts = new List<string>();
        foreach (var ep in endpoints)
        {
            var provider = ep?["provider"]?.GetValue<string>() ?? "unknown";
            var region = ep?["region"]?.GetValue<string?>();
            // region lives inside the per-provider block, not at endpoint top-level;
            // try both so the summary matches whatever shape is present.
            if (string.IsNullOrEmpty(region) && ep?[provider] is JsonObject block)
            {
                region = block["region"]?.GetValue<string?>();
            }
            parts.Add(string.IsNullOrEmpty(region) ? provider : $"{provider} {region}");
        }

        return parts.Count == 0 ? null : string.Join(" + ", parts);
    }

    // ── EndpointRef (Pending) parsing ────────────────────────────────────────

    /// <summary>Parse a JSONB <c>endpoint_ref</c> text column into a
    /// <see cref="PendingEndpoint"/>, or null if it isn't a Pending endpoint.
    /// The tagged-union shape is <c>{"kind":"pending", cloud_account_id, region,
    /// vm_size, os, proxy_stack, topology, language?}</c>. A malformed ref
    /// (undecodable JSON / missing <c>cloud_account_id</c>) also yields null,
    /// but is WARN-logged when a logger is supplied — callers otherwise can't
    /// distinguish "not a pending endpoint" from "corrupt pending endpoint".</summary>
    internal static PendingEndpoint? ParsePending(string endpointRef, ILogger? logger = null)
    {
        try
        {
            using var doc = JsonDocument.Parse(endpointRef);
            var root = doc.RootElement;
            if (root.ValueKind != JsonValueKind.Object
                || !root.TryGetProperty("kind", out var kind)
                || kind.GetString() != EndpointKindPending)
            {
                return null;
            }

            var cloudAccountId = root.GetProperty("cloud_account_id").GetGuid();
            var region = root.TryGetProperty("region", out var r) ? r.GetString() ?? "" : "";
            var vmSize = root.TryGetProperty("vm_size", out var v) ? v.GetString() ?? "" : "";
            var os = root.TryGetProperty("os", out var o) ? o.GetString() ?? "" : "";
            var proxyStack = root.TryGetProperty("proxy_stack", out var ps) ? ps.GetString() ?? "nginx" : "nginx";
            var language = root.TryGetProperty("language", out var l) && l.ValueKind == JsonValueKind.String
                ? l.GetString()
                : null;

            return new PendingEndpoint(cloudAccountId, region, vmSize, os, proxyStack, language);
        }
        catch (Exception ex)
        {
            // Behavior preserved (null → caller skips the run), but no longer a
            // silent swallow: a pending-kind ref that fails to parse means a
            // stuck run, so leave a trace of WHY.
            logger?.LogWarning(
                ex,
                "endpoint_ref failed to parse as a Pending endpoint ({Length} chars) — treating as non-pending",
                endpointRef.Length);
            return null;
        }
    }

    /// <summary>First usable host (FQDN preferred, bare IP otherwise) from the
    /// deployment's JSON <c>endpoint_ips</c> array text. Mirrors Rust
    /// <c>first_endpoint_host</c>.</summary>
    internal static string? FirstEndpointHost(string? endpointIps)
    {
        if (string.IsNullOrWhiteSpace(endpointIps))
        {
            return null;
        }

        try
        {
            using var doc = JsonDocument.Parse(endpointIps);
            if (doc.RootElement.ValueKind != JsonValueKind.Array)
            {
                return null;
            }
            foreach (var el in doc.RootElement.EnumerateArray())
            {
                if (el.ValueKind == JsonValueKind.String)
                {
                    var s = el.GetString()?.Trim();
                    if (!string.IsNullOrEmpty(s))
                    {
                        return s;
                    }
                }
            }
        }
        catch (JsonException)
        {
            return null;
        }
        return null;
    }

    // ── Proxy port + label helpers (port of test_config.rs) ───────────────────

    /// <summary>HTTPS listener port for a proxy stack after a standard deploy.
    /// Ported from <c>networker_common::test_config::proxy_https_port</c>, with
    /// one correction: the legacy constant said IIS serves 443, but the actual
    /// Windows endpoint deploy (<c>_iis_setup_powershell</c>) binds HTTPS on
    /// <b>8445</b> — the readiness gate probed 443 forever and every IIS matrix
    /// cell failed "never became reachable" (2026-08-03). 8445 also sits inside
    /// the NSG/SG 8443-8445 openings; 443 was never opened in-guest either.</summary>
    internal static int ProxyHttpsPort(string stack) => stack switch
    {
        "nginx" => 8444,
        "caddy" => 8454,
        "traefik" => 8455,
        "haproxy" => 8456,
        "apache" => 8457,
        "iis" => 8445,
        _ => 443,
    };

    private static string ShortId(Guid id) => id.ToString("N")[..8];

    // Windows NetBIOS constraint (install.sh's strictest): ≤15 chars, alnum + '-'.
    // Azure VM names must start with a word char and — critically — Azure derives
    // the NIC ipconfig name as `ipconfig<vmName>`, which must END with a word char
    // or '_'. A trailing '-' fails VM creation with InvalidResourceName (E2E
    // 2026-07-29: the then config-name-derived label "nwk-auto-rust--ng" was
    // truncated to "nwk-auto-rust--" → ipconfig ended in '-' → whole deploy
    // failed with install.sh exit 1). So: collapse dash-runs, forbid a leading
    // dash, cap at 15, then trim any trailing dash left by the source or the
    // truncation. Labels are now run-id-derived, but this stays the last line
    // of defense for any raw input.
    internal static string SanitizeVmLabel(string raw)
    {
        var sb = new System.Text.StringBuilder(16);
        var lastDash = false;
        foreach (var c in raw)
        {
            if (char.IsAsciiLetterOrDigit(c))
            {
                sb.Append(char.ToLowerInvariant(c));
                lastDash = false;
            }
            else if (c == '-' && sb.Length > 0 && !lastDash)
            {
                sb.Append('-'); // collapse runs; never a leading dash
                lastDash = true;
            }
            if (sb.Length >= 15)
            {
                break;
            }
        }
        var label = sb.ToString().TrimEnd('-');
        return label.Length == 0 ? "nwk-auto-vm" : label;
    }

    /// <summary>Parsed <c>EndpointRef::Pending</c> payload.</summary>
    internal sealed record PendingEndpoint(
        Guid CloudAccountId,
        string Region,
        string VmSize,
        string Os,
        string ProxyStack,
        string? Language);
}
