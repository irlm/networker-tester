using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Realtime;
using Networker.Data;

namespace Networker.ControlPlane.Background;

/// <summary>
/// Agent-status reaper — the C# re-architecture of the Rust
/// <c>reconcile_once</c> reconciler in
/// <c>crates/networker-dashboard/src/agent_reaper.rs</c>. Runs on a fixed ~60s
/// cadence and reconciles the cached <c>agent.status</c> column against ground
/// truth: an agent is <b>truly online iff it holds a live connection</b> in the
/// singleton <see cref="AgentConnectionRegistry"/> (the SignalR equivalent of
/// the Rust in-memory agent hub). The <c>agent.status</c> column is a cache
/// written on connect/disconnect that goes stale on unclean disconnects (VM
/// force-deallocated) or a dashboard restart (fresh process, empty registry, DB
/// still says "online").
///
/// <para><b>Reap condition</b> (mirrors the Rust SQL exactly):
/// candidates are agents with <c>status = 'online'</c> AND
/// (<c>last_heartbeat IS NULL</c> OR <c>last_heartbeat &lt; now - 90s</c>). For
/// each candidate that is <b>not</b> <see cref="AgentConnectionRegistry.IsOnline"/>
/// its status is flipped to <c>offline</c> and an
/// <c>AgentStatus(status: "offline")</c> event is published. Registry membership
/// is authoritative, so a genuinely-connected agent whose heartbeat merely
/// lagged under load is <b>never</b> reaped.</para>
///
/// <para>The 90s staleness threshold is three missed 30s heartbeats — a
/// confident "gone" signal without flapping on a single lagged beat (Rust
/// <c>STALE_AFTER_SECS = 90</c>).</para>
///
/// <para><b>Scope discipline:</b> identical to <see cref="WatchdogService"/> —
/// <c>NetworkerDbContext</c> is scoped, so each tick opens a fresh
/// <see cref="IServiceScopeFactory"/> scope; the singleton registry and
/// <see cref="EventBus"/> are injected directly.</para>
/// </summary>
public sealed class ReaperService : BackgroundService
{
    /// <summary>How often the reaper reconciles. Matches the Rust 60s cadence.</summary>
    private static readonly TimeSpan TickInterval = TimeSpan.FromSeconds(60);

    /// <summary>
    /// How stale a heartbeat must be before an agent is a reap candidate.
    /// Mirrors the Rust <c>STALE_AFTER_SECS = 90</c> (three missed 30s beats).
    /// </summary>
    private static readonly TimeSpan StaleAfter = TimeSpan.FromSeconds(90);

    private readonly IServiceScopeFactory _scopeFactory;
    private readonly AgentConnectionRegistry _registry;
    private readonly EventBus _events;
    private readonly ILogger<ReaperService> _logger;

    public ReaperService(
        IServiceScopeFactory scopeFactory,
        AgentConnectionRegistry registry,
        EventBus events,
        ILogger<ReaperService> logger)
    {
        _scopeFactory = scopeFactory;
        _registry = registry;
        _events = events;
        _logger = logger;
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        _logger.LogInformation("Agent-status reaper started (tick={Tick}s)", TickInterval.TotalSeconds);

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
                // Never let one bad tick kill the loop (Rust logs + continues).
                _logger.LogError(ex, "Agent-status reaper tick failed");
            }
        }
    }

    private async Task TickAsync(CancellationToken ct)
    {
        using var scope = _scopeFactory.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();

        var staleBefore = DateTime.UtcNow - StaleAfter;

        // Candidates: DB says online, but heartbeat is stale (or never arrived).
        // LINQ maps to: WHERE status = 'online'
        //   AND (last_heartbeat IS NULL OR last_heartbeat < now - 90s).
        var candidates = await db.Agents
            .Where(a => a.Status == "online"
                && (a.LastHeartbeat == null || a.LastHeartbeat < staleBefore))
            .Select(a => new { a.AgentId, a.Name, a.LastHeartbeat })
            .ToListAsync(ct)
            .ConfigureAwait(false);

        var reaped = 0;
        foreach (var agent in candidates)
        {
            // Registry membership is authoritative: a heartbeat can lag under
            // load while the socket is still very much alive. Only reap agents
            // genuinely absent from the live registry.
            if (_registry.IsOnline(agent.AgentId))
            {
                continue;
            }

            // Guarded update: only flip if still 'online' (avoids clobbering a
            // reconnect that re-registered between our query and this write).
            var affected = await db.Agents
                .Where(a => a.AgentId == agent.AgentId && a.Status == "online")
                .ExecuteUpdateAsync(
                    s => s.SetProperty(a => a.Status, "offline"),
                    ct)
                .ConfigureAwait(false);

            if (affected == 0)
            {
                continue;
            }

            _events.Publish(new AgentStatus(
                AgentId: agent.AgentId,
                Status: "offline",
                LastHeartbeat: agent.LastHeartbeat is DateTime hb
                    ? new DateTimeOffset(DateTime.SpecifyKind(hb, DateTimeKind.Utc))
                    : null));

            reaped++;
            _logger.LogInformation(
                "Marked stale agent {AgentId} ({AgentName}) offline (no heartbeat, not in live registry)",
                agent.AgentId, agent.Name);
        }

        if (reaped > 0)
        {
            _logger.LogInformation("Agent-status reaper: flipped {Count} stale agent(s) offline", reaped);
        }
    }
}
