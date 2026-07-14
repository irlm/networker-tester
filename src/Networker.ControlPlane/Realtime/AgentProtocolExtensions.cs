using Microsoft.AspNetCore.Authentication.JwtBearer;
using Microsoft.Extensions.DependencyInjection;

namespace Networker.ControlPlane.Realtime;

/// <summary>
/// DI + integration wiring for the agent protocol hub (<c>/ws/agent</c>) — the
/// C# re-architecture of the Rust <c>ws/agent_hub.rs</c> (M2 slice 2). This
/// milestone intentionally does NOT touch Program.cs; the integrator performs
/// the edits documented on <see cref="AddAgentProtocol"/>.
/// </summary>
public static class AgentProtocolExtensions
{
    /// <summary>
    /// Register the <see cref="AgentConnectionRegistry"/> singleton that maps
    /// <c>agent_id ⇄ connectionId</c> and exposes the outbound sender API the
    /// M3 dispatcher calls. Call during service registration, after
    /// <c>AddSignalR()</c> (the registry depends on
    /// <c>IHubContext&lt;AgentProtocolHub&gt;</c>) and after
    /// <c>AddDbContext&lt;NetworkerDbContext&gt;()</c> + <c>AddDashboardEventBus()</c>
    /// (the hub itself resolves the scoped <c>NetworkerDbContext</c> and the
    /// singleton <c>EventBus</c>, both already registered by Program.cs today).
    ///
    /// <para><b>Program.cs wiring the integrator must add (and the PoC removal):</b></para>
    /// <code>
    /// // 1. Services — register the connection registry singleton (near the
    /// //    other AddDashboardEventBus() / AddTesterQueueHub() calls):
    /// builder.Services.AddAgentProtocol();
    ///
    /// // 2. Pipeline — map the hub at /ws/agent, REPLACING the PoC mapping.
    /// //    Change this existing line:
    /// //        app.MapHub&lt;AgentHub&gt;("/ws/agent");
    /// //    to:
    /// app.MapHub&lt;AgentProtocolHub&gt;("/ws/agent");
    ///
    /// // 3. Remove the two PoC hub classes at the bottom of Program.cs — the
    /// //    `public class AgentHub(...) : Hub { ... }` and its companion
    /// //    `public class DashboardHub : Hub { ... }`. AgentProtocolHub
    /// //    supersedes AgentHub; BrowserHub (already mapped at /ws/dashboard by
    /// //    M2 slice 1) supersedes DashboardHub, whose only remaining consumer
    /// //    was the PoC AgentHub. Deleting them also lets you drop the now-unused
    /// //    `using Networker.Contracts;` (ProbeRunResult) import if nothing else
    /// //    references it.
    /// </code>
    ///
    /// <para><b>API-KEY auth — do NOT put this hub behind the JWT policy.</b>
    /// Agents authenticate with <c>?key=&lt;api_key&gt;</c> validated against
    /// <c>agent.api_key</c>, NOT a JWT. <see cref="AgentProtocolHub"/> is
    /// deliberately un-<c>[Authorize]</c>d and performs its own key check in
    /// <c>OnConnectedAsync</c>, aborting the connection on a missing/unknown key
    /// (the Rust <c>agent_ws_handler</c> returned 401 there). Because it is not
    /// under the JWT policy, no <c>OnMessageReceived</c> query-token shim is
    /// needed for <c>/ws/agent</c> — the JwtBearer WebSocket shim added for
    /// <c>/ws/dashboard</c> and <c>/ws/testers</c> keys off <c>access_token</c>,
    /// which agents never send, so the two auth schemes do not collide.</para>
    /// </summary>
    public static IServiceCollection AddAgentProtocol(this IServiceCollection services)
    {
        services.AddSingleton<AgentConnectionRegistry>();
        return services;
    }

    /// <summary>
    /// Convenience guard the integrator MAY call when hardening the JwtBearer
    /// WebSocket shim: ensures the <c>access_token</c>-from-query handler never
    /// fires for <c>/ws/agent</c> (agents authenticate by api-key, not JWT). The
    /// shim added by <c>EventBusServiceCollectionExtensions.ConfigureJwtBearerForWebSockets</c>
    /// already scopes itself to paths that carry an <c>access_token</c>, so this
    /// is belt-and-braces — included so the exclusion is explicit and testable.
    /// </summary>
    public static void ExcludeAgentPathFromJwtQueryAuth(JwtBearerOptions options)
    {
        options.Events ??= new JwtBearerEvents();
        var previous = options.Events.OnMessageReceived;
        options.Events.OnMessageReceived = async context =>
        {
            if (previous is not null)
            {
                await previous(context);
            }

            // If a prior shim lifted a token from the query for the agent path,
            // clear it — /ws/agent is api-key authenticated, never JWT.
            if (context.HttpContext.Request.Path.StartsWithSegments("/ws/agent"))
            {
                context.Token = null;
            }
        };
    }
}
