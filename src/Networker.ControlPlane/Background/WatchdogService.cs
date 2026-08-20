using System.Globalization;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Provisioning;
using Networker.ControlPlane.Realtime;
using Networker.Data;

namespace Networker.ControlPlane.Background;

/// <summary>
/// Stale-job watchdog — the C# re-architecture of the Rust
/// <c>reap_stale_assigned_jobs</c> sub-routine in
/// <c>crates/networker-dashboard/src/scheduler.rs</c>. Runs on a fixed ~60s
/// cadence and fails runs the system can no longer make progress on:
///
/// <list type="number">
///   <item><b>Stale <c>running</c> runs</b> — only runs whose
///     <c>last_heartbeat</c> (fallback <c>started_at</c> when heartbeat is null)
///     is older than <see cref="RunningStaleCutoff"/> (the Rust
///     <c>find_stale_assigned(client, 120)</c> query) AND whose executing agent
///     (parsed from <c>worker_id</c> — the FK-free string that records the
///     agent; <c>tester_id</c> is a project_tester FK, not an agent id) is not in
///     the live <see cref="AgentConnectionRegistry"/>. Hub/registry membership is
///     the authoritative "truly online" signal (identical to the Rust
///     <c>state.agents.is_agent_online</c> guard): if the socket is live the run
///     may just be slow to heartbeat, so it is left alone. A run is NEVER reaped
///     merely for having a null/unparseable <c>worker_id</c> — it must first fail the 120s
///     staleness precondition (a fresh heartbeat or a start under 120s ago keeps
///     it alive). Reaped runs are failed with the Rust user-facing guidance
///     <c>"Agent disconnected — tester may have been deleted or restarted"</c>.</item>
///   <item><b>Stale <c>queued</c> runs</b> — runs still <c>queued</c> whose
///     <c>created_at</c> is older than <see cref="QueuedCutoff"/> (the Rust
///     <c>QUEUED_CUTOFF_SECS = 300</c>, 5 minutes). No runner ever claimed them.
///     Runs whose config <c>endpoint_kind = 'pending'</c> are excluded — they
///     wait for the provisioning orchestrator, not an agent (runs already in
///     status <c>provisioning</c> are outside the query by construction).</item>
/// </list>
///
/// <para>Every failed run publishes a <c>JobUpdate(status: "failed")</c> on the
/// <see cref="EventBus"/> — the C# analogue of the Rust
/// <c>DashboardEvent::JobUpdate</c> the reaper sends for wire compatibility.</para>
///
/// <para><b>Scope discipline:</b> <c>NetworkerDbContext</c> is registered
/// <i>scoped</i>, so a long-lived <see cref="BackgroundService"/> cannot inject
/// it directly. Each tick opens a fresh DI scope via
/// <see cref="IServiceScopeFactory"/> and resolves the context from it — the
/// standard pattern for consuming a scoped service from a singleton hosted
/// service. The singleton <see cref="AgentConnectionRegistry"/> and
/// <see cref="EventBus"/> are injected directly (safe from a singleton).</para>
/// </summary>
public sealed class WatchdogService : BackgroundService
{
    /// <summary>How often the watchdog reconciles. Matches the Rust 60s cadence.</summary>
    private static readonly TimeSpan TickInterval = TimeSpan.FromSeconds(60);

    /// <summary>
    /// How long a run may sit in <c>queued</c> before it is failed. Mirrors the
    /// Rust <c>QUEUED_CUTOFF_SECS = 300</c> (5 minutes).
    /// </summary>
    private static readonly TimeSpan QueuedCutoff = TimeSpan.FromSeconds(300);

    /// <summary>
    /// How stale a <c>running</c> run's heartbeat (fallback: start) must be
    /// before it is even CONSIDERED for reaping. Mirrors the Rust
    /// <c>find_stale_assigned(client, 120)</c> cutoff.
    /// </summary>
    private static readonly TimeSpan RunningStaleCutoff = TimeSpan.FromSeconds(120);

    /// <summary>
    /// How long a <c>running</c> run may make NO progress (no streamed attempt,
    /// no heartbeat) before it is failed even though its agent is online.
    /// Override with <c>DASHBOARD_RUN_NO_PROGRESS_SECS</c> (clamped 120s…6h; 0
    /// disables the sweep for operators who prefer the old behaviour).
    /// </summary>
    private static readonly TimeSpan NoProgressCutoff = ResolveNoProgressCutoff(
        Environment.GetEnvironmentVariable("DASHBOARD_RUN_NO_PROGRESS_SECS"));

