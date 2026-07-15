using System.Diagnostics;
using System.Globalization;
using System.Text.Json.Nodes;

namespace Networker.Endpoint;

/// <summary>
/// Whether HTTP/3 is advertised/served. Mirrors the Rust <c>http3</c> feature
/// (on by default). Toggle off with <c>ENDPOINT_HTTP3=0</c> / <c>false</c>.
/// </summary>
public static class Http3
{
    public static bool Enabled { get; set; } = true;
}

/// <summary>
/// Stamps every response with the server wall-clock timestamp, version, and
/// (when an H3 port is set) an <c>Alt-Svc</c> header. Ported from the Rust
/// <c>add_server_timestamp</c> middleware. Registered before the handler runs
/// via <c>OnStarting</c> so the headers land even on streamed responses.
/// </summary>
public sealed class ServerTimestampMiddleware
{
    private readonly RequestDelegate _next;
    private readonly AppState _state;

    public ServerTimestampMiddleware(RequestDelegate next, AppState state)
    {
        _next = next;
        _state = state;
    }

    public Task Invoke(HttpContext ctx)
    {
        ctx.Response.OnStarting(() =>
        {
            var ts = DateTimeOffset.UtcNow.ToString("yyyy-MM-ddTHH:mm:ss.ffffffzzz", CultureInfo.InvariantCulture);
            ctx.Response.Headers["x-networker-server-timestamp"] = ts;
            ctx.Response.Headers["x-networker-server-version"] = ServerInfo.Version;
            if (_state.H3Port is { } port)
                ctx.Response.Headers["alt-svc"] = $"h3=\":{port}\"; ma=86400";
            return Task.CompletedTask;
        });
        return _next(ctx);
    }
}

/// <summary>
/// Bearer-token auth, ported from the Rust <c>bench_auth_middleware</c>.
/// When <c>BENCH_API_TOKEN</c> is set, every request except <c>/health</c> must
/// carry a matching <c>Authorization: Bearer &lt;token&gt;</c>. A
/// <c>Server-Timing: auth;dur=X.X</c> metric is appended to every response.
/// </summary>
public sealed class BenchAuthMiddleware
{
    private readonly RequestDelegate _next;
    private static readonly string? Token = Environment.GetEnvironmentVariable("BENCH_API_TOKEN");

    public BenchAuthMiddleware(RequestDelegate next) => _next = next;

    public async Task Invoke(HttpContext ctx)
    {
        var t0 = Stopwatch.GetTimestamp();

        if (ctx.Request.Path == "/health")
        {
            await _next(ctx);
            return;
        }

        if (!string.IsNullOrEmpty(Token))
        {
            var auth = ctx.Request.Headers.Authorization.FirstOrDefault();
            var provided = auth is not null && auth.StartsWith("Bearer ", StringComparison.Ordinal)
                ? auth["Bearer ".Length..]
                : null;

            if (provided != Token)
            {
                var durMs401 = Stopwatch.GetElapsedTime(t0).TotalMilliseconds;
                ctx.Response.StatusCode = 401;
                ctx.Response.ContentType = "application/json";
                ctx.Response.Headers["server-timing"] = $"auth;dur={durMs401.ToString("0.0", CultureInfo.InvariantCulture)}";
                var err = new JsonObject { ["error"] = "unauthorized" };
                await ctx.Response.WriteAsync(err.ToJsonString());
                return;
            }
        }

        var durMs = Stopwatch.GetElapsedTime(t0).TotalMilliseconds;
        var authMetric = $"auth;dur={durMs.ToString("0.0", CultureInfo.InvariantCulture)}";

        // Append (or set) the auth timing before the response starts.
        ctx.Response.OnStarting(() =>
        {
            if (ctx.Response.Headers.TryGetValue("server-timing", out var existing) &&
                !string.IsNullOrEmpty(existing.ToString()))
            {
                ctx.Response.Headers["server-timing"] = $"{existing}, {authMetric}";
            }
            else
            {
                ctx.Response.Headers["server-timing"] = authMetric;
            }
            return Task.CompletedTask;
        });

        await _next(ctx);
    }
}
