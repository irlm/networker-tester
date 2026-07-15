using System.Net.WebSockets;
using System.Text;
using System.Threading.Channels;

namespace Networker.ControlPlane.Realtime.RawWs;

/// <summary>
/// One raw agent WebSocket — the C# analogue of the Rust
/// <c>handle_agent_socket</c>'s socket half (crates/networker-dashboard/src/ws/
/// agent_hub.rs): a bounded outbound channel + pump task that serialises all
/// writes onto the socket, and a fragment-assembling text receive loop with a
/// server-side idle timeout.
///
/// <para><b>Why the channel pump.</b> <see cref="WebSocket.SendAsync"/> is not
/// safe for concurrent callers, but the dispatcher / watchdog / command API can
/// all push <see cref="ControlMessage"/>s to the same agent concurrently. Every
/// outbound frame therefore goes through a bounded
/// <see cref="Channel{T}"/> (capacity <see cref="OutboundCapacity"/>, the Rust
/// <c>AGENT_CHANNEL_CAPACITY</c>) drained by a single pump task — the exact
/// shape of the Rust <c>mpsc::channel</c> + sink task. A full channel applies
/// backpressure (<see cref="BoundedChannelFullMode.Wait"/>) honouring the
/// caller's CancellationToken.</para>
///
/// <para><b>Ping/pong.</b> Incoming client pings are answered automatically by
/// the managed WebSocket (as tungstenite does agent-side); server keepalive
/// pings are configured at accept time (<see cref="AgentSocketEndpoint"/>).
/// Control frames never surface from <see cref="WebSocket.ReceiveAsync"/>, so
/// the idle timeout below is keyed on data frames only — an agent that sends no
/// frames (its heartbeat cadence is seconds) for <c>idleTimeout</c> is dead.</para>
/// </summary>
public sealed class AgentSocketConnection : IAsyncDisposable
{
    /// <summary>Outbound channel capacity — the Rust <c>AGENT_CHANNEL_CAPACITY</c>.</summary>
    public const int OutboundCapacity = 256;

    /// <summary>
    /// Maximum assembled inbound message size — matches the 64&#160;MiB the Rust
    /// hub allows (<c>ws.max_message_size(64 * 1024 * 1024)</c>); benchmark
    /// artifacts ride in <c>run_finished</c> frames and can be large.
    /// </summary>
    public const int MaxMessageBytes = 64 * 1024 * 1024;

    private readonly WebSocket _socket;
    private readonly ILogger _logger;
    private readonly Channel<string> _outbound;
    private readonly CancellationTokenSource _lifetime;
    private readonly Task _pump;

    /// <summary>
    /// Synthetic connection id registered with
    /// <see cref="AgentConnectionRegistry"/>; the <c>raw-</c> prefix keeps it
    /// disjoint from SignalR connection ids so the compare-and-remove
    /// unregister guard works across both transports.
    /// </summary>
    public string ConnectionId { get; } = $"raw-{Guid.NewGuid():N}";

    public AgentSocketConnection(WebSocket socket, ILogger logger, CancellationToken lifetime = default)
    {
        _socket = socket;
        _logger = logger;
        _outbound = Channel.CreateBounded<string>(new BoundedChannelOptions(OutboundCapacity)
        {
            SingleReader = true,
            SingleWriter = false,
            FullMode = BoundedChannelFullMode.Wait,
        });
        _lifetime = CancellationTokenSource.CreateLinkedTokenSource(lifetime);
        _pump = Task.Run(() => PumpOutboundAsync(_lifetime.Token), CancellationToken.None);
    }

    // ── Outbound ─────────────────────────────────────────────────────────────

    /// <summary>
    /// Enqueue one serialized <c>{"type":"...", ...}</c> envelope to be written
    /// as a WS text frame. This is the sender delegate registered with
    /// <see cref="AgentConnectionRegistry.Register(Guid, string, Func{string, CancellationToken, Task})"/>.
    /// Throws <see cref="ChannelClosedException"/> once the connection is being
    /// torn down — the registry translates any sender failure into its
    /// "not sent" <c>false</c> result.
    /// </summary>
    public async Task SendAsync(string json, CancellationToken ct)
        => await _outbound.Writer.WriteAsync(json, ct);

