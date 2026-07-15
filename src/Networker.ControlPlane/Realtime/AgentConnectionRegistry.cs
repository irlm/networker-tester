using System.Collections.Concurrent;
using Microsoft.AspNetCore.SignalR;

namespace Networker.ControlPlane.Realtime;

/// <summary>
/// In-process registry mapping <c>agent_id → live connection</c> — the C#
/// analogue of the Rust agent hub's <c>HashMap&lt;Uuid, mpsc::Sender&lt;String&gt;&gt;</c>
/// (crates/networker-dashboard/src/ws/agent_hub.rs). It lets the control plane
/// push a typed <see cref="ControlMessage"/> to one specific connected agent.
///
/// <para><b>Dual transport (Phase-2 M6).</b> A connection is registered with a
/// per-connection <em>sender delegate</em> that receives the serialized
/// <c>{"type":"...", ...}</c> envelope:</para>
/// <list type="bullet">
///   <item><b>SignalR</b> (<see cref="AgentProtocolHub"/>): the two-argument
///   <see cref="Register(Guid, string)"/> overload installs the default sender,
///   which pushes the payload as the single argument of the
///   <see cref="ClientReceiveMethod"/> client method via
///   <c>IHubContext&lt;AgentProtocolHub&gt;</c>.</item>
///   <item><b>Raw WebSocket</b> (<c>RawWs.AgentSocketEndpoint</c> — the
///   transport the fielded Rust agents actually speak): the three-argument
///   <see cref="Register(Guid, string, Func{string, CancellationToken, Task})"/>
///   overload installs a sender that enqueues the payload onto the socket's
///   bounded outbound channel (<c>RawWs.AgentSocketConnection.SendAsync</c> —
///   <c>WebSocket.SendAsync</c> is not concurrent-safe, so all writes are
///   serialized through a single pump task, exactly like the Rust
///   <c>mpsc::channel</c> + sink task).</item>
/// </list>
/// <para>Either way the on-the-wire payload is byte-identical to the WS text
/// frame the Rust hub sends — the agent decodes the same
/// <c>{"type":"...", ...}</c> JSON it decodes from Rust today.</para>
///
/// <para>Registered as a singleton (see <see cref="AgentProtocolExtensions"/>).
/// Backed by a <see cref="ConcurrentDictionary{TKey,TValue}"/> so it is safe to
/// mutate from concurrent hub invocations / socket loops. An agent that
/// reconnects (new connection id) simply overwrites its previous mapping — last
/// writer wins, matching the Rust <c>register()</c> which inserts
/// unconditionally. Raw connection ids carry a <c>raw-</c> prefix so they stay
/// disjoint from SignalR connection ids and the compare-and-remove
/// <see cref="Unregister"/> guard works across both transports.</para>
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

    /// <summary>
    /// One live agent connection: the transport's connection id (SignalR
    /// connectionId or the raw endpoint's synthetic <c>raw-…</c> id) plus the
    /// sender delegate that delivers one serialized envelope to it. A record
    /// class so the compare-and-remove in <see cref="Unregister"/> matches the
    /// exact registered instance (a racing re-registration creates a new
    /// instance and is therefore never clobbered).
    /// </summary>
    private sealed record Connection(string ConnectionId, Func<string, CancellationToken, Task> Sender);

    private readonly IHubContext<AgentProtocolHub> _hub;
    private readonly ILogger<AgentConnectionRegistry> _logger;

    // agent_id -> current live connection (id + sender).
    private readonly ConcurrentDictionary<Guid, Connection> _connections = new();

    public AgentConnectionRegistry(
        IHubContext<AgentProtocolHub> hub,
        ILogger<AgentConnectionRegistry> logger)
    {
        _hub = hub;
        _logger = logger;
    }

    /// <summary>
    /// Associate <paramref name="agentId"/> with a SignalR connection: installs
    /// the default sender that pushes payloads through
    /// <c>IHubContext&lt;AgentProtocolHub&gt;.Clients.Client(connectionId)</c>.
    /// Overwrites any prior mapping (reconnect / new connection). Mirrors the
    /// Rust <c>AgentHub::register</c>.
    /// </summary>
    public void Register(Guid agentId, string connectionId)
        => Register(
            agentId,
            connectionId,
            (payload, ct) => _hub.Clients.Client(connectionId)
                .SendAsync(ClientReceiveMethod, payload, ct));

    /// <summary>
    /// Associate <paramref name="agentId"/> with a connection whose outbound
    /// path is <paramref name="sender"/> — the raw-WebSocket transport passes
    /// its socket's channel-enqueue here. The delegate receives the serialized
    /// <c>{"type":"...", ...}</c> envelope; a throwing sender is reported as
    /// "not sent" (<c>false</c>) by <see cref="SendAsync"/>. Overwrites any
    /// prior mapping (reconnect / transport switch) — last writer wins.
    /// </summary>
    public void Register(Guid agentId, string connectionId, Func<string, CancellationToken, Task> sender)
        => _connections[agentId] = new Connection(connectionId, sender);

    /// <summary>
    /// Remove the mapping for <paramref name="agentId"/>, but only if it still
    /// points at <paramref name="connectionId"/>. The guard prevents a stale
    /// disconnect from an old socket clobbering a fresh reconnection that
    /// already re-registered under a new connection id (a race the Rust
    /// single-socket model could not hit, but this side can — including a raw
    /// reconnect racing a SignalR disconnect, which is why raw ids are
    /// <c>raw-</c> prefixed and thus never collide with SignalR ids).
    /// </summary>
    public void Unregister(Guid agentId, string connectionId)
    {
        if (_connections.TryGetValue(agentId, out var current)
            && current.ConnectionId == connectionId)
        {
            // ICollection<KeyValuePair<>>.Remove only removes on an exact
            // key+value match — the atomic compare-and-remove we want. If a
            // re-registration slipped in between the read and this call, the
            // value no longer matches and nothing is removed.
            ((ICollection<KeyValuePair<Guid, Connection>>)_connections)
                .Remove(new KeyValuePair<Guid, Connection>(agentId, current));
        }
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
    /// the discriminator is written) and hand it to the agent's registered
    /// sender delegate. Returns <c>false</c> when the agent has no live
    /// connection OR its sender fails (e.g. the raw socket's outbound channel is
    /// already closed mid-teardown) — the C# equivalent of the Rust
    /// <c>send_to_agent</c> returning <c>"agent … not connected"</c>. Never
    /// throws for a missing/dead agent; caller cancellation still propagates.
    /// </summary>
    public async Task<bool> SendAsync(Guid agentId, ControlMessage message, CancellationToken ct = default)
    {
        if (!_connections.TryGetValue(agentId, out var connection))
        {
            _logger.LogWarning(
                "Cannot send {Type} to agent {AgentId}: not connected",
                message.GetType().Name, agentId);
            return false;
        }

        // Serialize against the base type so [JsonPolymorphic] emits `type`.
        var payload = System.Text.Json.JsonSerializer.Serialize(message);
        try
        {
            await connection.Sender(payload, ct);
            return true;
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            throw; // caller-requested cancellation is not a delivery verdict
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex,
                "Failed to send {Type} to agent {AgentId} (conn {ConnId})",
                message.GetType().Name, agentId, connection.ConnectionId);
            return false;
        }
    }
}
