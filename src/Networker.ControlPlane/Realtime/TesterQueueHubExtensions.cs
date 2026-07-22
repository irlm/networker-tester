using Microsoft.AspNetCore.SignalR;

namespace Networker.ControlPlane.Realtime;

/// <summary>
/// DI + wiring helpers for the tester-queue hub (<c>/ws/testers</c>).
///
/// Program.cs is intentionally NOT edited by this milestone; the integration
/// step wires the hub in with the three calls documented on
/// <see cref="AddTesterQueueHub"/>.
/// </summary>
public static class TesterQueueHubExtensions
{
    /// <summary>
    /// Register the <see cref="TesterQueueRegistry"/> singleton that backs the
    /// hub. Call during service registration, after <c>AddSignalR()</c> and
    /// <c>AddNetworkerAuth(...)</c> (the hub depends on <c>AuthRepository</c> and
    /// <c>NetworkerDbContext</c>, both already registered by those calls).
    ///
    /// <para><b>Program.cs wiring (done by integration, three edits):</b></para>
    /// <code>
    /// // 1. Services — register the registry singleton:
    /// builder.Services.AddTesterQueueHub();
    ///
    /// // 2. Services — let SignalR read the JWT from the WebSocket query string.
    /// //    A browser WebSocket can't set an Authorization header, so the token
    /// //    arrives as ?access_token=&lt;jwt&gt;. This mirrors the Rust handler's
    /// //    ?token=&lt;jwt&gt; (name differs: SignalR's JS client uses access_token).
    /// //    Add this to the existing AddJwtBearer(options =&gt; { ... }) in
    /// //    AddNetworkerAuth, or re-configure it here:
    /// builder.Services.Configure&lt;JwtBearerOptions&gt;(
    ///     JwtBearerDefaults.AuthenticationScheme, options =&gt;
    /// {
    ///     options.Events = new JwtBearerEvents
    ///     {
    ///         OnMessageReceived = ctx =&gt;
    ///         {
    ///             var token = ctx.Request.Query["access_token"];
    ///             var path = ctx.HttpContext.Request.Path;
    ///             if (!string.IsNullOrEmpty(token) &amp;&amp;
    ///                 path.StartsWithSegments("/ws/testers"))
    ///             {
    ///                 ctx.Token = token;
    ///             }
    ///             return Task.CompletedTask;
    ///         },
    ///     };
    /// });
    ///
    /// // 3. Pipeline — map the hub (after UseNetworkerAuth):
    /// app.MapHub&lt;TesterQueueHub&gt;("/ws/testers");
    /// </code>
    /// </summary>
    public static IServiceCollection AddTesterQueueHub(this IServiceCollection services)
    {
        // Singleton: the registry outlives individual hub connections (each hub
        // invocation gets a fresh transient hub instance, but shares this state).
        services.AddSingleton<TesterQueueRegistry>();

        // The broadcaster lets non-hub code (schedulers, agent-result handlers)
        // push queue updates without holding a hub instance.
        services.AddSingleton<TesterQueueBroadcaster>();
        services.AddSingleton<ITesterQueuePush>(
            sp => sp.GetRequiredService<TesterQueueBroadcaster>());

        // THE producer of tester_queue_update deltas: observes EventBus run
        // transitions (JobUpdate/JobComplete) and pushes the rebuilt queue to
        // subscribers via the broadcaster. Without this the dashboard's live
        // queue panel only refreshed on reconnect snapshots (2026-07 gap fix).
        services.AddSingleton<TesterQueueUpdateProducer>();
        services.AddSingleton<IDashboardEventObserver>(
            sp => sp.GetRequiredService<TesterQueueUpdateProducer>());
        return services;
    }
}

/// <summary>
/// Push seam over <see cref="TesterQueueBroadcaster"/> so producers (and their
/// tests) depend on the send contract rather than SignalR's <c>IHubContext</c>.
/// </summary>
public interface ITesterQueuePush
{
    /// <inheritdoc cref="TesterQueueBroadcaster.NotifyQueueUpdateAsync"/>
    Task NotifyQueueUpdateAsync(
        string projectId,
        string testerId,
        string trigger,
        TesterQueueEntry? running,
        IReadOnlyList<TesterQueueEntry> queued,
        CancellationToken ct = default);
}

/// <summary>
/// Fan-out helper for pushing tester-queue messages to subscribers of a tester
/// from outside the hub (e.g. when the control plane enqueues/completes a
/// benchmark). SignalR Groups do the routing that the Rust hub did by hand over
/// per-subscriber mpsc channels.
///
/// Each push bumps the tester's monotonic <c>seq</c> via the registry, matching
/// the Rust hub which stamps every snapshot/update with an increasing seq.
///
/// <para><b>Slow-subscriber ejection:</b> the Rust hub drops a subscriber whose
/// bounded mpsc buffer is full (a slow/stuck client) and logs it. SignalR does
/// not expose per-connection send back-pressure at the group-send layer, so the
/// exact "buffer full → eject" hook has no direct equivalent. It is approximated
/// by SignalR's own client-timeout / backpressure limits
/// (<c>HubOptions.MaximumParallelInvocationsPerClient</c>, the transport send
/// buffer, and <c>ClientTimeoutInterval</c>): a client that can't keep up is
/// disconnected by the framework, and <see cref="TesterQueueHub.OnDisconnectedAsync"/>
/// then cleans its registry entries — the same end state (subscriber removed) by
/// a different mechanism.</para>
/// </summary>
public sealed class TesterQueueBroadcaster(
    IHubContext<TesterQueueHub> hub,
    TesterQueueRegistry registry) : ITesterQueuePush
{
    /// <summary>
    /// Push a <c>tester_queue_update</c> to all subscribers of a tester. No-op if
    /// nobody is subscribed. <paramref name="trigger"/> names the cause
    /// (e.g. "benchmark_queued", "benchmark_completed").
    /// </summary>
    public Task NotifyQueueUpdateAsync(
        string projectId,
        string testerId,
        string trigger,
        TesterQueueEntry? running,
        IReadOnlyList<TesterQueueEntry> queued,
        CancellationToken ct = default)
    {
        if (!registry.HasSubscribers(projectId, testerId))
        {
            return Task.CompletedTask;
        }

        var seq = registry.NextSeq(projectId, testerId);
        var msg = new TesterQueueUpdateMessage(projectId, testerId, seq, trigger, queued, running);

        return hub.Clients
            .Group(TesterQueueRegistry.GroupName(projectId, testerId))
            .SendAsync(TesterQueueHub.ClientMethod, msg, ct);
    }

    /// <summary>Push a <c>phase_update</c> to subscribers of a tester.</summary>
    public Task NotifyPhaseAsync(
        string projectId,
        string testerId,
        string entityType,
        string entityId,
        string phase,
        IReadOnlyList<string> appliedStages,
        string? outcome = null,
        string? message = null,
        CancellationToken ct = default)
    {
        if (!registry.HasSubscribers(projectId, testerId))
        {
            return Task.CompletedTask;
        }

        var seq = registry.NextSeq(projectId, testerId);
        var msg = new PhaseUpdateMessage(
            projectId, entityType, entityId, seq, phase, appliedStages, outcome, message);

        return hub.Clients
            .Group(TesterQueueRegistry.GroupName(projectId, testerId))
            .SendAsync(TesterQueueHub.ClientMethod, msg, ct);
    }
}
