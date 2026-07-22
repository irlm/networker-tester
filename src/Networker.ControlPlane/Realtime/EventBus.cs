using System.Text.Json;
using Microsoft.AspNetCore.SignalR;
using Networker.ControlPlane.Realtime.RawWs;

namespace Networker.ControlPlane.Realtime;

/// <summary>
/// Sequenced dashboard event bus with a bounded replay ring buffer — the C#
/// re-architecture of the Rust <c>EventBus</c>
/// (crates/networker-dashboard/src/services/event_bus.rs).
///
/// Responsibilities:
/// <list type="bullet">
///   <item>Assign a monotonic <c>seq</c> (starts at 1) to every published event
///     via <see cref="Interlocked"/> — the single source of ordering truth.</item>
///   <item>Retain the last <see cref="EventLogCapacity"/> events in a bounded,
///     thread-safe ring buffer so a reconnecting browser presenting
///     <c>?since=&lt;seq&gt;</c> can catch up on what it missed.</item>
///   <item>Broadcast each event live to all connected browser clients through
///     <see cref="IHubContext{BrowserHub}"/> — SignalR replaces the Rust
///     <c>tokio::sync::broadcast</c> channel and per-connection fan-out.</item>
/// </list>
///
/// <para><b>Semantics preserved from Rust:</b></para>
/// <list type="bullet">
///   <item>Single-process: replay is served only by the instance that buffered
///     the events. On restart <c>seq</c> resets to 1 (no false-positive replay);
///     a missed window is handled the same as a restart — the UI misses a
///     handful of events, acceptable for best-effort live streaming atop a
///     durable DB.</item>
///   <item><c>Replay(since)</c> returns buffered events with <c>seq &gt; since</c>,
///     oldest first; empty when <c>since</c> is at/ahead of the head.</item>
///   <item>Ring eviction: when full, the oldest event is dropped before the new
///     one is appended (FIFO), capping memory at <c>EventLogCapacity</c>.</item>
/// </list>
///
/// <para><b>Registered as a singleton</b> (see
/// <c>EventBusServiceCollectionExtensions.AddDashboardEventBus</c>).</para>
/// </summary>
public sealed class EventBus
{
    /// <summary>
    /// Maximum number of recent events held in the replay ring. Matches the
    /// Rust <c>EVENT_LOG_CAPACITY</c>. At ~50 events/s during a busy benchmark
    /// this buys ~40s of recent history — ample for a WS reconnect after a
    /// transient network blip.
    /// </summary>
    public const int EventLogCapacity = 2048;

    private const string BroadcastMethod = "event";

    private readonly IHubContext<BrowserHub> _hub;
    private readonly ILogger<EventBus> _logger;

    // Raw-WebSocket fan-out (Phase-2 M6): the React frontend speaks raw WS, not
    // SignalR, so every published event is ALSO serialized once and enqueued to
    // each registered raw browser socket. Optional-with-default so existing
    // construction sites (and DI graphs without AddRawWebSockets) keep working.
    private readonly RawSocketRegistry? _rawSockets;

    // Monotonic sequence counter. `Interlocked.Increment` returns the
    // POST-increment value, so the first published event gets seq = 1 (the
    // counter starts at 0) — identical to the Rust `fetch_add(1) + 1`.
    private long _seq;

    // Bounded FIFO ring buffer of recent events for replay. Guarded by
    // `_bufferLock`; the critical section is kept tiny (enqueue + optional
    // dequeue / snapshot copy) so publishers never contend meaningfully.
    private readonly Queue<SeqEvent> _buffer = new(EventLogCapacity);
    private readonly Lock _bufferLock = new();

    public EventBus(
        IHubContext<BrowserHub> hub,
        ILogger<EventBus> logger,
        RawSocketRegistry? rawSockets = null,
        IEnumerable<IDashboardEventObserver>? observers = null)
    {
        _hub = hub;
        _logger = logger;
        _rawSockets = rawSockets;
        _observers = observers?.ToArray() ?? [];
    }

    // In-process observers (e.g. the tester-queue update producer) — see
    // IDashboardEventObserver. Snapshotted to an array at construction.
    private readonly IDashboardEventObserver[] _observers;

