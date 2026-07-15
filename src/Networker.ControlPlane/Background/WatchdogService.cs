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
///     (<c>tester_id</c> holds the AGENT id) is not in the live
///     <see cref="AgentConnectionRegistry"/>. Hub/registry membership is the
///     authoritative "truly online" signal (identical to the Rust
///     <c>state.agents.is_agent_online</c> guard): if the socket is live the run
///     may just be slow to heartbeat, so it is left alone. A run is NEVER reaped
///     merely for having a null <c>tester_id</c> — it must first fail the 120s
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
        // never even a candidate — regardless of its tester_id. Registry
        // membership (in-memory, authoritative) is then checked per-row below —
        // never expressible in SQL.
        var runningStaleBefore = now - RunningStaleCutoff;
        var running = await db.TestRuns
            .Where(r => r.Status == "running" &&
                ((r.LastHeartbeat != null && r.LastHeartbeat < runningStaleBefore) ||
                 (r.LastHeartbeat == null && r.StartedAt != null && r.StartedAt < runningStaleBefore)))
            .Select(r => new { r.Id, r.TesterId })
            .ToListAsync(ct)
            .ConfigureAwait(false);

        var reapedRunning = 0;
        foreach (var run in running)
        {
            // Authoritative liveness: tester_id holds the EXECUTING AGENT's id
            // (Rust: "Tester id == agent id in the v0.28 model"). If that agent
            // still holds a live connection the run may just be slow to
            // heartbeat — leave it. Only reap when the agent is genuinely absent
            // from the registry (or was never stamped despite 120s of silence).
            if (run.TesterId is Guid agentId && _registry.IsOnline(agentId))
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
                AgentId: run.TesterId,
                StartedAt: null,
                FinishedAt: eventNow));

            reapedRunning++;
            _logger.LogWarning(
                "Reaped stale running run {RunId} — agent {TesterId} offline",
                run.Id, run.TesterId);
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

        if (reapedRunning > 0 || reapedQueued > 0)
        {
            _logger.LogInformation(
                "Stale-job watchdog: failed {Running} running + {Queued} queued run(s)",
                reapedRunning, reapedQueued);
        }

        _monitor.ReportTick(
            OpsServiceNames.Watchdog,
            reapedRunning + reapedQueued,
            $"reaped_running={reapedRunning} reaped_queued={reapedQueued}");
    }
}
