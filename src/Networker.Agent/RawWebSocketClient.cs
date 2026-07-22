using System.Net.WebSockets;
using System.Text;
using System.Text.Json;
using System.Threading.Channels;

namespace Networker.Agent;

/// <summary>
/// Raw-WebSocket transport to the control plane — the C# replacement for the
/// Rust <c>ws_client::run</c> (crates/networker-agent/src/ws_client.rs). Speaks
/// plain text frames against <c>/ws/agent?key={apiKey}</c> (the M6 raw-WS
/// endpoint), NOT SignalR.
///
/// Design mirrors the Rust client:
///   * One connection per <see cref="RunOnceAsync"/> call; the caller
///     (AgentWorker) wraps it in a reconnect loop with back-off, exactly like
///     the Rust <c>main.rs</c> <c>loop { select! { ws_client::run(..) } }</c>.
///   * Single-writer send discipline: <see cref="ClientWebSocket.SendAsync"/>
///     is NOT concurrent-safe, so all outbound frames funnel through a bounded
///     <see cref="Channel{T}"/> drained by one send pump — the analogue of the
///     Rust <c>mpsc</c> + <c>sink_handle</c> forwarding task.
///   * The receive loop decodes each text frame into a <see cref="ControlMessage"/>
///     and hands it to a dispatcher callback; unparseable frames are ignored
///     (Rust: <c>if let Ok(ctrl) = decode(...)</c>).
/// </summary>
public sealed class RawWebSocketClient
{
    private const int WsChannelCapacity = 4096;
    private const int ReceiveBufferBytes = 64 * 1024;

    /// <summary>Request header carrying the agent api-key — must match the control
    /// plane's <c>AgentSocketEndpoint.ApiKeyHeader</c>. Hyphenated so nginx (which
    /// drops underscore headers by default) forwards it to the upstream.</summary>
    public const string ApiKeyHeader = "X-LagHound-Agent-Key";

    private readonly ILogger _logger;

    public RawWebSocketClient(ILogger logger) => _logger = logger;

    /// <summary>Bounded outbound-frame sink handed to run/command executors. All
    /// writes go through here; the send pump is the only WS writer.</summary>
    public interface IFrameSink
    {
        /// <summary>Enqueue a serialized <see cref="AgentMessage"/> for sending.
        /// Non-blocking; drops with a logged warning if the queue is full or the
        /// connection is gone (Rust: <c>try_send</c> logs "channel full or closed").</summary>
        bool TrySend(AgentMessage message);
    }

    /// <summary>
    /// Connect, pump, and process one connection until it drops. Returns when
    /// the socket closes (normally or on error) — the caller reconnects.
    /// </summary>
    /// <param name="dashboardUrl">Base WS URL, e.g. <c>ws://host:3000/ws/agent</c>.</param>
    /// <param name="apiKey">Agent api-key, sent in the <see cref="ApiKeyHeader"/>
    /// request header so it never appears in the URL / proxy access log.</param>
    /// <param name="onControl">Dispatcher for each decoded inbound frame. Given
    /// the message + the outbound sink so handlers can stream replies.</param>
    /// <param name="onConnected">Invoked once the socket is open + the pump is
    /// running, before the receive loop starts (used to launch the heartbeat).</param>
    public async Task RunOnceAsync(
        string dashboardUrl,
        string apiKey,
        Func<ControlMessage, IFrameSink, CancellationToken, Task> onControl,
        Func<IFrameSink, CancellationToken, Task>? onConnected,
        CancellationToken cancellationToken)
    {
        // The api-key travels in a header, not the URL query, so it never lands
        // in the proxy access log. The control plane accepts the header and (until
        // the Rust-agent decommission) still falls back to the legacy ?key=.
        var uri = new Uri(dashboardUrl);
        _logger.LogInformation("Connecting to {DashboardUrl}", dashboardUrl);

        using var socket = new ClientWebSocket();
        socket.Options.SetRequestHeader(ApiKeyHeader, apiKey);
        await socket.ConnectAsync(uri, cancellationToken).ConfigureAwait(false);
        _logger.LogInformation("Connected to dashboard (raw WS v2)");

        // Link a per-connection CTS: when the receive loop ends (close/error)
        // we cancel the pump + any onConnected task (heartbeat), matching the
        // Rust `heartbeat_handle.abort()` / `sink_handle.abort()` on disconnect.
        using var connCts = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        var connToken = connCts.Token;

        var channel = Channel.CreateBounded<string>(new BoundedChannelOptions(WsChannelCapacity)
        {
            FullMode = BoundedChannelFullMode.DropWrite,
            SingleReader = true,
        });
        var sink = new ChannelFrameSink(channel.Writer, _logger);

        var sendPump = Task.Run(() => SendPumpAsync(socket, channel.Reader, connToken), CancellationToken.None);
        var onConnectedTask = onConnected is null
            ? Task.CompletedTask
            : Task.Run(() => onConnected(sink, connToken), CancellationToken.None);

        try
        {
            await ReceiveLoopAsync(socket, sink, onControl, connToken).ConfigureAwait(false);
        }
        finally
        {
            // Stop the pump + heartbeat, drain, and best-effort close the socket.
            channel.Writer.TryComplete();
            connCts.Cancel();
            try { await sendPump.ConfigureAwait(false); } catch { /* pump cancelled */ }
            try { await onConnectedTask.ConfigureAwait(false); } catch { /* heartbeat cancelled */ }
            await TryCloseAsync(socket).ConfigureAwait(false);
        }
    }

