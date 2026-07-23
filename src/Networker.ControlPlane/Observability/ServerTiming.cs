using System.Diagnostics;
using System.Globalization;

namespace Networker.ControlPlane.Observability;

/// <summary>
/// Stamps <c>X-Process-Time-Ms</c> on every response with the server-side
/// processing duration in milliseconds. The frontend's api client already reads
/// this header (<c>client.ts</c>) to split each request's wall-clock into
/// <c>server_ms</c> vs <c>network_ms</c> for the perf log — but the control plane
/// never emitted it, so every perf-log row had a null server_ms and slowness
/// could not be attributed to server vs client/network (perf sweep 2026-07).
///
/// <para>Registered first in the pipeline (before <c>UseErrorEnvelope</c>) so the
/// stopwatch wraps the whole request. The value is written in
/// <see cref="Microsoft.AspNetCore.Http.HttpResponse.OnStarting"/> — the last
/// moment before headers flush — so it reflects nearly the entire server time
/// yet is still set before the response is committed. Header-only, O(1), no
/// allocation on the hot path beyond the stopwatch.</para>
/// </summary>
public static class ServerTiming
{
    public const string HeaderName = "X-Process-Time-Ms";

    public static IApplicationBuilder UseServerTiming(this IApplicationBuilder app)
    {
        return app.Use(async (context, next) =>
        {
            var sw = Stopwatch.GetTimestamp();
            context.Response.OnStarting(state =>
            {
                var ctx = (Microsoft.AspNetCore.Http.HttpContext)state;
                var ms = Stopwatch.GetElapsedTime(sw).TotalMilliseconds;
                // Set (not append) — idempotent, and CORS-exposed below so the
                // browser can read it same-origin (it can) and cross-origin.
                ctx.Response.Headers[HeaderName] =
                    ms.ToString("F1", CultureInfo.InvariantCulture);
                return Task.CompletedTask;
            }, context);

            await next();
        });
    }
}