    /// <summary>
    /// Publish an event: assign the next <c>seq</c>, append to the replay ring
    /// (evicting the oldest entry when full), then broadcast to all connected
    /// browser clients. Returns the assigned sequence number.
    /// </summary>
    /// <remarks>
    /// The broadcast is fire-and-forget (SignalR <c>SendAsync</c> is awaited
    /// internally on a detached task) so publishers on hot paths (agent-hub
    /// slice, deploy streamer) never block on client I/O. Reaching zero clients
    /// is a legitimate steady state, never an error — mirrors the Rust
    /// <c>send()</c> that returns 0 subscribers without failing.
    /// </remarks>
    public long Publish(DashboardEvent evt)
    {
        ArgumentNullException.ThrowIfNull(evt);

        var seq = Interlocked.Increment(ref _seq);
        var seqEvent = new SeqEvent(seq, evt);

        lock (_bufferLock)
        {
            if (_buffer.Count >= EventLogCapacity)
            {
                _buffer.Dequeue();
            }
            _buffer.Enqueue(seqEvent);
        }

        // In-process observers (tester-queue producer etc.) — synchronous but
        // contractually non-blocking (they detach real work); an observer bug
        // must never break publishing.
        foreach (var observer in _observers)
        {
            try
            {
                observer.OnEvent(evt);
            }
            catch (Exception ex)
            {
                _logger.LogWarning(
                    ex, "Dashboard event observer {Observer} failed for seq={Seq}",
                    observer.GetType().Name, seq);
            }
        }

        // Broadcast live. Detached so Publish stays synchronous and non-blocking
        // for callers; failures are logged, never propagated (a dead client
        // must not break the publisher).
        _ = BroadcastAsync(seqEvent);

        // Raw-WS fan-out: serialize once (the SeqEventJsonConverter flat shape,
        // {"seq":N,"type":"...",...} — identical to the SignalR payload) and
        // enqueue to every raw browser socket. Non-blocking: each socket has a
        // bounded send queue; overflow ejects that socket (slow-subscriber
        // ejection, mirroring the Rust broadcast-lag behavior). Failures are
        // contained the same way as the SignalR path — never the publisher's
        // problem.
        if (_rawSockets is { BrowserClientCount: > 0 } raw)
        {
            try
            {
                raw.BroadcastBrowserEvent(seq, JsonSerializer.Serialize(seqEvent));
            }
            catch (Exception ex)
            {
                _logger.LogWarning(ex, "Failed raw-WS fan-out for SeqEvent seq={Seq}", seq);
            }
        }

        return seq;
    }

    private async Task BroadcastAsync(SeqEvent seqEvent)
    {
        try
        {
            await _hub.Clients.All.SendAsync(BroadcastMethod, seqEvent);
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "Failed to broadcast SeqEvent seq={Seq}", seqEvent.Seq);
        }
    }

    /// <summary>
    /// Snapshot of buffered events with <c>seq &gt; since</c>, oldest first.
    /// Returns an empty list when <c>since</c> is at or ahead of the highest
    /// buffered seq (nothing to replay).
    /// </summary>
    /// <remarks>
    /// <b>Edge cases (matching Rust):</b>
    /// <list type="bullet">
    ///   <item><c>since = 0</c> (first connect / no dedup point): returns the
    ///     entire buffer. Callers pass 0 to mean "no replay" and simply skip
    ///     calling this — but if called, it is well-defined.</item>
    ///   <item><c>since</c> below the oldest retained seq (client was offline
    ///     longer than the ring holds): returns the whole buffer. The client
    ///     necessarily misses the evicted window — indistinguishable from a
    ///     dashboard restart, handled the same way (best-effort).</item>
    ///   <item><c>since</c> at/beyond the head: empty list.</item>
    /// </list>
    /// The caller must subscribe to the live feed <b>before</b> calling this,
    /// then dedup incoming live events by <c>seq &gt; maxReplayedSeq</c> — an
    /// event published in the race window appears in both paths and the dedup
    /// rule drops the live duplicate. <see cref="BrowserHub"/> implements this.
    /// </remarks>
    public IReadOnlyList<SeqEvent> Replay(long since)
    {
        lock (_bufferLock)
        {
            // Fast path: whole buffer newer than `since` (or since <= 0).
            if (_buffer.Count == 0)
            {
                return Array.Empty<SeqEvent>();
            }

            var result = new List<SeqEvent>(_buffer.Count);
            foreach (var e in _buffer)
            {
                if (e.Seq > since)
                {
                    result.Add(e);
                }
            }
            return result;
        }
    }

    /// <summary>
    /// The seq of the most recently published event, or 0 if none. Useful for a
    /// freshly-connected client to learn the current head without waiting for
    /// the next event.
    /// </summary>
    public long CurrentSeq() => Interlocked.Read(ref _seq);

    // Design note: a plain Queue under a single short-lived Lock is used rather
    // than a ConcurrentQueue because bounded eviction requires an atomic
    // "dequeue-if-full then enqueue", which ConcurrentQueue cannot express
    // without external locking anyway. The critical section is O(1) on publish
    // and O(buffer) only on replay (rare), so contention is negligible.
}
