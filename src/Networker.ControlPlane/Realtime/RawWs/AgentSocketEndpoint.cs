using System.Net.WebSockets;

namespace Networker.ControlPlane.Realtime.RawWs;

/// <summary>
/// GET <c>/ws/agent?key=&lt;api_key&gt;</c> — the raw (non-SignalR) WebSocket
/// agent endpoint, byte-compatible with the Rust
/// <c>agent_ws_handler</c>/<c>handle_agent_socket</c> pair
/// (crates/networker-dashboard/src/ws/agent_hub.rs). Fielded Rust agents speak
/// plain tokio-tungstenite text frames (<c>{url}?key={api_key}</c> →
/// <c>AgentMessage</c> out / <c>ControlMessage</c> in), NOT the SignalR
/// handshake — this endpoint lets those unmodified agents connect to the C#
/// control plane (Phase-2 M6 cutover).
///
/// <para><b>Flow (Rust parity):</b></para>
/// <list type="number">
///   <item>Must be a WebSocket upgrade request → otherwise 400.</item>
///   <item>Validate <c>?key=</c> against <c>agent.api_key</c> BEFORE the
///   upgrade → missing/unknown key gets HTTP 401, exactly like the axum
///   handler's <c>ok_or(StatusCode::UNAUTHORIZED)</c>.</item>
///   <item>Accept; mark agent online + publish <c>AgentStatus(online)</c>;
///   register the connection (raw sender) in
///   <see cref="AgentConnectionRegistry"/>; send the
///   <c>{"type":"welcome","agent_id":...,"agent_name":...}</c> frame.</item>
///   <item>Receive loop: each text frame →
///   <see cref="AgentMessageProcessor.HandleFrameAsync"/> in a fresh DI scope
///   (the scoped <c>NetworkerDbContext</c> must not live for the whole
///   connection). A ~120s idle timeout closes dead peers.</item>
///   <item>On close/error: unregister (compare-and-remove), mark offline +
///   publish <c>AgentStatus(offline)</c> + fail orphaned runs — the same
///   <see cref="AgentMessageProcessor.HandleDisconnectAsync"/> the SignalR
///   hub's <c>OnDisconnectedAsync</c> runs.</item>
/// </list>
/// </summary>
public static class AgentSocketEndpoint
{
    /// <summary>Route the fielded Rust agents dial (see <c>ws_client.rs</c>).</summary>
    public const string Path = "/ws/agent";

    /// <summary>Query-string key carrying the agent api-key (Rust: <c>?key=</c>).
    /// Legacy transport — kept for fielded agents that predate the header; the
    /// secret lands in the URL (nginx access log) so it is redacted at the proxy
    /// and removed entirely at the Rust-agent decommission.</summary>
    public const string ApiKeyQueryKey = "key";

    /// <summary>Preferred transport: the agent api-key in a request header, so it
    /// never appears in the URL / access log. Hyphenated (nginx drops
    /// underscore headers by default). New agents send this; the server accepts
    /// either.</summary>
    public const string ApiKeyHeader = "X-LagHound-Agent-Key";

    /// <summary>
    /// Server-side staleness guard: no inbound data frame for this long →
    /// close. The agent heartbeats every few seconds, so 120s of silence (the
    /// same window the stale-run watchdog uses) means the peer is gone even if
    /// TCP has not noticed.
    /// </summary>
    public static readonly TimeSpan IdleTimeout = TimeSpan.FromSeconds(120);

    /// <summary>Server → agent WS keepalive ping cadence.</summary>
    public static readonly TimeSpan KeepAliveInterval = TimeSpan.FromSeconds(30);

