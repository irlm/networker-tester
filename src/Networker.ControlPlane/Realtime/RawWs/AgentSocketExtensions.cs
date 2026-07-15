using Microsoft.Extensions.DependencyInjection.Extensions;

namespace Networker.ControlPlane.Realtime.RawWs;

/// <summary>
/// DI + pipeline wiring for the raw-WebSocket agent endpoint (Phase-2 M6
/// cutover). This milestone does NOT touch Program.cs; the integrator performs
/// the edits documented on <see cref="MapAgentRawSocket"/>.
/// </summary>
public static class AgentSocketExtensions
{
    /// <summary>
    /// Register the scoped <see cref="AgentMessageProcessor"/> the raw endpoint
    /// resolves per inbound frame. Call after
    /// <c>AddDbContext&lt;NetworkerDbContext&gt;()</c> and
    /// <c>AddDashboardEventBus()</c> (its dependencies) and alongside
    /// <c>AddAgentProtocol()</c> (the shared <see cref="AgentConnectionRegistry"/>).
    /// Optional but recommended — <see cref="AgentSocketEndpoint"/> falls back
    /// to <c>ActivatorUtilities</c> activation when the processor is not
    /// registered, and the SignalR hub constructs its own from its scoped
    /// dependencies, so the current Program.cs keeps working unmodified.
    /// </summary>
    public static IServiceCollection AddAgentRawSocket(this IServiceCollection services)
    {
        services.TryAddScoped<AgentMessageProcessor>();
        return services;
    }

    /// <summary>
    /// Map GET <c>/ws/agent</c> to the raw agent WebSocket handler and enable
    /// the WebSocket middleware (with the server keepalive-ping cadence). Call
    /// before <c>app.Run()</c>.
    ///
    /// <para><b>Program.cs wiring the integrator must apply (M6 cutover):</b></para>
    /// <code>
    /// // 1. Services — next to the existing builder.Services.AddAgentProtocol():
    /// builder.Services.AddAgentRawSocket();
    ///
    /// // 2. Pipeline — the fielded Rust agents own /ws/agent now. REMAP the
    /// //    SignalR protocol hub to /hub/agent (kept for SignalR-speaking C#
    /// //    agents / tooling), then map the raw endpoint. Change:
    /// //        app.MapHub&lt;AgentProtocolHub&gt;("/ws/agent");
    /// //    to:
    /// app.MapHub&lt;AgentProtocolHub&gt;("/hub/agent");
    /// app.MapAgentRawSocket();
    /// </code>
    ///
    /// <para>The two mappings must not share a route: <c>MapAgentRawSocket</c>
    /// binds GET <c>/ws/agent</c>, so the hub MUST move to <c>/hub/agent</c>
    /// first or endpoint routing will report an ambiguous match.</para>
    ///
    /// <para><b>Auth:</b> the endpoint is <c>AllowAnonymous</c> at the routing
    /// layer on purpose — agents authenticate with <c>?key=&lt;api_key&gt;</c>
    /// validated against <c>agent.api_key</c> inside the handler (401 before
    /// the upgrade, exactly like the Rust <c>agent_ws_handler</c>), never JWT.</para>
    /// </summary>
    public static WebApplication MapAgentRawSocket(this WebApplication app)
    {
        // Idempotent enough for our purposes: UseWebSockets only installs the
        // upgrade-handshake middleware; WebApplication slots it between routing
        // and endpoint execution even when called at mapping time.
        app.UseWebSockets(new WebSocketOptions
        {
            KeepAliveInterval = AgentSocketEndpoint.KeepAliveInterval,
        });

        app.MapGet(AgentSocketEndpoint.Path, AgentSocketEndpoint.HandleAsync)
            .AllowAnonymous();

        return app;
    }
}