    /// <summary>Parse + clamp the no-progress cutoff (testable).</summary>
    internal static TimeSpan ResolveNoProgressCutoff(string? raw)
    {
        if (raw is not null && int.TryParse(raw, NumberStyles.Integer, CultureInfo.InvariantCulture, out var secs))
        {
            if (secs <= 0)
            {
                return TimeSpan.MaxValue;   // disabled
            }
            return TimeSpan.FromSeconds(Math.Clamp(secs, 120, 6 * 60 * 60));
        }
        return TimeSpan.FromMinutes(15);
    }

    /// <summary>
    /// Slack the stale-deploy sweep grants on top of the deployment's own
    /// scaled <see cref="DeployRunner.DeployTimeoutFor"/> budget, so the deploy
    /// runner's in-process timeout always fires FIRST (it tree-kills install.sh
    /// and writes the richer "install.sh timed out after Nm" message). The
    /// watchdog only wins when no runner is driving the deployment at all — a
    /// control-plane crash orphaned it (quality audit F3(b)). Issue #804: the
    /// old flat 30m cutoff undercut the scaled budget (#740) and killed a
    /// legitimate 38m cpp deploy at 30 minutes.
    /// </summary>
    internal static readonly TimeSpan DeploymentBudgetSlack = TimeSpan.FromMinutes(5);

    /// <summary>The stale-deploy sweep's reap threshold for one deployment:
    /// the SAME scaled budget the deploy runner enforces (single source of
    /// truth — <see cref="DeployRunner.DeployTimeoutFor"/>) plus
    /// <see cref="DeploymentBudgetSlack"/>.</summary>
    internal static TimeSpan DeploymentReapCutoffFor(string deployJson)
        => DeployRunner.DeployTimeoutFor(deployJson) + DeploymentBudgetSlack;

    /// <summary>User-facing message for a reaped deployment whose install NEVER
    /// started: the row is still <c>pending</c> — <see cref="DeployRunner"/>
    /// flips to <c>running</c> immediately before spawning install.sh, so a
    /// budget-aged pending row means the deploy driver died before any install
    /// ran (control-plane crash between kick and spawn, or a recovery claim
    /// whose re-run never launched). Built on the classifier's
    /// <see cref="ProvisioningFailureClassifier.NeverStartedReapPrefix"/> so
    /// the orchestrator's retry arm re-queues the linked run instead of failing
    /// it terminally (issue #817) — nothing was attempted, so nothing can have
    /// failed permanently.</summary>
    internal static string NeverStartedReapedErrorFor(string deployJson)
    {
        var budget = DeployRunner.DeployTimeoutFor(deployJson);
        return ProvisioningFailureClassifier.NeverStartedReapPrefix
            + string.Format(
                CultureInfo.InvariantCulture,
                " — it sat pending past its {0:F0}m budget with no deploy driver (control-plane crash or lost driver); the linked run is retried automatically",
                budget.TotalMinutes);
    }

    /// <summary>User-facing message for a reaped stale deployment: states the
    /// budget that was enforced, and blames a control-plane restart only when a
    /// recovery re-run actually happened (recovery_attempts &gt; 0, V052) —
    /// the old message claimed "the control plane may have restarted" even for
    /// plain flat-timeout kills (issue #804).</summary>
    internal static string DeploymentReapedErrorFor(string deployJson, bool recoveredFromRestart)
    {
        var budget = DeployRunner.DeployTimeoutFor(deployJson);
        var languages = DeployRunner.LanguageCountFor(deployJson);
        var langNote = languages switch
        {
            0 => "stack-only",
            1 => "1 language",
            _ => $"{languages} languages",
        };
        var msg = string.Format(
            CultureInfo.InvariantCulture,
            "Deployment did not finish within its {0:F0}m budget ({1})",
            budget.TotalMinutes, langNote);
        return recoveredFromRestart
            ? msg + " — a control-plane restart interrupted it mid-deploy and the recovery re-run also ran out of time"
            : msg;
    }

