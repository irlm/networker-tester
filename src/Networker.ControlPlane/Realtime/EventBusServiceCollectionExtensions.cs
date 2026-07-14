using Microsoft.AspNetCore.Authentication.JwtBearer;
using Microsoft.AspNetCore.SignalR;
using Microsoft.Extensions.DependencyInjection;

namespace Networker.ControlPlane.Realtime;

/// <summary>
/// DI + integration wiring for the browser event bus (the <c>/ws/dashboard</c>
/// live-updates channel with replay + sequence numbers).
///
/// <para><b>How Program.cs should wire this in</b> (the integrator does this —
/// these files intentionally do not touch Program.cs):</para>
/// <code>
/// // 1. Service registration (before builder.Build()):
/// builder.Services.AddSignalR();            // already present
/// builder.Services.AddDashboardEventBus();  // registers EventBus singleton
///
/// // 2. Map the hub (in the pipeline, after UseNetworkerAuth()):
/// app.MapHub&lt;BrowserHub&gt;("/ws/dashboard");
///
/// // NOTE: Program.cs currently maps its own DashboardHub at "/ws/dashboard".
/// // Replace that mapping with BrowserHub — the two cannot both own the same
/// // path. BrowserHub supersedes DashboardHub for the browser feed.
/// </code>
///
/// <para><b>JwtBearer-from-query REQUIREMENT (integrator must add this):</b></para>
/// WebSocket clients cannot set an <c>Authorization</c> header on the upgrade
/// request, so SignalR's JS client passes the JWT in the query string as
/// <c>?access_token=&lt;jwt&gt;</c>. For <see cref="BrowserHub"/>'s
/// <c>[Authorize]</c> to succeed, the existing JwtBearer options (configured in
/// <c>AuthExtensions.AddNetworkerAuth</c>) MUST pull the token from the query
/// string for hub paths. Add an <c>OnMessageReceived</c> event to the JwtBearer
/// options:
/// <code>
/// .AddJwtBearer(options =>
/// {
///     options.TokenValidationParameters = tokenService.ValidationParameters;
///     options.MapInboundClaims = false;
///     options.Events = new JwtBearerEvents
///     {
///         OnMessageReceived = context =>
///         {
///             var accessToken = context.Request.Query["access_token"];
///             var path = context.HttpContext.Request.Path;
///             if (!string.IsNullOrEmpty(accessToken) &amp;&amp;
///                 path.StartsWithSegments("/ws"))
///             {
///                 context.Token = accessToken;
///             }
///             return Task.CompletedTask;
///         }
///     };
/// });
/// </code>
/// Without this, the WebSocket negotiate/connect for <c>/ws/dashboard</c>
/// returns 401 because the token is only in the query string, never a header.
/// (<see cref="ConfigureJwtBearerForWebSockets"/> below packages this snippet as
/// a reusable helper the integrator may call instead of hand-copying it.)
/// </summary>
public static class EventBusServiceCollectionExtensions
{
    /// <summary>
    /// Register the <see cref="EventBus"/> as a singleton. Idempotent-safe to
    /// call once during service configuration. Requires <c>AddSignalR()</c> to
    /// have been called (the bus depends on <c>IHubContext&lt;BrowserHub&gt;</c>).
    /// </summary>
    public static IServiceCollection AddDashboardEventBus(this IServiceCollection services)
    {
        services.AddSingleton<EventBus>();
        return services;
    }

    /// <summary>
    /// Reusable JwtBearer configuration that lifts the token from the
    /// <c>access_token</c> query parameter for <c>/ws</c> paths — required for
    /// WebSocket auth (see the class remarks). The integrator can apply this to
    /// the JwtBearer options registered in <c>AuthExtensions</c>:
    /// <code>
    /// .AddJwtBearer(options =>
    /// {
    ///     options.TokenValidationParameters = tokenService.ValidationParameters;
    ///     options.MapInboundClaims = false;
    ///     EventBusServiceCollectionExtensions.ConfigureJwtBearerForWebSockets(options);
    /// });
    /// </code>
    /// </summary>
    public static void ConfigureJwtBearerForWebSockets(JwtBearerOptions options)
    {
        options.Events ??= new JwtBearerEvents();
        var previous = options.Events.OnMessageReceived;
        options.Events.OnMessageReceived = async context =>
        {
            if (previous is not null)
            {
                await previous(context);
            }

            // Only override if a prior handler hasn't already set the token.
            if (string.IsNullOrEmpty(context.Token))
            {
                var accessToken = context.Request.Query["access_token"];
                var path = context.HttpContext.Request.Path;
                if (!string.IsNullOrEmpty(accessToken) && path.StartsWithSegments("/ws"))
                {
                    context.Token = accessToken;
                }
            }
        };
    }
}
