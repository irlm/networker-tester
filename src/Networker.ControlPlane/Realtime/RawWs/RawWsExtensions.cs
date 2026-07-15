using System.Net.WebSockets;
using System.Security.Claims;
using System.Text;
using Microsoft.AspNetCore.SignalR;
using Networker.ControlPlane.Auth;

namespace Networker.ControlPlane.Realtime.RawWs;

/// <summary>
/// Raw-WebSocket bridge for the React frontend (Phase-2 M6 cutover blocker).
///
/// <para>The frontend does NOT speak SignalR: <c>dashboard/src/hooks/useWebSocket.ts</c>
/// opens <c>new WebSocket("/ws/dashboard?token=&lt;jwt&gt;[&amp;project_id=..][&amp;since=..]")</c>
/// and <c>JSON.parse</c>s each text frame; <c>useTesterSubscription.ts</c> /
/// <c>usePhaseSubscription.ts</c> open <c>/ws/testers?token=&lt;jwt&gt;</c> and exchange
/// flat <c>{"type":"..."}</c> JSON text frames. These endpoints serve that exact
/// wire contract (the Rust <c>ws/browser_hub.rs</c> + <c>ws/tester_hub.rs</c>
/// behavior) on top of the existing C# services (EventBus, TesterQueueRegistry,
/// AuthRepository, NetworkerDbContext), so the frontend connects unmodified.</para>
///
/// <para><b>Program.cs wiring (REQUIRED — done by the integrator; these files
/// deliberately do not touch Program.cs):</b></para>
/// <code>
/// // 1. Services — AFTER AddSignalR() and AddNetworkerAuth(...):
/// //    (order matters: AddRawWebSockets replaces SignalR's
/// //    HubLifetimeManager&lt;TesterQueueHub&gt; with a mirroring decorator, so it
/// //    must be registered after AddSignalR's open-generic default.)
/// builder.Services.AddRawWebSockets();
///
/// // 2. Pipeline — enable the WebSocket middleware (before the endpoints),
/// //    then map the raw endpoints:
/// app.UseWebSockets();
/// app.MapRawWebSockets();
///
/// // 3. REMAP the SignalR hubs off /ws/* — the raw endpoints now own the
/// //    paths the browser dials. SignalR (native C# clients / future frontend)
/// //    moves to /hub/*:
/// app.MapHub&lt;BrowserHub&gt;("/hub/dashboard");      // was /ws/dashboard
/// app.MapHub&lt;TesterQueueHub&gt;("/hub/testers");    // was /ws/testers
/// // /ws/agent (AgentProtocolHub) is unaffected — agents speak SignalR.
/// </code>
///
/// <para><b>Fan-out paths:</b> browser events flow straight from
/// <see cref="EventBus.Publish"/> into <see cref="RawSocketRegistry"/> (EventBus
/// takes an optional registry). Tester-queue traffic is produced through
/// <c>IHubContext&lt;TesterQueueHub&gt;</c> group sends (TesterQueueBroadcaster),
/// which flow through <see cref="HubLifetimeManager{THub}"/> — the decorator
/// registered here (<see cref="RawWsTesterQueueLifetimeManager"/>) mirrors every
/// <c>tq:*</c> group send to the raw subscribers as the bare JSON payload,
/// without editing the hub or broadcaster.</para>
/// </summary>
public static class RawWsExtensions
{
    /// <summary>Raw browser event feed path — what useWebSocket.ts dials.</summary>
    public const string BrowserPath = "/ws/dashboard";

    /// <summary>Raw tester-queue feed path — what useTesterSubscription.ts dials.</summary>
    public const string TesterPath = "/ws/testers";

    /// <summary>
    /// Register the raw-WS singletons: the <see cref="RawSocketRegistry"/> and
    /// the <see cref="HubLifetimeManager{THub}"/> decorator that mirrors
    /// TesterQueueHub group sends to raw sockets. MUST be called after
    /// <c>AddSignalR()</c> (the closed-generic registration here supersedes
    /// SignalR's open-generic default for TesterQueueHub only).
    /// </summary>
    public static IServiceCollection AddRawWebSockets(this IServiceCollection services)
    {
        services.AddSingleton<RawSocketRegistry>();

        services.AddSingleton<HubLifetimeManager<TesterQueueHub>>(sp =>
            new RawWsTesterQueueLifetimeManager(
                ActivatorUtilities.CreateInstance<DefaultHubLifetimeManager<TesterQueueHub>>(sp),
                sp.GetRequiredService<RawSocketRegistry>()));

        return services;
    }

    /// <summary>
    /// Map the raw-WebSocket endpoints at <see cref="BrowserPath"/> and
    /// <see cref="TesterPath"/>. Requires <c>app.UseWebSockets()</c> earlier in
    /// the pipeline (without it every request reports non-WebSocket → 400) and
    /// the SignalR hubs remapped off these paths (see class remarks).
    /// </summary>
    public static WebApplication MapRawWebSockets(this WebApplication app)
    {
        app.MapGet(BrowserPath, BrowserSocketEndpoint.HandleAsync);
        app.MapGet(TesterPath, TesterQueueSocketEndpoint.HandleAsync);
        return app;
    }
}

