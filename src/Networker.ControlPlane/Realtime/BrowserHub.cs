using Microsoft.AspNetCore.Authorization;
using Microsoft.AspNetCore.SignalR;

namespace Networker.ControlPlane.Realtime;

/// <summary>
/// Browser-facing live-updates hub — the C# re-architecture of the Rust
/// <c>ws/browser_hub.rs</c>. Mapped (by Program.cs) at <c>/ws/dashboard</c>.
///
/// <para><b>Connection lifecycle</b> (mirrors Rust):</para>
/// <list type="number">
///   <item>Client connects with <c>?access_token=&lt;jwt&gt;[&amp;since=&lt;seq&gt;]</c>.
///     (Rust used <c>?token=</c>; SignalR's JS client sends the JWT as
///     <c>access_token</c> by convention — see the JwtBearer integration note
///     on <see cref="EventBusServiceCollectionExtensions"/>.)</item>
///   <item>The hub requires authentication — an invalid/absent token is rejected
///     by the JwtBearer middleware before <see cref="OnConnectedAsync"/> runs
///     (401 on negotiate), so no anonymous browser ever tails the feed.</item>
///   <item>On connect, if <c>since &gt; 0</c>, the hub reads the replay batch
///     (<c>EventBus.Replay(since)</c>) and sends every buffered event with
///     <c>seq &gt; since</c> to THIS caller so it catches up on what it missed
///     during the disconnect.</item>
///   <item>Live events arrive via the EventBus broadcast to all connected
///     clients (<c>IHubContext&lt;BrowserHub&gt;.Clients.All</c>).</item>
/// </list>
///
/// <para><b>Replay / dedup contract</b> (identical to Rust): the client tracks
/// the maximum <c>seq</c> it has applied. Because the connection is already in
/// <c>Clients.All</c> by the time the replay batch is sent, an event published
/// during the small replay window can arrive on BOTH paths; the client drops
/// any incoming event whose <c>seq</c> is <c>&lt;= maxAppliedSeq</c>, so each
/// event is applied exactly once. This is why events carry <c>seq</c>: the
/// replay batch and the live tail are reconciled purely by sequence number.</para>
///
/// <para><b>Lag recovery</b>: if the client detects a gap (next live <c>seq</c>
/// jumps by more than 1), it reconnects with <c>?since=&lt;lastAppliedSeq&gt;</c>
/// to trigger a fresh replay of the missed window from the ring buffer.</para>
///
/// <para>The single client-side receive method is named <c>"event"</c>; each
/// message is a flat <see cref="SeqEvent"/>:
/// <c>{"seq":N,"type":"...", ...fields}</c>.</para>
/// </summary>
[Authorize]
public sealed class BrowserHub : Hub
{
    /// <summary>The SignalR client method invoked for every dashboard event.</summary>
    public const string ClientReceiveMethod = "event";

    /// <summary>Query-string key carrying the last-seen seq for replay-on-reconnect.</summary>
    public const string SinceQueryKey = "since";

    private readonly EventBus _bus;
    private readonly ILogger<BrowserHub> _logger;

    public BrowserHub(EventBus bus, ILogger<BrowserHub> logger)
    {
        _bus = bus;
        _logger = logger;
    }

    public override async Task OnConnectedAsync()
    {
        var since = ReadSince();

        if (since > 0)
        {
            // Catch the client up on events it missed while disconnected. The
            // client dedups by tracking max applied seq, so any overlap with the
            // live tail during this window is harmless.
            var replay = _bus.Replay(since);
            _logger.LogInformation(
                "Browser {ConnId} connected with since={Since}; replaying {Count} buffered event(s) (head={Head})",
                Context.ConnectionId, since, replay.Count, _bus.CurrentSeq());

            foreach (var seqEvent in replay)
            {
                // Send only to the reconnecting caller — the rest of the fleet
                // already saw these live.
                await Clients.Caller.SendAsync(ClientReceiveMethod, seqEvent, Context.ConnectionAborted);
            }
        }
        else
        {
            _logger.LogInformation(
                "Browser {ConnId} connected (no replay; since<=0, head={Head})",
                Context.ConnectionId, _bus.CurrentSeq());
        }

        await base.OnConnectedAsync();
    }

    public override Task OnDisconnectedAsync(Exception? exception)
    {
        if (exception is not null)
        {
            _logger.LogDebug(exception, "Browser {ConnId} disconnected with error", Context.ConnectionId);
        }
        else
        {
            _logger.LogDebug("Browser {ConnId} disconnected", Context.ConnectionId);
        }
        return base.OnDisconnectedAsync(exception);
    }

    /// <summary>
    /// Parse the <c>?since=&lt;seq&gt;</c> query parameter. Absent, empty, or
    /// unparseable → 0 (no replay), matching Rust's <c>since.unwrap_or(0)</c>.
    /// Negative values are clamped to 0.
    /// </summary>
    private long ReadSince()
    {
        var http = Context.GetHttpContext();
        if (http is null)
        {
            return 0;
        }

        var raw = http.Request.Query[SinceQueryKey];
        if (raw.Count == 0)
        {
            return 0;
        }

        return long.TryParse(raw.ToString(), out var since) && since > 0 ? since : 0;
    }
}
