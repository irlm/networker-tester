using Microsoft.EntityFrameworkCore;
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
    /// How long a deployment may sit in <c>pending</c>/<c>running</c> (or a run in
    /// <c>provisioning</c> whose deployment is gone) before it is failed. The
    /// deploy runs on a DETACHED in-process task; a control-plane restart
    /// mid-deploy (every release!) orphans the deployment forever with nothing to
    /// time it out (quality audit F3(b)). 30 min comfortably exceeds the
    /// <c>DeployRunner.DeployTimeout</c> (30 min) plus slack, so a live deploy is
    /// never falsely reaped.
    /// </summary>
    private static readonly TimeSpan DeploymentStaleCutoff = TimeSpan.FromMinutes(30);

    /// <summary>User-facing message for a deployment orphaned by a restart.</summary>
    private const string DeploymentReapedError =
        "Deployment did not finish within 30 minutes — the control plane may have restarted mid-deploy";

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
            .Select(r => new { r.Id, r.WorkerId })
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
            if (workerAgentId is Guid agentId && _registry.IsOnline(agentId))
            {
                continue;
            }

            var affected = await db.TestRuns
                .Where(r => r.Id == run.Id && r.Status == "running")
                .ExecuteUpdateAsync(
                    s => s
                        .SetProperty(r => r.Status, "failed")
                        .SetProperty(r => r.ErrorMessage, RunningReapedError)
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
        // LINQ: WHERE status = 'queued' AND created_at < now - 300s
        //   AND config.endpoint_kind <> 'pending'.
        // Pending-endpoint runs are waiting on the provisioning orchestrator
        // (which flips them to `provisioning`, then back to `queued` with a
        // concrete endpoint once the VM is live) — the "no runner claimed it"
        // cutoff does not apply to them.
        var queuedCutoff = now - QueuedCutoff;
        var stuckQueued = await db.TestRuns
            .Where(r => r.Status == "queued"
                && r.CreatedAt < queuedCutoff
                && r.TestConfig.EndpointKind != "pending")
            .Select(r => r.Id)
            .ToListAsync(ct)
            .ConfigureAwait(false);

        var reapedQueued = 0;
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
        // Fail deployments older than the cutoff — the orchestrator's next tick
        // then fails their run via the DeploymentFailed arm (quality audit F3(b)).
        var deploymentStaleBefore = now - DeploymentStaleCutoff;
        var stuckDeployments = await db.Deployments
            .Where(d => (d.Status == "pending" || d.Status == "running")
                && d.CreatedAt < deploymentStaleBefore)
            .Select(d => d.DeploymentId)
            .ToListAsync(ct)
            .ConfigureAwait(false);

        var reapedDeployments = 0;
        foreach (var deploymentId in stuckDeployments)
        {
            var affected = await db.Deployments
                .Where(d => d.DeploymentId == deploymentId
                    && (d.Status == "pending" || d.Status == "running"))
                .ExecuteUpdateAsync(
                    s => s
                        .SetProperty(d => d.Status, "failed")
                        .SetProperty(d => d.ErrorMessage, DeploymentReapedError)
                        .SetProperty(d => d.FinishedAt, now),
                    ct)
                .ConfigureAwait(false);

            if (affected == 0)
            {
                // The deploy runner finished it between query and update.
                continue;
            }

            reapedDeployments++;
            _logger.LogWarning(
                "Reaped stale deployment {DeploymentId} — pending/running for more than {Cutoff}m (control plane likely restarted mid-deploy)",
                deploymentId, DeploymentStaleCutoff.TotalMinutes);
        }

        // ── Orphaned `provisioning` runs whose deployment is gone/missing ────
        // A run stuck in `provisioning` whose linked deployment no longer exists
        // (or was never created) can never be promoted or failed by the
        // orchestrator (its DeploymentFailed/Cancelled arms need a deployment
        // row). Fail such runs directly once they are older than the cutoff.
        var provisioningStaleBefore = now - DeploymentStaleCutoff;
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