    private async Task ReceiveLoopAsync(
        ClientWebSocket socket,
        IFrameSink sink,
        Func<ControlMessage, IFrameSink, CancellationToken, Task> onControl,
        CancellationToken token)
    {
        var buffer = new byte[ReceiveBufferBytes];
        var accumulator = new MemoryStream();

        while (!token.IsCancellationRequested && socket.State == WebSocketState.Open)
        {
            WebSocketReceiveResult result;
            accumulator.SetLength(0);
            do
            {
                result = await socket.ReceiveAsync(new ArraySegment<byte>(buffer), token)
                    .ConfigureAwait(false);
                if (result.MessageType == WebSocketMessageType.Close)
                {
                    _logger.LogInformation("Server closed connection");
                    return;
                }
                accumulator.Write(buffer, 0, result.Count);
            }
            while (!result.EndOfMessage);

            if (result.MessageType != WebSocketMessageType.Text)
                continue; // ignore binary frames (Rust handles only Text)

            var text = Encoding.UTF8.GetString(accumulator.GetBuffer(), 0, (int)accumulator.Length);
            ControlMessage? ctrl;
            try
            {
                ctrl = JsonSerializer.Deserialize<ControlMessage>(text, AgentProtocolJson.Options);
            }
            catch (JsonException)
            {
                // Rust: `if let Ok(ctrl) = decode(...)` — unparseable frames are
                // silently ignored. Trace it so operators can debug drift.
                _logger.LogTrace("Ignoring undecodable control frame: {Frame}", Truncate(text, 256));
                continue;
            }

            if (ctrl is null)
                continue;

            await onControl(ctrl, sink, token).ConfigureAwait(false);
        }
    }

    private async Task SendPumpAsync(
        ClientWebSocket socket,
        ChannelReader<string> reader,
        CancellationToken token)
    {
        try
        {
            await foreach (var text in reader.ReadAllAsync(token).ConfigureAwait(false))
            {
                if (socket.State != WebSocketState.Open)
                    break;
                var bytes = Encoding.UTF8.GetBytes(text);
                await socket.SendAsync(
                    new ArraySegment<byte>(bytes),
                    WebSocketMessageType.Text,
                    endOfMessage: true,
                    token).ConfigureAwait(false);
            }
        }
        catch (OperationCanceledException)
        {
            // Connection tearing down — Rust drops the sink task on disconnect.
        }
        catch (WebSocketException ex)
        {
            _logger.LogDebug(ex, "Send pump stopped: socket error");
        }
    }

    private static async Task TryCloseAsync(ClientWebSocket socket)
    {
        try
        {
            if (socket.State is WebSocketState.Open or WebSocketState.CloseReceived)
            {
                using var closeCts = new CancellationTokenSource(TimeSpan.FromSeconds(2));
                await socket.CloseAsync(
                    WebSocketCloseStatus.NormalClosure, "bye", closeCts.Token).ConfigureAwait(false);
            }
        }
        catch
        {
            // Best-effort close; the socket is disposed by the caller regardless.
        }
    }

    // LEGACY transport (no longer used by RunOnceAsync — the key now travels in
    // the ApiKeyHeader). Retained only as the reference for the ?key= form the
    // control plane still accepts as a fallback for fielded pre-header agents,
    // pinned by ConfigAndArgsTests until the Rust-agent decommission.
    internal static Uri BuildUri(string dashboardUrl, string apiKey)
    {
        // Rust: format!("{}?key={}", cfg.dashboard_url, cfg.api_key) — a naive
        // concat. We URL-encode the key (a valid superset: a key with no special
        // chars is byte-identical to the Rust concat) and pick ? or & based on
        // whether the URL already carries a query string.
        var sep = dashboardUrl.Contains('?') ? '&' : '?';
        return new Uri($"{dashboardUrl}{sep}key={Uri.EscapeDataString(apiKey)}");
    }

    private static string Truncate(string s, int max) =>
        s.Length <= max ? s : s[..max];

    /// <summary>Channel-backed frame sink: serialises + enqueues; the single
    /// send pump is the only writer to the socket.</summary>
    private sealed class ChannelFrameSink(ChannelWriter<string> writer, ILogger logger) : IFrameSink
    {
        public bool TrySend(AgentMessage message)
        {
            string text;
            try
            {
                text = JsonSerializer.Serialize(message, AgentProtocolJson.Options);
            }
            catch (Exception ex)
            {
                logger.LogWarning(ex, "Failed to encode {Type}", message.GetType().Name);
                return false;
            }

            if (!writer.TryWrite(text))
            {
                logger.LogError("Failed to send message: channel full or closed");
                return false;
            }

            return true;
        }
    }
}
