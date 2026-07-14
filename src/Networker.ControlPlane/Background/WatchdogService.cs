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
///   <item><b>Stale <c>running</c> runs</b> — a run assigned to an agent
///     (<c>tester_id</c>) that is no longer in the live
///     <see cref="AgentConnectionRegistry"/>. Hub/registry membership is the
///     authoritative "truly online" signal (identical to the Rust
///     <c>state.agents.is_agent_online</c> guard): if the socket is live the run
///     may just be slow to heartbeat, so it is left alone. Otherwise it is failed
///     with <c>error_message = "Agent offline"</c> and <c>finished_at = now</c>.</item>
///   <item><b>Stale <c>queued</c> runs</b> — runs still <c>queued</c> whose
///     <c>created_at</c> is older than <see cref="QueuedCutoff"/> (the Rust
///     <c>QUEUED_CUTOFF_SECS = 300</c>, 5 minutes). No runner ever claimed them;
///     they are failed with <c>error_message = "Queued too long"</c>.</item>
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

    private readonly IServiceScopeFactory _scopeFactory;
    private readonly AgentConnectionRegistry _registry;
    private readonly EventBus _events;
    private readonly ILogger<WatchdogService> _logger;

    public WatchdogService(
        IServiceScopeFactory scopeFactory,
        AgentConnectionRegistry registry,
        EventBus events,
        ILogger<WatchdogService> logger)
    {
        _scopeFactory = scopeFactory;
        _registry = registry;
        _events = events;
        _logger = logger;
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        _logger.LogInformation("Stale-job watchdog started (tick={Tick}s)", TickInterval.TotalSeconds);

        // PeriodicTimer's steady cadence is the C# analogue of the Rust
        // tokio::time::interval loop. A slow tick simply delays the next one; we
        // never fire a burst to "catch up".
        using var timer = new PeriodicTimer(TickInterval);
        while (await timer.WaitForNextTickAsync(stoppingToken).ConfigureAwait(false))
        {
            try
            {
                await TickAsync(stoppingToken).ConfigureAwait(false);
            }
            catch (OperationCanceledException) when (stoppingToken.IsCancellationRequested)
            {
                break;
            }
            catch (Exception ex)
            {
                // A single failed tick must never kill the loop — log and retry
                // next interval (matches the Rust `tracing::error!` + continue).
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
        // LINQ: WHERE status = 'running'. Registry membership (in-memory,
        // authoritative) is checked per-row below — never expressible in SQL.
        var running = await db.TestRuns
            .Where(r => r.Status == "running")
            .Select(r => new { r.Id, r.TesterId })
            .ToListAsync(ct)
            .ConfigureAwait(false);

        var reapedRunning = 0;
        foreach (var run in running)
        {
            // Authoritative liveness: if the agent still holds a live connection
            // the run may just be lagging its heartbeat — leave it. Only reap
            // when the agent is genuinely absent from the registry. A run with no
            // tester assigned can never have a live agent, so it is reapable.
            if (run.TesterId is Guid testerId && _registry.IsOnline(testerId))
            {
                continue;
            }

            var affected = await db.TestRuns
                .Where(r => r.Id == run.Id && r.Status == "running")
                .ExecuteUpdateAsync(
                    s => s
                        .SetProperty(r => r.Status, "failed")
                        .SetProperty(r => r.ErrorMessage, "Agent offline")
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
        // LINQ: WHERE status = 'queued' AND created_at < now - 300s.
        var queuedCutoff = now - QueuedCutoff;
        var stuckQueued = await db.TestRuns
            .Where(r => r.Status == "queued" && r.CreatedAt < queuedCutoff)
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
                        .SetProperty(r => r.ErrorMessage, "Queued too long")
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
    }
}
