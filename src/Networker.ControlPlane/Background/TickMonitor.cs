using System.Collections.Concurrent;

namespace Networker.ControlPlane.Background;

/// <summary>
/// Canonical names for the hosted background loops — the keys used for both
/// <see cref="TickMonitor"/> reporting and <see cref="LeaderLockKeys"/> advisory
/// keys, and the <c>name</c> field surfaced by <c>GET /api/health/background</c>.
/// Renaming one is a breaking change for dashboards/alerts AND moves its
/// advisory-lock key — don't.
/// </summary>
public static class OpsServiceNames
{
    public const string Scheduler = "scheduler";
    public const string QueuedRedispatch = "queued-redispatch";
    public const string Watchdog = "watchdog";
    public const string AgentReaper = "agent-reaper";
    public const string AutoShutdown = "auto-shutdown";
    public const string OrphanReaper = "orphan-reaper";
    public const string WorkspaceInactivity = "workspace-inactivity";
    public const string ProvisioningOrchestrator = "provisioning-orchestrator";

    /// <summary>Every known background service, in display order.</summary>
    public static readonly string[] All =
    [
        Scheduler,
        QueuedRedispatch,
        Watchdog,
        AgentReaper,
        AutoShutdown,
        OrphanReaper,
        WorkspaceInactivity,
        ProvisioningOrchestrator,
    ];
}

/// <summary>
/// Immutable point-in-time view of one background service's tick history.
/// </summary>
/// <param name="Service">Canonical service name (<see cref="OpsServiceNames"/>).</param>
/// <param name="StartedAt">When the service's loop reported in (per process).</param>
/// <param name="LastTickAt">Completion time of the most recent successful tick,
/// or null if the loop has not completed a tick yet this process.</param>
/// <param name="TicksTotal">Successful ticks since process start.</param>
/// <param name="LastItems">Items processed by the most recent successful tick.</param>
/// <param name="LastNote">Optional per-tick detail (e.g. "launched=2 failed=0").</param>
/// <param name="LastError">Message of the most recent tick failure, if any.</param>
/// <param name="LastErrorAt">When that failure was reported.</param>
public sealed record ServiceTickSnapshot(
    string Service,
    DateTimeOffset StartedAt,
    DateTimeOffset? LastTickAt,
    long TicksTotal,
    int LastItems,
    string? LastNote,
    string? LastError,
    DateTimeOffset? LastErrorAt);

/// <summary>
/// In-process observability registry for the background loops — the data source
/// behind <c>GET /api/health/background</c>. Each hosted service calls
/// <see cref="ReportStarted"/> once when its loop starts,
/// <see cref="ReportTick"/> after every successful tick, and
/// <see cref="ReportError"/> from its per-tick catch block. The ops endpoint
/// reads a consistent <see cref="Snapshot"/> and applies the staleness math
/// (ticked within 3× its expected interval ⇒ healthy).
///
/// <para>Thread-safe: a <see cref="ConcurrentDictionary{TKey,TValue}"/> of
/// per-service states, each mutated under its own lock so a snapshot never
/// observes a torn tick (e.g. new <c>LastTickAt</c> with the old
/// <c>TicksTotal</c>). Purely in-memory and per-replica by design: with
/// per-tick leader election every replica runs ticks over time, so each
/// replica's own view answers "is the background machinery healthy HERE".</para>
///
/// <para>Time is injected (<see cref="TimeProvider"/>) so the staleness math is
/// unit-testable without sleeping.</para>
/// </summary>
public sealed class TickMonitor
{
    private readonly TimeProvider _time;
    private readonly ConcurrentDictionary<string, State> _services = new(StringComparer.Ordinal);

    public TickMonitor(TimeProvider? time = null) => _time = time ?? TimeProvider.System;

    private sealed class State
    {
        public readonly object Sync = new();
        public DateTimeOffset StartedAt;
        public DateTimeOffset? LastTickAt;
        public long TicksTotal;
        public int LastItems;
        public string? LastNote;
        public string? LastError;
        public DateTimeOffset? LastErrorAt;
    }

    private State GetOrAdd(string service) =>
        _services.GetOrAdd(service, _ => new State { StartedAt = _time.GetUtcNow() });

    /// <summary>Record that a service's loop has started (idempotent; the first
    /// call per process wins for <see cref="ServiceTickSnapshot.StartedAt"/>).</summary>
    public void ReportStarted(string service) => GetOrAdd(service);

    /// <summary>Record a successful tick.</summary>
    public void ReportTick(string service, int itemsProcessed, string? note = null)
    {
        var s = GetOrAdd(service);
        lock (s.Sync)
        {
            s.LastTickAt = _time.GetUtcNow();
            s.TicksTotal++;
            s.LastItems = itemsProcessed;
            s.LastNote = note;
        }
    }

    /// <summary>Record a failed tick (called from the loop's catch block).</summary>
    public void ReportError(string service, Exception exception)
    {
        var s = GetOrAdd(service);
        lock (s.Sync)
        {
            s.LastError = $"{exception.GetType().Name}: {exception.Message}";
            s.LastErrorAt = _time.GetUtcNow();
        }
    }

    /// <summary>Consistent snapshot of every service that has reported in,
    /// ordered by service name for stable output.</summary>
    public IReadOnlyList<ServiceTickSnapshot> Snapshot()
    {
        var result = new List<ServiceTickSnapshot>(_services.Count);
        foreach (var (name, s) in _services)
        {
            lock (s.Sync)
            {
                result.Add(new ServiceTickSnapshot(
                    name, s.StartedAt, s.LastTickAt, s.TicksTotal,
                    s.LastItems, s.LastNote, s.LastError, s.LastErrorAt));
            }
        }

        result.Sort(static (a, b) => string.CompareOrdinal(a.Service, b.Service));
        return result;
    }

    /// <summary>The injected clock's current UTC time — the same "now" the
    /// snapshots were stamped with, for staleness math at the endpoint.</summary>
    public DateTimeOffset UtcNow => _time.GetUtcNow();
}
