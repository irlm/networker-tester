using Microsoft.AspNetCore.SignalR;
using Networker.ControlPlane.Realtime.RawWs;
using Networker.Data;

namespace Networker.ControlPlane.Realtime;

/// <summary>
/// Agent-facing protocol hub — the SignalR transport shell of the agent
/// protocol (the C# re-architecture of the Rust <c>ws/agent_hub.rs</c>).
/// Mapped by Program.cs; after the M6 raw-WebSocket cutover the raw endpoint
/// (<see cref="AgentSocketEndpoint"/>) owns <c>/ws/agent</c> and this hub moves
/// to <c>/hub/agent</c> (see <see cref="AgentSocketExtensions.MapAgentRawSocket"/>).
///
/// <para><b>Thin shell (M6).</b> Every per-message persistence + event-bus rule
/// lives in the transport-agnostic <see cref="AgentMessageProcessor"/>, shared
/// verbatim with the raw endpoint — this class only adapts the SignalR
/// connection lifecycle (connect/abort/disconnect, per-connection Items) onto
/// it. Do not add protocol logic here; add it to the processor so both
/// transports stay identical.</para>
///
/// <para><b>Named <c>AgentProtocolHub</c>, not <c>AgentHub</c></b>, on purpose:
/// a proof-of-concept <c>AgentHub</c> once lived in Program.cs and a duplicate
/// type name would not compile (see <see cref="AgentProtocolExtensions"/>).</para>
///
/// <para><b>Authentication (api-key, NOT JWT).</b> Agents authenticate with
/// <c>?key=&lt;api_key&gt;</c> validated against <c>agent.api_key</c>, exactly
/// like the Rust <c>agent_ws_handler</c>. This hub therefore must NOT carry the
/// JWT <c>[Authorize]</c> attribute — it does its own key check in
/// <see cref="OnConnectedAsync"/> and aborts the connection when the key is
/// missing or unknown.</para>
///
/// <para><b>DI scope.</b> <see cref="NetworkerDbContext"/> is scoped; SignalR
/// creates a fresh DI scope per hub-method invocation, so the processor this
/// hub news up from its scoped dependencies is valid exactly for one
/// invocation, never shared across concurrent frames. (It is constructed
/// directly rather than resolved so Program.cs works whether or not
/// <see cref="AgentSocketExtensions.AddAgentRawSocket"/> registered it.)
/// <see cref="EventBus"/> and <see cref="AgentConnectionRegistry"/> are
/// singletons.</para>
/// </summary>
public sealed class AgentProtocolHub : Hub
{
    /// <summary>Query-string key carrying the agent api-key (Rust: <c>?key=</c>).</summary>
    public const string ApiKeyQueryKey = "key";

    /// <summary>Per-connection item key under which the resolved agent id is stashed.</summary>
    private const string AgentIdItemKey = "agent_id";

    /// <summary>Per-connection item key under which the resolved agent name is stashed.</summary>
    private const string AgentNameItemKey = "agent_name";

    private readonly AgentMessageProcessor _processor;
    private readonly AgentConnectionRegistry _registry;
    private readonly ILogger<AgentProtocolHub> _logger;

    public AgentProtocolHub(
        NetworkerDbContext db,
        EventBus bus,
        AgentConnectionRegistry registry,
        ILogger<AgentProtocolHub> logger,
        ILogger<AgentMessageProcessor> processorLogger)
    {
        _processor = new AgentMessageProcessor(db, bus, processorLogger);
        _registry = registry;
        _logger = logger;
    }

    // ── Connection lifecycle ─────────────────────────────────────────────────

    /// <summary>
    /// Validate the <c>?key=</c> api-key against <c>agent.api_key</c>; abort the
    /// connection if it is missing or unknown (Rust returns 401 from the
    /// upgrade handler — SignalR's nearest equivalent is aborting the connection
    /// so no frames are ever processed). On success: register the connection
    /// (default SignalR sender), mark the agent <c>online</c> + stamp
    /// <c>last_heartbeat</c>, publish an <see cref="AgentStatus"/> event, and
    /// send the <see cref="WelcomeMessage"/> frame.
    /// </summary>
    public override async Task OnConnectedAsync()
    {
        var http = Context.GetHttpContext();
        var apiKey = http?.Request.Query[ApiKeyQueryKey].ToString();

        var identity = await _processor.AuthenticateAsync(apiKey, Context.ConnectionAborted);
        if (identity is null)
        {
            _logger.LogWarning(
                "Agent connection {ConnId} rejected: {Reason}",
                Context.ConnectionId,
                string.IsNullOrEmpty(apiKey) ? "no api key" : "unknown api key");
            Context.Abort();
            return;
        }

        Context.Items[AgentIdItemKey] = identity.AgentId;
        Context.Items[AgentNameItemKey] = identity.Name;

        _logger.LogInformation(
            "Agent connected (v2): {AgentId} name={Name} conn={ConnId}",
            identity.AgentId, identity.Name, Context.ConnectionId);

        // Register connection so the dispatcher can push ControlMessages
        // (two-argument overload = the SignalR IHubContext sender).
        _registry.Register(identity.AgentId, Context.ConnectionId);

        // Mark online + heartbeat + AgentStatus(online) (Rust: update_status "online").
        await _processor.HandleConnectAsync(identity.AgentId, Context.ConnectionAborted);

        // Send Welcome as a native SignalR method (the one control message the
        // hub emits directly rather than through the registry's "message" push).
        await Clients.Caller.SendAsync(
            AgentConnectionRegistry.ClientReceiveMethod,
            AgentMessageProcessor.WelcomeFrame(identity.AgentId, identity.Name),
            Context.ConnectionAborted);

        await base.OnConnectedAsync();
    }

    /// <summary>
    /// Deregister the connection (compare-and-remove, so a fresh reconnect that
    /// already re-registered is never clobbered), then run the shared
    /// disconnect cleanup: mark the agent <c>offline</c>, publish
    /// <see cref="AgentStatus"/>(offline), and fail the agent's orphaned runs —
    /// <see cref="AgentMessageProcessor.HandleDisconnectAsync"/>.
    /// </summary>
    public override async Task OnDisconnectedAsync(Exception? exception)
    {
        if (Context.Items.TryGetValue(AgentIdItemKey, out var raw) && raw is Guid agentId)
        {
            _registry.Unregister(agentId, Context.ConnectionId);
            await _processor.HandleDisconnectAsync(agentId);
        }

        await base.OnDisconnectedAsync(exception);
    }

    // ── Inbound AgentMessage entry point ─────────────────────────────────────

    /// <summary>
    /// Single inbound entry point: the agent invokes ONE hub method,
    /// <c>Receive</c>, with the serialized <c>{"type":"...", ...}</c> envelope —
    /// the same frame it sends the Rust hub (and the raw endpoint) as WS text.
    /// Decode + dispatch + persistence all happen in the shared
    /// <see cref="AgentMessageProcessor.HandleFrameAsync"/>; unknown /
    /// undecodable frames are ignored (Rust drops decode failures and legacy v1
    /// variants silently).
    /// </summary>
    public Task Receive(string frame)
        => _processor.HandleFrameAsync(AgentId(), frame, Context.ConnectionAborted);

    /// <summary>
    /// The agent id resolved at connect time. Non-null for every inbound frame
    /// because <see cref="OnConnectedAsync"/> aborts unauthenticated connections
    /// before any hub method runs.
    /// </summary>
    private Guid AgentId()
        => Context.Items.TryGetValue(AgentIdItemKey, out var raw) && raw is Guid id
            ? id
            : Guid.Empty;
}
