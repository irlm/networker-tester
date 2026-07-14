using System.Collections.Concurrent;
using Microsoft.AspNetCore.SignalR;

namespace Networker.ControlPlane.Realtime;

/// <summary>
/// In-process registry mapping <c>agent_id ⇄ SignalR connectionId</c> — the C#
/// analogue of the Rust agent hub's <c>HashMap&lt;Uuid, mpsc::Sender&lt;String&gt;&gt;</c>
/// (crates/networker-dashboard/src/ws/agent_hub.rs). It lets the control plane
/// push a typed <see cref="ControlMessage"/> to one specific connected agent.
///
/// <para><b>Rust → SignalR mapping.</b> In Rust each connected agent owns an
/// mpsc channel; <c>send_to_agent</c> serialises the <c>ControlMessage</c> to a
/// JSON string and <c>try_send</c>s it, and the socket's outbound pump writes it
/// as a WS text frame. Here the connection is identified by its SignalR
/// <c>connectionId</c>, and the outbound push is a
/// <c>IHubContext&lt;AgentProtocolHub&gt;.Clients.Client(connId).SendAsync(...)</c>.
/// To keep the on-the-wire payload byte-identical to Rust, the serialized
/// envelope is sent as the single argument of one client method
/// (<see cref="ClientReceiveMethod"/> = <c>"message"</c>) rather than as a
/// per-variant SignalR method — the agent decodes the same
/// <c>{"type":"...", ...}</c> JSON it decodes from Rust today. The one variant
/// that maps to a native SignalR method is <c>Welcome</c>, sent inline from the
/// hub's <c>OnConnectedAsync</c> (see <see cref="AgentProtocolHub"/>).</para>
///
/// <para>Registered as a singleton (see <see cref="AgentProtocolExtensions"/>).
/// Backed by a <see cref="ConcurrentDictionary{TKey,TValue}"/> so it is safe to
/// mutate from concurrent hub invocations. An agent that reconnects (new
/// connectionId) simply overwrites its previous mapping — last writer wins,
/// matching the Rust <c>register()</c> which inserts unconditionally.</para>
/// </summary>
public sealed class AgentConnectionRegistry
{
    /// <summary>
    /// The SignalR client method the agent handles for every pushed
    /// <see cref="ControlMessage"/>. Its single argument is the serialized
    /// <c>{"type":"...", ...}</c> envelope — identical to the WS text frame the
    /// Rust hub sends.
    /// </summary>
    public const string ClientReceiveMethod = "message";

    private readonly IHubContext<AgentProtocolHub> _hub;
    private readonly ILogger<AgentConnectionRegistry> _logger;

    // agent_id -> current SignalR connection id.
    private readonly ConcurrentDictionary<Guid, string> _connections = new();

    public AgentConnectionRegistry(
        IHubContext<AgentProtocolHub> hub,
        ILogger<AgentConnectionRegistry> logger)
    {
        _hub = hub;
        _logger = logger;
    }

    /// <summary>
    /// Associate <paramref name="agentId"/> with <paramref name="connectionId"/>.
    /// Overwrites any prior mapping (reconnect / new connection). Mirrors the
    /// Rust <c>AgentHub::register</c>.
    /// </summary>
    public void Register(Guid agentId, string connectionId)
        => _connections[agentId] = connectionId;

    /// <summary>
    /// Remove the mapping for <paramref name="agentId"/>, but only if it still
    /// points at <paramref name="connectionId"/>. The guard prevents a stale
    /// <c>OnDisconnectedAsync</c> from an old socket clobbering a fresh
    /// reconnection that already re-registered under a new connection id
    /// (a race the Rust single-socket model could not hit, but SignalR can).
    /// </summary>
    public void Unregister(Guid agentId, string connectionId)
    {
        // ICollection<KeyValuePair<>>.Remove only removes on an exact key+value
        // match — the atomic compare-and-remove we want.
        ((ICollection<KeyValuePair<Guid, string>>)_connections)
            .Remove(new KeyValuePair<Guid, string>(agentId, connectionId));
    }

