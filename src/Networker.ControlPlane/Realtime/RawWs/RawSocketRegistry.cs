using System.Collections.Concurrent;
using System.Threading.Channels;

namespace Networker.ControlPlane.Realtime.RawWs;

/// <summary>
/// One raw-WebSocket client with a bounded, single-reader send queue.
///
/// <para><b>Why a queue:</b> <see cref="System.Net.WebSockets.WebSocket.SendAsync(ArraySegment{byte}, System.Net.WebSockets.WebSocketMessageType, bool, CancellationToken)"/>
/// is NOT safe for concurrent callers. Every producer (EventBus fan-out, tester
/// queue broadcasts, snapshot pushes) enqueues a serialized JSON text frame here;
/// a single pump task (<see cref="RunSendPumpAsync"/>) drains the channel and is
/// the ONLY code that touches the socket's send side.</para>
///
/// <para><b>Backpressure / slow-subscriber ejection:</b> the channel is bounded
/// (<see cref="DefaultSendQueueCapacity"/>). When a client can't drain fast
/// enough and the queue fills, <see cref="TryEnqueue(long, string)"/> fails,
/// the connection is marked dropped, and the <c>onDropped</c> callback fires
/// (production wires it to <c>WebSocket.Abort()</c>). This mirrors the Rust
/// hubs: the tester hub's bounded mpsc drops the subscriber when full, and the
/// browser hub's lagged broadcast channel forces a client resync — a stuck
/// client never blocks the publisher.</para>
///
/// <para><b>Replay watermark (browser feed):</b> the Rust browser hub subscribes
/// to the live channel BEFORE snapshotting the replay log, then skips live
/// events with <c>seq &lt;= max_replayed</c>. Same here: the connection is
/// registered (frames start buffering), the replay batch is written directly to
/// the socket, <see cref="SetReplayWatermark"/> is set, and only then does the
/// pump start — it discards any buffered live frame whose seq is covered by the
/// replay. Frames enqueued with <c>seq = 0</c> (tester-queue traffic) bypass the
/// watermark check.</para>
///
/// <para>The send side is delegate-based (<c>sendTextAsync</c>) so the
/// queue/overflow/watermark logic is unit-testable without a real socket.</para>
/// </summary>
public sealed class RawSocketConnection
{
    /// <summary>
    /// Per-socket send-queue capacity. Comfortably above the Rust tester hub's
    /// CHANNEL_BUF (64) — the browser feed can burst harder during benchmarks.
    /// </summary>
    public const int DefaultSendQueueCapacity = 256;

    private readonly Channel<(long Seq, string Json)> _queue;
    private readonly Func<string, CancellationToken, Task> _sendTextAsync;
    private readonly Action<RawSocketConnection>? _onDropped;

    private long _replayWatermark;
    private int _dropped; // 0 = live, 1 = dropped (Interlocked)

    public RawSocketConnection(
        string id,
        Func<string, CancellationToken, Task> sendTextAsync,
        Action<RawSocketConnection>? onDropped = null,
        int capacity = DefaultSendQueueCapacity)
    {
        ArgumentException.ThrowIfNullOrEmpty(id);
        ArgumentNullException.ThrowIfNull(sendTextAsync);
        ArgumentOutOfRangeException.ThrowIfLessThan(capacity, 1);

        Id = id;
        _sendTextAsync = sendTextAsync;
        _onDropped = onDropped;
        _queue = Channel.CreateBounded<(long, string)>(new BoundedChannelOptions(capacity)
        {
            SingleReader = true,
            SingleWriter = false,
            // Wait mode makes TryWrite return false when full (we never await a
            // write — a full queue means the client is too slow and gets dropped).
            FullMode = BoundedChannelFullMode.Wait,
        });
    }

    /// <summary>Registry/connection identifier (never a SignalR connection id).</summary>
    public string Id { get; }

    /// <summary>True once the connection has been ejected or torn down.</summary>
    public bool IsDropped => Volatile.Read(ref _dropped) == 1;

