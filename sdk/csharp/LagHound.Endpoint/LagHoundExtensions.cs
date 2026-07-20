using LagHound.Endpoint.Internal;
using Microsoft.AspNetCore.Builder;
using Microsoft.Extensions.DependencyInjection;

namespace LagHound.Endpoint;

/// <summary>
/// Registration + mount extensions for the LagHound endpoint (contract v1).
/// Two shapes: DI-based <c>AddLagHound</c> + <c>UseLagHound</c>, and the
/// one-call <c>MapLagHound</c> used in the README quickstart.
/// </summary>
public static class LagHoundExtensions
{
    /// <summary>
    /// Register LagHound with the DI container. Validates config and builds the
    /// per-process runtime eagerly (fail-closed at startup if no token — contract §2).
    /// </summary>
    public static IServiceCollection AddLagHound(this IServiceCollection services, Action<LagHoundOptions> configure)
    {
        ArgumentNullException.ThrowIfNull(services);
        ArgumentNullException.ThrowIfNull(configure);

        var options = new LagHoundOptions();
        configure(options);

        // Throws now (startup) rather than mounting open routes.
        var runtime = new LagHoundRuntime(options);
        services.AddSingleton(runtime);
        return services;
    }

    /// <summary>
    /// Mount the LagHound middleware. Requires a prior <see cref="AddLagHound"/>.
    /// Place early enough that limits/auth run before the host's own routing.
    /// </summary>
    public static IApplicationBuilder UseLagHound(this IApplicationBuilder app)
    {
        ArgumentNullException.ThrowIfNull(app);
        var runtime = app.ApplicationServices.GetService(typeof(LagHoundRuntime)) as LagHoundRuntime
            ?? throw new InvalidOperationException(
                "UseLagHound() requires services.AddLagHound(...) to have been called first.");
        return app.UseMiddleware<LagHoundMiddleware>(runtime);
    }

    /// <summary>
    /// One-call mount for apps that don't pre-register in DI (README quickstart).
    /// Builds the runtime from the supplied options and inserts the middleware.
    /// </summary>
    public static IApplicationBuilder MapLagHound(this IApplicationBuilder app, LagHoundOptions options)
    {
        ArgumentNullException.ThrowIfNull(app);
        ArgumentNullException.ThrowIfNull(options);
        var runtime = new LagHoundRuntime(options);
        return app.UseMiddleware<LagHoundMiddleware>(runtime);
    }
}