    /// <summary>
    /// Whether an agent currently has a live connection. Mirrors the Rust
    /// <c>is_agent_online</c>. This is what the M3 dispatcher checks before
    /// assigning a run.
    /// </summary>
    public bool IsOnline(Guid agentId) => _connections.ContainsKey(agentId);

    /// <summary>Snapshot of all currently-connected agent ids.</summary>
    public IReadOnlyCollection<Guid> OnlineAgents() => _connections.Keys.ToArray();

    /// <summary>
    /// Pick any online agent (the Rust <c>any_online_agent</c>), or null if none
    /// are connected. Ordering is unspecified, matching the Rust
    /// <c>keys().next()</c>.
    /// </summary>
    public Guid? AnyOnlineAgent()
    {
        foreach (var id in _connections.Keys)
        {
            return id;
        }
        return null;
    }

    // ── Outbound sender API (what M3's dispatcher calls) ─────────────────────

    /// <summary>
    /// Assign a run to a specific agent (the Rust <c>ControlMessage::AssignRun</c>
    /// path). <paramref name="run"/> / <paramref name="config"/> are the already
    /// serialized canonical TestRun / TestConfig JSON. Returns <c>false</c> if
    /// the agent is not connected (mirrors the Rust <c>send_to_agent</c> "agent
    /// not connected" error path — the caller/redispatcher retries).
    /// </summary>
    public Task<bool> AssignRunAsync(
        Guid agentId,
        System.Text.Json.JsonElement run,
        System.Text.Json.JsonElement config,
        CancellationToken ct = default)
        => SendAsync(agentId, new AssignRunMessage(run, config), ct);

    /// <summary>Cooperatively cancel an in-flight run on a specific agent.</summary>
    public Task<bool> CancelRunAsync(Guid agentId, Guid runId, CancellationToken ct = default)
        => SendAsync(agentId, new CancelRunMessage(runId), ct);

    /// <summary>Dispatch a typed command envelope to a specific agent.</summary>
    public Task<bool> SendCommandAsync(Guid agentId, CommandMessage command, CancellationToken ct = default)
        => SendAsync(agentId, command, ct);

    /// <summary>Cancel an in-flight command on a specific agent.</summary>
    public Task<bool> CancelCommandAsync(Guid agentId, Guid commandId, CancellationToken ct = default)
        => SendAsync(agentId, new CancelMessage(commandId), ct);

    /// <summary>Send a dashboard-side liveness ping (server clock) to an agent.</summary>
    public Task<bool> HeartbeatPingAsync(Guid agentId, DateTimeOffset now, CancellationToken ct = default)
        => SendAsync(agentId, new HeartbeatPingMessage(now), ct);

    /// <summary>Ask an agent to drain and shut down gracefully.</summary>
    public Task<bool> ShutdownAsync(Guid agentId, CancellationToken ct = default)
        => SendAsync(agentId, new ShutdownMessage(), ct);

    /// <summary>
    /// Core outbound push: serialise <paramref name="message"/> to the flat
    /// <c>{"type":"...", ...}</c> envelope (against the polymorphic base type so
    /// the discriminator is written) and send it to the agent's current
    /// connection as the single argument of <see cref="ClientReceiveMethod"/>.
    /// Returns <c>false</c> when the agent has no live connection — the C#
    /// equivalent of the Rust <c>send_to_agent</c> returning
    /// <c>"agent … not connected"</c>. Never throws for a missing agent.
    /// </summary>
    public async Task<bool> SendAsync(Guid agentId, ControlMessage message, CancellationToken ct = default)
    {
        if (!_connections.TryGetValue(agentId, out var connectionId))
        {
            _logger.LogWarning(
                "Cannot send {Type} to agent {AgentId}: not connected",
                message.GetType().Name, agentId);
            return false;
        }

        // Serialize against the base type so [JsonPolymorphic] emits `type`.
        var payload = System.Text.Json.JsonSerializer.Serialize(message);
        await _hub.Clients.Client(connectionId).SendAsync(ClientReceiveMethod, payload, ct);
        return true;
    }
}