    /// <summary>
    /// Frames enqueued with a positive <c>seq</c> at or below this watermark are
    /// discarded by the pump (already delivered via the replay batch). Set once,
    /// after the replay flush and before <see cref="RunSendPumpAsync"/> starts.
    /// </summary>
    public void SetReplayWatermark(long maxReplayedSeq) =>
        Volatile.Write(ref _replayWatermark, maxReplayedSeq);

    /// <summary>
    /// Enqueue a serialized JSON text frame for this client. Returns false (and
    /// ejects the connection) if the send queue is full or the connection is
    /// already dropped. Never blocks — safe on hot publish paths.
    /// </summary>
    /// <param name="seq">Event sequence number for browser-feed frames (used by
    /// the replay-watermark dedup); pass 0 for frames without replay semantics
    /// (tester-queue traffic).</param>
    /// <param name="json">The exact UTF-16 JSON payload to send as one text frame.</param>
    public bool TryEnqueue(long seq, string json)
    {
        if (IsDropped)
        {
            return false;
        }

        if (_queue.Writer.TryWrite((seq, json)))
        {
            return true;
        }

        // Queue full → slow subscriber → eject (Rust: mpsc try_send failure
        // drops the subscriber; broadcast Lagged forces a resync).
        Drop();
        return false;
    }

    /// <summary>Enqueue a frame with no replay-dedup semantics.</summary>
    public bool TryEnqueue(string json) => TryEnqueue(0, json);

    /// <summary>
    /// Eject the connection: no further frames are accepted, the pump drains and
    /// exits, and <c>onDropped</c> (socket abort in production) fires exactly once.
    /// </summary>
    public void Drop()
    {
        if (Interlocked.Exchange(ref _dropped, 1) == 1)
        {
            return;
        }

        _queue.Writer.TryComplete();
        _onDropped?.Invoke(this);
    }

    /// <summary>
    /// Graceful shutdown: stop accepting frames and let the pump finish sending
    /// what is already queued, without invoking the drop callback.
    /// </summary>
    public void CompleteQueue() => _queue.Writer.TryComplete();

    /// <summary>
    /// The single send pump: drains the queue in order and writes each frame to
    /// the socket. Runs until the queue completes, the token cancels, or a send
    /// fails (which ejects the connection). Start exactly once per connection.
    /// </summary>
    public async Task RunSendPumpAsync(CancellationToken cancellationToken)
    {
        try
        {
            await foreach (var (seq, json) in _queue.Reader.ReadAllAsync(cancellationToken)
                               .ConfigureAwait(false))
            {
                // Replay dedup: a live event published during the replay window
                // was already sent from the replay batch — skip the live copy.
                if (seq > 0 && seq <= Volatile.Read(ref _replayWatermark))
                {
                    continue;
                }

                await _sendTextAsync(json, cancellationToken).ConfigureAwait(false);
            }
        }
        catch (OperationCanceledException)
        {
            // Normal teardown.
        }
        catch (Exception)
        {
            // Send failure — the socket is unusable; eject so producers stop
            // queueing and the endpoint's cleanup path runs.
            Drop();
        }
    }
}

/// <summary>
/// Singleton registry of raw-WebSocket subscribers for the two browser-facing
/// feeds, giving non-SignalR clients the same fan-out the hubs get:
///
/// <list type="bullet">
///   <item><b>Browser feed</b> (<c>/ws/dashboard</c>): a flat set of connections.
///     <see cref="EventBus.Publish"/> serializes each <see cref="SeqEvent"/> once
///     and calls <see cref="BroadcastBrowserEvent"/> — the raw twin of the
///     SignalR <c>Clients.All</c> broadcast.</item>
///   <item><b>Tester-queue feed</b> (<c>/ws/testers</c>): connections keyed by the
///     same group names <see cref="TesterQueueRegistry.GroupName"/> produces
///     (<c>tq:{projectId}:{testerId}</c>). <see cref="RawWsTesterQueueLifetimeManager"/>
///     mirrors every SignalR group send for <c>tq:*</c> groups into
///     <see cref="BroadcastTesterGroup"/>, so raw sockets receive the exact
///     type-tagged payload without the SignalR envelope.</item>
/// </list>
///
/// All broadcast methods are non-blocking: they enqueue into each connection's
/// bounded channel and eject (remove + abort) any connection whose queue
/// overflows — mirroring the Rust slow-subscriber ejection.
/// </summary>
public sealed class RawSocketRegistry
{
    private readonly ConcurrentDictionary<string, RawSocketConnection> _browserClients = new();