    /// <summary>
    /// How long a run may sit in <c>provisioning</c> with its deployment row
    /// gone/missing before it is failed. Deliberately FLAT (unlike the
    /// deployment sweep above): this arm only fires when the deployment row no
    /// longer exists, so there is no config to scale a budget from and no live
    /// install to protect — 30m is pure grace. A run whose deployment row still
    /// exists is never touched here (a #785 recovery re-uses the SAME
    /// deployment row, so recovered deploys keep their run out of this arm by
    /// construction).
    /// </summary>
    private static readonly TimeSpan ProvisioningOrphanCutoff = TimeSpan.FromMinutes(30);

    /// <summary>User-facing message for a provisioning run whose deployment is gone.</summary>
    private const string ProvisioningOrphanError =
        "Provisioning stalled — the deployment was lost or never finished; no VM was provisioned";

    /// <summary>Rust reaper's user-facing message for a dead running run.</summary>
    private const string RunningReapedError =
        "Agent disconnected — tester may have been deleted or restarted";

    /// <summary>Rust reaper's user-facing message for a never-claimed queued run.</summary>
    private const string QueuedReapedError =
        "No runner claimed this job within 5 minutes — check that at least one agent is online for this workspace";

    private readonly IServiceScopeFactory _scopeFactory;
    private readonly AgentConnectionRegistry _registry;
    private readonly EventBus _events;
    private readonly ILogger<WatchdogService> _logger;
    private readonly PgAdvisoryLeaderLock? _leader;
    private readonly TickMonitor _monitor;

    public WatchdogService(
        IServiceScopeFactory scopeFactory,
        AgentConnectionRegistry registry,
        EventBus events,
        ILogger<WatchdogService> logger,
        PgAdvisoryLeaderLock? leaderLock = null,
        TickMonitor? tickMonitor = null)
    {
        _scopeFactory = scopeFactory;
        _registry = registry;
        _events = events;
        _logger = logger;
        // M6 ops infra (AddOpsInfrastructure); optional for bare test hosts.
        _leader = leaderLock;
        _monitor = tickMonitor ?? new TickMonitor();
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        _logger.LogInformation("Stale-job watchdog started (tick={Tick}s)", TickInterval.TotalSeconds);
        _monitor.ReportStarted(OpsServiceNames.Watchdog);

        // PeriodicTimer's steady cadence is the C# analogue of the Rust
        // tokio::time::interval loop. A slow tick simply delays the next one; we
        // never fire a burst to "catch up".
        using var timer = new PeriodicTimer(TickInterval);
        while (await timer.WaitForNextTickAsync(stoppingToken).ConfigureAwait(false))
        {
            try
            {
                var ranAsLeader = await _leader
                    .TryRunGuardedAsync(LeaderLockKeys.Watchdog, TickAsync, stoppingToken)
                    .ConfigureAwait(false);
                if (!ranAsLeader)
                {
                    _logger.LogDebug("Stale-job watchdog tick skipped — another replica holds the leader lock");
                }
            }
            catch (OperationCanceledException) when (stoppingToken.IsCancellationRequested)
            {
                break;
            }
            catch (Exception ex)
            {
                // A single failed tick must never kill the loop — log and retry
                // next interval (matches the Rust `tracing::error!` + continue).
                _monitor.ReportError(OpsServiceNames.Watchdog, ex);
                _logger.LogError(ex, "Stale-job watchdog tick failed");
            }
        }
    }