/// <summary>
/// Shared plumbing for the two raw-WS endpoints: query-string JWT auth
/// (matching the Rust hubs' <c>?token=</c>, plus <c>access_token</c> for
/// SignalR-convention compatibility) and framed text send/receive helpers.
/// </summary>
internal static class RawWsIo
{
    /// <summary>Max inbound text message size (subscribe frames are tiny).</summary>
    internal const int MaxInboundMessageBytes = 64 * 1024;

    /// <summary>
    /// Authenticate the upgrade request from the query string. Accepts
    /// <c>?token=</c> (what the React hooks send — same as the Rust hubs) and
    /// falls back to <c>?access_token=</c>. Returns the validated principal or
    /// null (missing/empty/invalid token) — callers respond 401, exactly like
    /// the Rust handlers' pre-upgrade rejection.
    /// </summary>
    internal static ClaimsPrincipal? Authenticate(HttpContext context)
    {
        var token = context.Request.Query["token"].ToString();
        if (string.IsNullOrEmpty(token))
        {
            token = context.Request.Query["access_token"].ToString();
        }

        if (string.IsNullOrEmpty(token))
        {
            return null;
        }

        var tokens = context.RequestServices.GetRequiredService<JwtTokenService>();
        return tokens.Validate(token);
    }

    /// <summary>Send one UTF-8 text frame (single, non-fragmented message).</summary>
    internal static Task SendTextAsync(WebSocket socket, string json, CancellationToken ct)
    {
        var bytes = Encoding.UTF8.GetBytes(json);
        return socket.SendAsync(bytes, WebSocketMessageType.Text, endOfMessage: true, ct);
    }

    /// <summary>
    /// Receive the next complete TEXT message (reassembling fragments). Returns
    /// null when the socket closes, errors, or the message exceeds
    /// <see cref="MaxInboundMessageBytes"/>. Binary messages are drained and
    /// skipped (the Rust hubs ignore them too).
    /// </summary>
    internal static async Task<string?> ReceiveTextMessageAsync(WebSocket socket, CancellationToken ct)
    {
        var buffer = new byte[4096];
        using var accumulated = new MemoryStream();

        while (true)
        {
            accumulated.SetLength(0);
            var isText = true;

            while (true)
            {
                ValueWebSocketReceiveResult result;
                try
                {
                    result = await socket.ReceiveAsync(buffer.AsMemory(), ct).ConfigureAwait(false);
                }
                catch (OperationCanceledException)
                {
                    return null;
                }
                catch (WebSocketException)
                {
                    return null;
                }

                if (result.MessageType == WebSocketMessageType.Close)
                {
                    return null;
                }

                if (result.MessageType == WebSocketMessageType.Binary)
                {
                    isText = false;
                }

                if (isText)
                {
                    accumulated.Write(buffer, 0, result.Count);
                    if (accumulated.Length > MaxInboundMessageBytes)
                    {
                        return null; // pathological client — endpoint tears down
                    }
                }

                if (result.EndOfMessage)
                {
                    break;
                }
            }

            if (isText && accumulated.Length > 0)
            {
                return Encoding.UTF8.GetString(accumulated.GetBuffer(), 0, (int)accumulated.Length);
            }
            // Empty text frame or binary message → keep reading.
        }
    }

    /// <summary>
    /// Wait for the peer to close (or the connection to abort), discarding all
    /// inbound frames — the browser feed is outbound-only; protocol-level
    /// ping/pong is handled by the WebSocket implementation itself.
    /// </summary>
    internal static async Task PumpInboundUntilClosedAsync(WebSocket socket, CancellationToken ct)
    {
        var buffer = new byte[1024];
        while (socket.State == WebSocketState.Open || socket.State == WebSocketState.CloseSent)
        {
            ValueWebSocketReceiveResult result;
            try
            {
                result = await socket.ReceiveAsync(buffer.AsMemory(), ct).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                return;
            }
            catch (WebSocketException)
            {
                return;
            }

            if (result.MessageType == WebSocketMessageType.Close)
            {
                return;
            }
        }
    }

    /// <summary>Best-effort graceful close; falls back to abort semantics.</summary>
    internal static async Task TryCloseAsync(WebSocket socket)
    {
        if (socket.State is WebSocketState.Open or WebSocketState.CloseReceived)
        {
            try
            {
                using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(5));
                await socket
                    .CloseAsync(WebSocketCloseStatus.NormalClosure, null, cts.Token)
                    .ConfigureAwait(false);
            }
            catch
            {
                // The peer is gone — nothing further to do.
            }
        }
    }

    /// <summary>Abort a socket, swallowing already-disposed races.</summary>
    internal static void SafeAbort(WebSocket socket)
    {
        try
        {
            socket.Abort();
        }
        catch
        {
            // Disposed concurrently — fine.
        }
    }
}