    // groupName ("tq:{project}:{tester}") -> connId -> connection.
    private readonly ConcurrentDictionary<string, ConcurrentDictionary<string, RawSocketConnection>> _testerGroups
        = new();

    private readonly ILogger<RawSocketRegistry> _logger;

    public RawSocketRegistry(ILogger<RawSocketRegistry> logger)
    {
        _logger = logger;
    }

    // ── Browser feed (/ws/dashboard) ─────────────────────────────────────────

    public int BrowserClientCount => _browserClients.Count;

    /// <summary>
    /// Register a browser-feed connection for live fan-out. Call BEFORE reading
    /// the replay batch so no event falls between the snapshot and the live tail
    /// (frames buffer in the connection's channel until its pump starts).
    /// </summary>
    public void RegisterBrowser(RawSocketConnection connection) =>
        _browserClients[connection.Id] = connection;

    public void UnregisterBrowser(RawSocketConnection connection) =>
        _browserClients.TryRemove(connection.Id, out _);

    /// <summary>
    /// Fan a serialized <see cref="SeqEvent"/> frame out to every raw browser
    /// socket. Connections whose send queue overflows are ejected on the spot.
    /// </summary>
    public void BroadcastBrowserEvent(long seq, string json)
    {
        foreach (var connection in _browserClients.Values)
        {
            if (!connection.TryEnqueue(seq, json))
            {
                _browserClients.TryRemove(connection.Id, out _);
                _logger.LogWarning(
                    "raw ws: browser client {Conn} ejected (send queue overflow — slow subscriber)",
                    connection.Id);
            }
        }
    }

    // ── Tester-queue feed (/ws/testers) ──────────────────────────────────────

    public bool HasTesterGroup(string groupName) =>
        _testerGroups.TryGetValue(groupName, out var set) && !set.IsEmpty;

    /// <summary>Add a connection to a <c>tq:{project}:{tester}</c> fan-out group.</summary>
    public void SubscribeTesterGroup(string groupName, RawSocketConnection connection)
    {
        var set = _testerGroups.GetOrAdd(
            groupName, static _ => new ConcurrentDictionary<string, RawSocketConnection>());
        set[connection.Id] = connection;
    }

    /// <summary>Remove a connection from a single group; retires empty groups.</summary>
    public void UnsubscribeTesterGroup(string groupName, string connectionId)
    {
        if (!_testerGroups.TryGetValue(groupName, out var set))
        {
            return;
        }

        set.TryRemove(connectionId, out _);
        if (set.IsEmpty)
        {
            // Benign race: a concurrent SubscribeTesterGroup may re-create the
            // group; GetOrAdd on the subscribe side makes that safe.
            _testerGroups.TryRemove(new KeyValuePair<string, ConcurrentDictionary<string, RawSocketConnection>>(groupName, set));
        }
    }

    /// <summary>Drop a connection from every tester group (call on disconnect).</summary>
    public void RemoveTesterConnection(RawSocketConnection connection)
    {
        foreach (var kvp in _testerGroups)
        {
            if (kvp.Value.ContainsKey(connection.Id))
            {
                UnsubscribeTesterGroup(kvp.Key, connection.Id);
            }
        }
    }

    /// <summary>
    /// Send a serialized tester-queue message (snapshot/update/phase — the flat
    /// <c>{"type":"..."}</c> JSON) to every raw subscriber of a group. Overflowing
    /// connections are ejected.
    /// </summary>
    public void BroadcastTesterGroup(string groupName, string json)
    {
        if (!_testerGroups.TryGetValue(groupName, out var set))
        {
            return;
        }

        foreach (var connection in set.Values)
        {
            if (!connection.TryEnqueue(json))
            {
                UnsubscribeTesterGroup(groupName, connection.Id);
                _logger.LogWarning(
                    "raw ws: tester client {Conn} ejected from {Group} (send queue overflow)",
                    connection.Id, groupName);
            }
        }
    }
}