    private async Task TickAsync(CancellationToken ct)
    {
        using var scope = _scopeFactory.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();

        var now = DateTime.UtcNow;
        var eventNow = DateTimeOffset.UtcNow;

        // ── Stale `running` runs ────────────────────────────────────────────
        // The Rust find_stale_assigned(client, 120) preconditions, verbatim:
        //   WHERE status = 'running' AND (
        //     (last_heartbeat IS NOT NULL AND last_heartbeat < now - 120s)
        //     OR (last_heartbeat IS NULL AND started_at IS NOT NULL
        //         AND started_at < now - 120s))
        // A run with a fresh heartbeat, or one that started under 120s ago, is
        // never even a candidate — regardless of its worker_id. Registry
        // membership (in-memory, authoritative) is then checked per-row below —
        // never expressible in SQL.
        var runningStaleBefore = now - RunningStaleCutoff;
        var running = await db.TestRuns
            .Where(r => r.Status == "running" &&
                ((r.LastHeartbeat != null && r.LastHeartbeat < runningStaleBefore) ||
                 (r.LastHeartbeat == null && r.StartedAt != null && r.StartedAt < runningStaleBefore)))
            .Select(r => new { r.Id, r.WorkerId, r.LastHeartbeat, r.StartedAt, r.CreatedAt })
            .ToListAsync(ct)
            .ConfigureAwait(false);

        var reapedRunning = 0;
        foreach (var run in running)
        {
            // Authoritative liveness: worker_id holds the EXECUTING AGENT's id
            // (as text). tester_id is a project_tester FK, NOT an agent id, so it
            // can never be used to look up the agent in the registry. Parse the
            // worker_id Guid; if that agent still holds a live connection the run
            // may just be slow to heartbeat — leave it. Only reap when the agent
            // is genuinely absent from the registry (or worker_id is
            // null/unparseable despite 120s of silence — a run that was never
            // claimed by any live agent).
            Guid? workerAgentId = Guid.TryParse(run.WorkerId, out var parsed) ? parsed : null;
            var lastProgress = run.LastHeartbeat ?? run.StartedAt ?? run.CreatedAt;
            var noProgressFor = now - lastProgress;
            var agentOnline = workerAgentId is Guid onlineId && _registry.IsOnline(onlineId);
            // An ONLINE agent normally means "slow, not dead" — but the agent
            // streams an attempt event (which refreshes last_heartbeat) per
            // attempt, so total silence for NoProgressCutoff means the tester
            // itself is wedged: prod had a `path` probe hang with the agent
            // heartbeating happily, and the run sat `running` for 22 minutes
            // holding its runner until it was cancelled by hand (prod mode sweep,
            // v0.28.213). Reap those too, with a message that says which it was.
            if (agentOnline && noProgressFor < NoProgressCutoff)
            {
                continue;
            }
            var reapError = agentOnline
                ? string.Format(
                    CultureInfo.InvariantCulture,
                    "Run made no progress for {0:F0} minutes while its agent stayed online — the tester "
                    + "process appears wedged; the run was failed so the runner could be released.",
                    noProgressFor.TotalMinutes)
                : RunningReapedError;

            var affected = await db.TestRuns
                .Where(r => r.Id == run.Id && r.Status == "running")
                .ExecuteUpdateAsync(
                    s => s
                        .SetProperty(r => r.Status, "failed")
                        .SetProperty(r => r.ErrorMessage, reapError)
                        .SetProperty(r => r.FinishedAt, now),
                    ct)
                .ConfigureAwait(false);

            if (affected == 0)
            {
                // Lost a race (agent reported completion first) — skip the event.
                continue;
            }

            _events.Publish(new JobUpdate(
                JobId: run.Id,
                Status: "failed",
                AgentId: workerAgentId,
                StartedAt: null,
                FinishedAt: eventNow));

            reapedRunning++;
            _logger.LogWarning(
                "Reaped stale running run {RunId} — agent {WorkerId} offline",
                run.Id, run.WorkerId);
        }

        // ── Stale `queued` runs ─────────────────────────────────────────────
        // Two guards beyond the Rust original (2026-08-03 matrix diagnosis —
        // BOTH bit the same relaunch):
        //
        // 1. LIVENESS: the error text always said "check that an agent is
        //    online", but nothing checked. A queued run behind a CONNECTED but
        //    busy runner is legitimately waiting its turn — killing it at 5min
        //    starves any launch wider than the runner's concurrency. Only reap
        //    when the registry has no online agent at all.
        // 2. CLOCK BASIS: created_at is wrong for promoted cells — provisioning
        //    routinely takes >5min, promotion rewrites endpoint_kind away from
        //    'pending' (losing that exclusion), and the run re-queues already
        //    older than the cutoff → reaped on the next tick before the runner
        //    could ever claim it. The orchestrator stamps last_heartbeat at
        //    re-queue, so the queued-age basis is COALESCE(last_heartbeat,
        //    created_at) — "time since it became claimable".
        //
        // Pending-endpoint runs are still excluded — they wait for the
        // provisioning orchestrator, not an agent.
        var reapedQueued = 0;
        var anyAgentOnline = _registry.OnlineAgents().Count > 0;
        // A runner being auto-woken for queued work (power_state 'starting')
        // takes a few minutes to boot + connect — no agent is online during
        // that window, but the queued runs are about to be claimable. Don't
        // reap while a wake is in flight.
        var anyWaking = !anyAgentOnline
            && await db.ProjectTesters.AnyAsync(t => t.PowerState == "starting", ct)
                .ConfigureAwait(false);
        var queuedCutoff = now - QueuedCutoff;
        var stuckQueued = anyAgentOnline || anyWaking
            ? new List<Guid>()
            : await db.TestRuns
                .Where(r => r.Status == "queued"
                    && (r.LastHeartbeat ?? r.CreatedAt) < queuedCutoff
                    && r.TestConfig.EndpointKind != "pending")
                .Select(r => r.Id)
                .ToListAsync(ct)
                .ConfigureAwait(false);

        foreach (var runId in stuckQueued)
        {
            var affected = await db.TestRuns
                .Where(r => r.Id == runId && r.Status == "queued")
                .ExecuteUpdateAsync(
                    s => s
                        .SetProperty(r => r.Status, "failed")
                        .SetProperty(r => r.ErrorMessage, QueuedReapedError)
                        .SetProperty(r => r.FinishedAt, now),
                    ct)
                .ConfigureAwait(false);

            if (affected == 0)
            {
                // A redispatcher/agent claimed it between query and update.
                continue;
            }

            _events.Publish(new JobUpdate(
                JobId: runId,
                Status: "failed",
                AgentId: null,
                StartedAt: null,
                FinishedAt: eventNow));

            reapedQueued++;
            _logger.LogWarning(
                "Reaped stale queued run {RunId} — no runner claimed it within {Cutoff}s",
                runId, QueuedCutoff.TotalSeconds);
        }

        // ── Stale `pending`/`running` deployments (restart-orphan sweep) ─────
        // The deploy runs on a detached in-process Task.Run; a control-plane
        // restart mid-deploy orphans the deployment at pending/running forever,
        // and with it any run in `provisioning`. Nothing else times these out.
        // Fail deployments older than THEIR OWN scaled budget + slack — the
        // orchestrator's next tick then fails their run via the DeploymentFailed
        // arm (quality audit F3(b)).
        //
        // Threshold: DeployRunner.DeployTimeoutFor(config) + slack — the SAME
        // scaled budget (#740) the runner's own timeout enforces, so a deploy
        // legitimately using its language budget is never watchdog-killed
        // (issue #804: a 38m cpp deploy died at the old flat 30m). The config
        // parse can't run in SQL, so the query prefilters at the MINIMUM
        // possible threshold (base budget + slack) and the per-deployment
        // scaled threshold is applied per row below.
        //
        // Age basis is COALESCE(started_at, created_at): the runner stamps
        // started_at when the deploy actually flips to running, and the startup
        // recovery pass (issues #764/#785) stamps it when it re-claims an
        // interrupted deployment — so a recovered attempt gets a fresh window
        // instead of being reaped against the ORIGINAL attempt's created_at.
        // Pre-V052 rows (started_at null) keep the created_at basis unchanged.
        var deploymentStaleBefore = now - (DeployRunner.BaseDeployTimeout + DeploymentBudgetSlack);
        var stuckDeployments = await db.Deployments
            .Where(d => (d.Status == "pending" || d.Status == "running")
                && (d.StartedAt ?? d.CreatedAt) < deploymentStaleBefore)
            .Select(d => new { d.DeploymentId, d.Config, d.Status, d.StartedAt, d.CreatedAt, d.RecoveryAttempts })
            .ToListAsync(ct)
            .ConfigureAwait(false);

        var reapedDeployments = 0;
        foreach (var dep in stuckDeployments)
        {
            var reapCutoff = DeploymentReapCutoffFor(dep.Config);
            var basis = dep.StartedAt ?? dep.CreatedAt;
            if (now - basis < reapCutoff)
            {
                // Within its own scaled budget (+slack) — the deploy runner's
                // timeout owns this deployment; leave it alone. (The runner
                // re-stamps started_at when the install phase starts, #817, so
                // this basis ages the same re-anchored window the runner's own
                // timer enforces.)
                continue;
            }

            // A still-`pending` deployment never ran any install (the runner
            // flips to `running` before spawning install.sh) — reap it with the
            // RETRYABLE never-started marker so the orchestrator re-queues the
            // linked run instead of failing it terminally (issue #817). A
            // `running` one keeps the terminal budget message (#804/#808).
            var neverStarted = dep.Status == "pending";
            var reapError = neverStarted
                ? NeverStartedReapedErrorFor(dep.Config)
                : DeploymentReapedErrorFor(dep.Config, recoveredFromRestart: dep.RecoveryAttempts > 0);
            // The update re-checks the aging basis AND pins the status we
            // classified: a concurrent recovery re-claim (another replica's
            // startup pass, #785) re-stamps started_at between our SELECT and
            // this UPDATE — the fresh stamp must win, not the reap — and a
            // pending→running flip (the runner picked it up) must invalidate
            // the never-started classification, not carry it over.
            var reapBasisBefore = now - reapCutoff;
            var affected = await db.Deployments
                .Where(d => d.DeploymentId == dep.DeploymentId
                    && d.Status == dep.Status
                    && (d.StartedAt ?? d.CreatedAt) < reapBasisBefore)
                .ExecuteUpdateAsync(
                    s => s
                        .SetProperty(d => d.Status, "failed")
                        .SetProperty(d => d.ErrorMessage, reapError)
                        .SetProperty(d => d.FinishedAt, now),
                    ct)
                .ConfigureAwait(false);

            if (affected == 0)
            {
                // The deploy runner finished it (or a recovery pass re-claimed
                // it) between query and update.
                continue;
            }

            reapedDeployments++;
            _logger.LogWarning(
                "Reaped stale deployment {DeploymentId} — {Status} past its {Budget}m budget (+{Slack}m slack; recovery_attempts={Recoveries}; retryable={Retryable})",
                dep.DeploymentId,
                dep.Status,
                DeployRunner.DeployTimeoutFor(dep.Config).TotalMinutes,
                DeploymentBudgetSlack.TotalMinutes,
                dep.RecoveryAttempts,
                neverStarted);
        }

        // ── Orphaned `provisioning` runs whose deployment is gone/missing ────
        // A run stuck in `provisioning` whose linked deployment no longer exists
        // (or was never created) can never be promoted or failed by the
        // orchestrator (its DeploymentFailed/Cancelled arms need a deployment
        // row). Fail such runs directly once they are older than the cutoff.
        var provisioningStaleBefore = now - ProvisioningOrphanCutoff;
        var provisioningRuns = await db.TestRuns
            .Where(r => r.Status == "provisioning" && r.CreatedAt < provisioningStaleBefore)
            .Select(r => new { r.Id, r.ProvisioningDeploymentId })
            .ToListAsync(ct)
            .ConfigureAwait(false);

        var reapedProvisioning = 0;
        foreach (var run in provisioningRuns)
        {
            // Only reap when the deployment is genuinely gone/missing — a run with
            // a live deployment is owned by the orchestrator (which fails it via
            // the DeploymentFailed arm once the sweep above marks the deployment
            // failed). A null link, or a link to a vanished deployment row, means
            // the orchestrator can never resolve it.
            var deploymentExists = run.ProvisioningDeploymentId is Guid depId
                && await db.Deployments
                    .AnyAsync(d => d.DeploymentId == depId, ct)
                    .ConfigureAwait(false);
            if (deploymentExists)
            {
                continue;
            }

            var affected = await db.TestRuns
                .Where(r => r.Id == run.Id && r.Status == "provisioning")
                .ExecuteUpdateAsync(
                    s => s
                        .SetProperty(r => r.Status, "failed")
                        .SetProperty(r => r.ErrorMessage, ProvisioningOrphanError)
                        .SetProperty(r => r.FinishedAt, now),
                    ct)
                .ConfigureAwait(false);

            if (affected == 0)
            {
                continue;
            }

            _events.Publish(new JobUpdate(
                JobId: run.Id,
                Status: "failed",
                AgentId: null,
                StartedAt: null,
                FinishedAt: eventNow));

            reapedProvisioning++;
            _logger.LogWarning(
                "Reaped orphaned provisioning run {RunId} — its deployment {DeploymentId} is gone/missing",
                run.Id, run.ProvisioningDeploymentId);
        }

        if (reapedRunning > 0 || reapedQueued > 0 || reapedDeployments > 0 || reapedProvisioning > 0)
        {
            _logger.LogInformation(
                "Stale-job watchdog: failed {Running} running + {Queued} queued run(s), "
                + "{Deployments} deployment(s), {Provisioning} orphaned provisioning run(s)",
                reapedRunning, reapedQueued, reapedDeployments, reapedProvisioning);
        }

        _monitor.ReportTick(
            OpsServiceNames.Watchdog,
            reapedRunning + reapedQueued + reapedDeployments + reapedProvisioning,
            $"reaped_running={reapedRunning} reaped_queued={reapedQueued} "
            + $"reaped_deployments={reapedDeployments} reaped_provisioning={reapedProvisioning}");
    }
}
