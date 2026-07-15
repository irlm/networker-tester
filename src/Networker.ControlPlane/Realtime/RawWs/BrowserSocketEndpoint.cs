using System.Text.Json;

namespace Networker.ControlPlane.Realtime.RawWs;

/// <summary>
/// Raw-WebSocket browser event feed at <c>/ws/dashboard</c> — the endpoint the
/// React <c>useWebSocket.ts</c> hook actually dials. Byte-for-byte the Rust
/// <c>ws/browser_hub.rs</c> contract:
///
/// <list type="number">
///   <item><b>Auth before upgrade</b>: <c>?token=&lt;jwt&gt;</c> (also accepts
///     <c>access_token</c>); missing/invalid → 401, non-WebSocket request → 400.
///     The hook additionally sends <c>project_id</c> — ignored, exactly as the
///     Rust handler's query struct ignores it.</item>
///   <item><b>Replay</b>: <c>?since=&lt;seq&gt;</c> (sent only on re-connects) →
///     every buffered <see cref="SeqEvent"/> with <c>seq &gt; since</c> is sent
///     as an individual text frame, oldest first, BEFORE the live tail.</item>
///   <item><b>Live tail</b>: each published event arrives as one flat JSON text
///     frame <c>{"seq":N,"type":"job_update",...}</c> (the
///     <see cref="SeqEventJsonConverter"/> shape — identical to the SignalR
///     payload, minus the SignalR envelope).</item>
///   <item><b>Dedup</b>: the connection registers for live fan-out BEFORE the
///     replay snapshot is taken (no gap), and the send pump skips live frames
///     with <c>seq &lt;= maxReplayedSeq</c> (no duplicates) — the Rust
///     subscribe-then-replay + <c>seq &gt; max_replayed</c> filter.</item>
///   <item><b>Slow subscriber</b>: a full send queue ejects the socket; the
///     client's reconnect-with-<c>since</c> loop resyncs it (Rust: broadcast
///     lag → client resync).</item>
/// </list>
/// </summary>
public static class BrowserSocketEndpoint
{
    /// <summary>Query key for the replay cursor (matches Rust + useWebSocket.ts).</summary>
    public const string SinceQueryKey = "since";

    public static async Task HandleAsync(HttpContext context)
    {
        if (!context.WebSockets.IsWebSocketRequest)
        {
            context.Response.StatusCode = StatusCodes.Status400BadRequest;
            await context.Response.WriteAsync("WebSocket upgrade required");
            return;
        }

        var principal = RawWsIo.Authenticate(context);
        if (principal is null)
        {
            context.Response.StatusCode = StatusCodes.Status401Unauthorized;
            return;
        }

        var since = ReadSince(context);
        var bus = context.RequestServices.GetRequiredService<EventBus>();
        var registry = context.RequestServices.GetRequiredService<RawSocketRegistry>();
        var logger = context.RequestServices
            .GetRequiredService<ILoggerFactory>()
            .CreateLogger("Networker.ControlPlane.Realtime.RawWs.BrowserSocketEndpoint");

        using var socket = await context.WebSockets.AcceptWebSocketAsync();
        var aborted = context.RequestAborted;

        var connection = new RawSocketConnection(
            $"raw-browser-{Guid.NewGuid():N}",
            (json, ct) => RawWsIo.SendTextAsync(socket, json, ct),
            onDropped: _ => RawWsIo.SafeAbort(socket));

        // Register for live fan-out FIRST — events published from here on buffer
        // in the bounded channel (the pump is not running yet), so nothing falls
        // between the replay snapshot and the live tail.
        registry.RegisterBrowser(connection);
        try
        {
            // Replay: flush the missed window directly on the socket. Safe —
            // the pump has not started, so this is the only sender.
            var maxReplayed = since;
            if (since > 0)
            {
                var replay = bus.Replay(since);
                logger.LogInformation(
                    "raw ws: browser {Conn} connected with since={Since}; replaying {Count} event(s) (head={Head})",
                    connection.Id, since, replay.Count, bus.CurrentSeq());

                foreach (var seqEvent in replay)
                {
                    // Same serializer the live path uses (SeqEventJsonConverter):
                    // {"seq":N,"type":"...",...} — one event per text frame.
                    await RawWsIo.SendTextAsync(socket, JsonSerializer.Serialize(seqEvent), aborted);
                    if (seqEvent.Seq > maxReplayed)
                    {
                        maxReplayed = seqEvent.Seq;
                    }
                }
            }
            else
            {
                logger.LogInformation(
                    "raw ws: browser {Conn} connected (no replay; head={Head})",
                    connection.Id, bus.CurrentSeq());
            }

            // Anything the fan-out buffered during replay with seq <= maxReplayed
            // was already delivered above — the pump discards it.
            connection.SetReplayWatermark(maxReplayed);

            using var pumpCts = CancellationTokenSource.CreateLinkedTokenSource(aborted);
            var pump = connection.RunSendPumpAsync(pumpCts.Token);

            // Hold the socket open until the browser closes (inbound frames are
            // ignored — the feed is outbound-only, like the Rust hub).
            await RawWsIo.PumpInboundUntilClosedAsync(socket, aborted);

            pumpCts.Cancel();
            await pump;
            await RawWsIo.TryCloseAsync(socket);
        }
        finally
        {
            registry.UnregisterBrowser(connection);
            connection.CompleteQueue();
            logger.LogDebug("raw ws: browser {Conn} disconnected", connection.Id);
        }
    }

    /// <summary>
    /// Parse <c>?since=</c>: absent/empty/unparseable/negative → 0 (no replay),
    /// matching the Rust <c>since.unwrap_or(0)</c>.
    /// </summary>
    private static long ReadSince(HttpContext context)
    {
        var raw = context.Request.Query[SinceQueryKey].ToString();
        return long.TryParse(raw, out var since) && since > 0 ? since : 0;
    }
}