    /// <summary>
    /// Single-consumer pump: drains the channel and performs the actual
    /// <see cref="WebSocket.SendAsync"/> text writes — the Rust sink task.
    /// Exits when the channel completes (graceful teardown), the lifetime token
    /// fires, or the socket errors (matching the Rust pump's
    /// <c>if send.is_err() break</c>).
    /// </summary>
    private async Task PumpOutboundAsync(CancellationToken ct)
    {
        try
        {
            await foreach (var json in _outbound.Reader.ReadAllAsync(ct))
            {
                var bytes = Encoding.UTF8.GetBytes(json);
                await _socket.SendAsync(bytes, WebSocketMessageType.Text, endOfMessage: true, ct);
            }
        }
        catch (OperationCanceledException)
        {
            // Teardown — expected.
        }
        catch (Exception ex) when (ex is WebSocketException or ObjectDisposedException or InvalidOperationException)
        {
            _logger.LogDebug(ex, "Agent socket {ConnectionId} outbound pump stopped", ConnectionId);
        }
        finally
        {
            // Once the socket can no longer be written, fail fast for senders.
            _outbound.Writer.TryComplete();
        }
    }

    // ── Inbound ──────────────────────────────────────────────────────────────

    /// <summary>
    /// Receive the next complete TEXT message, assembling fragments. Returns
    /// <c>null</c> when the connection is over: peer close frame, socket error,
    /// caller cancellation, or no frame arriving within
    /// <paramref name="idleTimeout"/> (the server-side staleness guard — the
    /// agent heartbeats every few seconds, so ~120s of silence means the peer
    /// is gone even if TCP has not noticed). Binary messages are skipped, like
    /// the Rust inbound pump which only matches <c>Message::Text</c>.
    /// </summary>
    public async Task<string?> ReceiveTextAsync(TimeSpan idleTimeout, CancellationToken ct)
    {
        var buffer = new byte[16 * 1024];
        using var ms = new MemoryStream();

        while (true)
        {
            WebSocketReceiveResult result;
            using var idleCts = CancellationTokenSource.CreateLinkedTokenSource(ct);
            idleCts.CancelAfter(idleTimeout);
            try
            {
                result = await _socket.ReceiveAsync(new ArraySegment<byte>(buffer), idleCts.Token);
            }
            catch (OperationCanceledException) when (!ct.IsCancellationRequested)
            {
                // Idle timeout — note the cancelled ReceiveAsync aborts the
                // managed socket, which is fine: we are closing it anyway.
                _logger.LogWarning(
                    "Agent socket {ConnectionId} idle for {Timeout}s — closing",
                    ConnectionId, (int)idleTimeout.TotalSeconds);
                return null;
            }
            catch (OperationCanceledException)
            {
                return null; // request aborted / app shutdown
            }
            catch (Exception ex) when (ex is WebSocketException or ObjectDisposedException)
            {
                _logger.LogDebug(ex, "Agent socket {ConnectionId} receive failed", ConnectionId);
                return null;
            }

            if (result.MessageType == WebSocketMessageType.Close)
            {
                return null;
            }

            if (ms.Length + result.Count > MaxMessageBytes)
            {
                _logger.LogWarning(
                    "Agent socket {ConnectionId} exceeded {Max} byte message cap — closing",
                    ConnectionId, MaxMessageBytes);
                await CloseAsync(WebSocketCloseStatus.MessageTooBig, "message too big");
                return null;
            }

            ms.Write(buffer, 0, result.Count);
            if (!result.EndOfMessage)
            {
                continue;
            }

            if (result.MessageType == WebSocketMessageType.Text)
            {
                return Encoding.UTF8.GetString(ms.GetBuffer(), 0, (int)ms.Length);
            }

            // Binary (or anything else): drop and keep reading — Rust's
            // `_ => {}` arm.
            ms.SetLength(0);
        }
    }

    // ── Teardown ─────────────────────────────────────────────────────────────

    /// <summary>Best-effort graceful close (ignored if the socket is already dead).</summary>
    public async Task CloseAsync(
        WebSocketCloseStatus status = WebSocketCloseStatus.NormalClosure,
        string? description = null)
    {
        try
        {
            if (_socket.State is WebSocketState.Open or WebSocketState.CloseReceived)
            {
                using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(5));
                await _socket.CloseAsync(status, description, cts.Token);
            }
        }
        catch (Exception ex) when (ex is WebSocketException or ObjectDisposedException or OperationCanceledException or InvalidOperationException)
        {
            // Already aborted — nothing to do.
        }
    }

    /// <summary>
    /// Complete the outbound channel, give the pump a moment to drain, then
    /// cancel it. Idempotent.
    /// </summary>
    public async ValueTask DisposeAsync()
    {
        _outbound.Writer.TryComplete();
        try
        {
            await _pump.WaitAsync(TimeSpan.FromSeconds(2));
        }
        catch (TimeoutException)
        {
            _lifetime.Cancel();
            try
            {
                await _pump;
            }
            catch
            {
                // Pump exceptions were already logged inside the pump.
            }
        }
        _lifetime.Dispose();
    }
}