    /// <summary>Endpoint handler — mapped by <see cref="AgentSocketExtensions.MapAgentRawSocket"/>.</summary>
    public static async Task HandleAsync(HttpContext context)
    {
        var logger = context.RequestServices
            .GetRequiredService<ILoggerFactory>()
            .CreateLogger(typeof(AgentSocketEndpoint).FullName!);

        if (!context.WebSockets.IsWebSocketRequest)
        {
            context.Response.StatusCode = StatusCodes.Status400BadRequest;
            await context.Response.WriteAsync("websocket upgrade required");
            return;
        }

        // ── Authenticate BEFORE the upgrade (Rust returns 401 from the
        //    upgrade handler). The DI scope is per-step: the scoped DbContext
        //    must never span the socket's lifetime.
        var apiKey = ResolveAgentApiKey(context);
        var scopeFactory = context.RequestServices.GetRequiredService<IServiceScopeFactory>();
        var limiter = context.RequestServices.GetRequiredService<AgentAuthLimiter>();
        var remoteIp = ResolveClientIp(context);

        // Per-IP brute-force short-circuit (V044): a source IP that has flooded
        // failed keys gets 429 for the cooldown window before any DB work.
        if (limiter.IsBlocked(remoteIp))
        {
            logger.LogWarning(
                "Raw agent connection from {Ip} short-circuited: too many failed auth attempts", remoteIp);
            context.Response.StatusCode = StatusCodes.Status429TooManyRequests;
            return;
        }

        AgentIdentity? identity;
        using (var authScope = scopeFactory.CreateScope())
        {
            identity = await GetProcessor(authScope)
                .AuthenticateAsync(apiKey, context.RequestAborted);
        }

        if (identity is null)
        {
            limiter.RecordFailure(remoteIp);
            logger.LogWarning(
                "Raw agent connection rejected: {Reason}",
                string.IsNullOrEmpty(apiKey) ? "no api key" : "unknown or expired api key");
            context.Response.StatusCode = StatusCodes.Status401Unauthorized;
            return;
        }

        // Successful auth: clear the IP's failure history and stamp last-used
        // (throttled). Both are best-effort and never block the connection.
        limiter.RecordSuccess(remoteIp);
        using (var stampScope = scopeFactory.CreateScope())
        {
            await GetProcessor(stampScope)
                .StampApiKeyUsedAsync(identity.AgentId, remoteIp, context.RequestAborted);
        }

        using var socket = await context.WebSockets.AcceptWebSocketAsync(new WebSocketAcceptContext
        {
            KeepAliveInterval = KeepAliveInterval,
        });

        var registry = context.RequestServices.GetRequiredService<AgentConnectionRegistry>();
        await using var conn = new AgentSocketConnection(socket, logger, context.RequestAborted);

        logger.LogInformation(
            "Agent connected (raw WS): {AgentId} name={Name} conn={ConnId}",
            identity.AgentId, identity.Name, conn.ConnectionId);

        try
        {
            // Mark online + publish AgentStatus(online) — Rust order: status
            // update, register, welcome.
            using (var scope = scopeFactory.CreateScope())
            {
                await GetProcessor(scope)
                    .HandleConnectAsync(identity.AgentId, context.RequestAborted);
            }

            // Register the raw sender so the dispatcher's outbound API
            // (AssignRunAsync / SendCommandAsync / ...) reaches this socket.
            registry.Register(identity.AgentId, conn.ConnectionId, conn.SendAsync);

            // Welcome frame — the exact ControlMessage shape from AgentProtocol.cs.
            await conn.SendAsync(
                AgentMessageProcessor.WelcomeFrame(identity.AgentId, identity.Name),
                context.RequestAborted);

            // ── Inbound pump: one fresh DI scope per frame.
            while (await conn.ReceiveTextAsync(IdleTimeout, context.RequestAborted)
                   is { } frame)
            {
                using var scope = scopeFactory.CreateScope();
                await GetProcessor(scope).HandleFrameAsync(identity.AgentId, frame);
            }
        }
        catch (Exception ex) when (ex is not OperationCanceledException)
        {
            logger.LogError(ex,
                "Agent socket {ConnId} for {AgentId} failed", conn.ConnectionId, identity.AgentId);
        }
        finally
        {
            // Compare-and-remove FIRST so a quick reconnect that already
            // re-registered is never clobbered; then the shared disconnect
            // cleanup (offline + AgentStatus(offline) + fail orphaned runs).
            registry.Unregister(identity.AgentId, conn.ConnectionId);
            try
            {
                using var scope = scopeFactory.CreateScope();
                await GetProcessor(scope).HandleDisconnectAsync(identity.AgentId);
            }
            catch (Exception ex)
            {
                logger.LogError(ex,
                    "Disconnect cleanup for agent {AgentId} failed", identity.AgentId);
            }
            await conn.CloseAsync();
        }
    }

    /// <summary>
    /// Resolve the scoped <see cref="AgentMessageProcessor"/> — registered by
    /// <see cref="AgentSocketExtensions.AddAgentRawSocket"/>, or activated
    /// directly from the scope's services when it is not (both paths yield a
    /// processor bound to the scope's <c>NetworkerDbContext</c>).
    /// </summary>
    private static AgentMessageProcessor GetProcessor(IServiceScope scope)
        => ActivatorUtilities.GetServiceOrCreateInstance<AgentMessageProcessor>(scope.ServiceProvider);

    /// <summary>
    /// The real client IP. In prod the control plane sits behind nginx, so the
    /// socket peer is always the proxy (127.0.0.1) — using it would make
    /// <c>api_key_last_used_ip</c> useless and collapse the per-IP brute-force
    /// limiter into a single global bucket. Prefer nginx's <c>X-Real-IP</c>
    /// (a single value it overwrites, so a client can't spoof it), then the
    /// left-most hop of <c>X-Forwarded-For</c>, and only fall back to the
    /// socket peer when neither header is present (direct/local connection).
    /// </summary>
    /// <summary>
    /// Resolve the agent api-key, preferring the <see cref="ApiKeyHeader"/>
    /// request header (keeps the secret out of the URL / access log) and falling
    /// back to the legacy <c>?key=</c> query for fielded agents that predate it.
    /// The query fallback is removed at the Rust-agent decommission, after which
    /// the header is the only accepted transport.
    /// </summary>
    internal static string ResolveAgentApiKey(HttpContext context)
    {
        var header = context.Request.Headers[ApiKeyHeader].ToString();
        if (!string.IsNullOrEmpty(header))
        {
            return header.Trim();
        }

        return context.Request.Query[ApiKeyQueryKey].ToString();
    }

    internal static string? ResolveClientIp(HttpContext context)
    {
        var realIp = context.Request.Headers["X-Real-IP"].ToString();
        if (!string.IsNullOrWhiteSpace(realIp))
        {
            return realIp.Trim();
        }

        var xff = context.Request.Headers["X-Forwarded-For"].ToString();
        if (!string.IsNullOrWhiteSpace(xff))
        {
            var first = xff.Split(',', 2)[0].Trim();
            if (first.Length > 0)
            {
                return first;
            }
        }

        return context.Connection.RemoteIpAddress?.ToString();
    }
}
