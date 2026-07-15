using System.Text.Json;
using Microsoft.AspNetCore.SignalR;

namespace Networker.ControlPlane.Realtime.RawWs;

/// <summary>
/// Decorator over SignalR's <see cref="HubLifetimeManager{THub}"/> for
/// <see cref="TesterQueueHub"/> that mirrors every group send for
/// <c>tq:{projectId}:{testerId}</c> groups into the raw-WebSocket subscribers.
///
/// <para><b>Why here:</b> tester-queue updates originate outside the hub —
/// <c>TesterQueueBroadcaster</c> pushes <c>tester_queue_update</c> /
/// <c>phase_update</c> through <c>IHubContext&lt;TesterQueueHub&gt;.Clients.Group(...)</c>,
/// and ALL such sends funnel through this lifetime manager. Decorating it gives
/// the raw sockets the identical fan-out (same shared
/// <see cref="TesterQueueRegistry"/> seq stamps, same message objects) without
/// editing the hub or the broadcaster. SignalR clients are untouched: every call
/// still delegates to the inner <see cref="DefaultHubLifetimeManager{THub}"/>.</para>
///
/// <para><b>What raw clients receive:</b> the first send argument (the message
/// record) serialized directly with System.Text.Json — the flat, type-tagged
/// shape pinned by the <c>[JsonPropertyName]</c> contract in
/// <c>TesterQueueMessages.cs</c> (<c>{"type":"tester_queue_update",...}</c>),
/// with no SignalR invocation envelope.</para>
///
/// <para>Registered by <see cref="RawWsExtensions.AddRawWebSockets"/>; the
/// closed-generic registration supersedes SignalR's open-generic default for
/// this hub only.</para>
/// </summary>
public sealed class RawWsTesterQueueLifetimeManager : HubLifetimeManager<TesterQueueHub>
{
    /// <summary>Group-name prefix produced by <see cref="TesterQueueRegistry.GroupName"/>.</summary>
    private const string TesterGroupPrefix = "tq:";

    private readonly HubLifetimeManager<TesterQueueHub> _inner;
    private readonly RawSocketRegistry _rawSockets;

    public RawWsTesterQueueLifetimeManager(
        HubLifetimeManager<TesterQueueHub> inner,
        RawSocketRegistry rawSockets)
    {
        _inner = inner;
        _rawSockets = rawSockets;
    }

    // ── Mirrored sends ────────────────────────────────────────────────────────

    public override Task SendGroupAsync(
        string groupName, string methodName, object?[] args, CancellationToken cancellationToken = default)
    {
        MirrorToRawSockets(groupName, args);
        return _inner.SendGroupAsync(groupName, methodName, args, cancellationToken);
    }

    public override Task SendGroupsAsync(
        IReadOnlyList<string> groupNames, string methodName, object?[] args,
        CancellationToken cancellationToken = default)
    {
        foreach (var groupName in groupNames)
        {
            MirrorToRawSockets(groupName, args);
        }
        return _inner.SendGroupsAsync(groupNames, methodName, args, cancellationToken);
    }

    public override Task SendGroupExceptAsync(
        string groupName, string methodName, object?[] args,
        IReadOnlyList<string> excludedConnectionIds, CancellationToken cancellationToken = default)
    {
        // Excluded ids are SignalR connection ids; raw connections have their
        // own id space and can never be the excluded sender — mirror fully.
        MirrorToRawSockets(groupName, args);
        return _inner.SendGroupExceptAsync(
            groupName, methodName, args, excludedConnectionIds, cancellationToken);
    }

    private void MirrorToRawSockets(string groupName, object?[] args)
    {
        if (!groupName.StartsWith(TesterGroupPrefix, StringComparison.Ordinal) ||
            !_rawSockets.HasTesterGroup(groupName))
        {
            return;
        }

        // Hub convention: single-argument client method carrying the type-tagged
        // message record (snapshot/update/phase).
        var payload = args is { Length: > 0 } ? args[0] : null;
        if (payload is null)
        {
            return;
        }

        // Serialize against the runtime type so the record's [JsonPropertyName]
        // contract (snake_case + "type" tag + null-omission) is honored.
        var json = JsonSerializer.Serialize(payload, payload.GetType());
        _rawSockets.BroadcastTesterGroup(groupName, json);
    }

    // ── Pure delegation ───────────────────────────────────────────────────────

    public override Task OnConnectedAsync(HubConnectionContext connection) =>
        _inner.OnConnectedAsync(connection);

    public override Task OnDisconnectedAsync(HubConnectionContext connection) =>
        _inner.OnDisconnectedAsync(connection);

    public override Task SendAllAsync(
        string methodName, object?[] args, CancellationToken cancellationToken = default) =>
        _inner.SendAllAsync(methodName, args, cancellationToken);

    public override Task SendAllExceptAsync(
        string methodName, object?[] args, IReadOnlyList<string> excludedConnectionIds,
        CancellationToken cancellationToken = default) =>
        _inner.SendAllExceptAsync(methodName, args, excludedConnectionIds, cancellationToken);

    public override Task SendConnectionAsync(
        string connectionId, string methodName, object?[] args,
        CancellationToken cancellationToken = default) =>
        _inner.SendConnectionAsync(connectionId, methodName, args, cancellationToken);

    public override Task SendConnectionsAsync(
        IReadOnlyList<string> connectionIds, string methodName, object?[] args,
        CancellationToken cancellationToken = default) =>
        _inner.SendConnectionsAsync(connectionIds, methodName, args, cancellationToken);

    public override Task SendUserAsync(
        string userId, string methodName, object?[] args,
        CancellationToken cancellationToken = default) =>
        _inner.SendUserAsync(userId, methodName, args, cancellationToken);

    public override Task SendUsersAsync(
        IReadOnlyList<string> userIds, string methodName, object?[] args,
        CancellationToken cancellationToken = default) =>
        _inner.SendUsersAsync(userIds, methodName, args, cancellationToken);

    public override Task AddToGroupAsync(
        string connectionId, string groupName, CancellationToken cancellationToken = default) =>
        _inner.AddToGroupAsync(connectionId, groupName, cancellationToken);

    public override Task RemoveFromGroupAsync(
        string connectionId, string groupName, CancellationToken cancellationToken = default) =>
        _inner.RemoveFromGroupAsync(connectionId, groupName, cancellationToken);
}
