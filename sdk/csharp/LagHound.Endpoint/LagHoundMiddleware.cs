using System.Diagnostics;
using System.Globalization;
using LagHound.Endpoint.Internal;
using Microsoft.AspNetCore.Http;

namespace LagHound.Endpoint;

/// <summary>
/// ASP.NET Core middleware implementing the LagHound endpoint contract v1.
/// Only requests under the configured prefix are handled; everything else is
/// passed straight through to the next middleware. Check order is fixed
/// (contract §5): kill switch → rate/concurrency limits → auth → route logic.
/// </summary>
internal sealed class LagHoundMiddleware
{
    private const string ContentTypeJson = "application/json";
    private const string ContentTypeOctet = "application/octet-stream";
    private const string CacheControl = "no-store, no-cache, must-revalidate";

    private readonly RequestDelegate _next;
    private readonly LagHoundRuntime _rt;

    public LagHoundMiddleware(RequestDelegate next, LagHoundRuntime rt)
    {
        _next = next;
        _rt = rt;
    }

    public async Task InvokeAsync(HttpContext ctx)
    {
        PathString path = ctx.Request.Path;
        if (!path.StartsWithSegments(_rt.Prefix, StringComparison.Ordinal, out PathString remainder))
        {
            await _next(ctx);
            return;
        }

        // Everything from here is a LagHound route. Any exception converts to a
        // 500 envelope confined to this route (contract §6.7) — never crashes the host.
        try
        {
            await HandleAsync(ctx, remainder.Value ?? "/");
        }
        catch (BadHttpRequestException)
        {
            // Client aborted / malformed body mid-drain — nothing to report.
            if (!ctx.Response.HasStarted)
            {
                await WriteEnvelopeAsync(ctx, StatusCodes.Status400BadRequest, "invalid_param", "invalid request", null, 0);
            }
        }
        catch (OperationCanceledException)
        {
            // Request aborted; let it unwind.
            throw;
        }
        catch (Exception)
        {
            if (!ctx.Response.HasStarted)
            {
                await WriteEnvelopeAsync(ctx, StatusCodes.Status500InternalServerError, "internal", "internal error", null, 0);
            }
        }
    }

    private async Task HandleAsync(HttpContext ctx, string subPath)
    {
        long started = Stopwatch.GetTimestamp();

        // 1. Kill switch → bare 404 (contract §6.5).
        if (KillSwitch.IsDisabled())
        {
            await BareNotFoundAsync(ctx);
            return;
        }

        // 2. Rate + concurrency limits (before auth, so brute-force is throttled).
        //    Unauthenticated rejections are bare 404s; authenticated get 429.
        bool authed = IsAuthenticated(ctx);

        string ip = ctx.Connection.RemoteIpAddress?.ToString() ?? "unknown";
        bool ipOk = _rt.PerIpLimiter.TryTake(ip);
        bool globalOk = _rt.GlobalLimiter.TryTake();
        if (!ipOk || !globalOk)
        {
            if (authed)
            {
                await WriteEnvelopeAsync(ctx, StatusCodes.Status429TooManyRequests, "rate_limited", "rate limit exceeded", retryAfterSeconds: 1, started);
            }
            else
            {
                await BareNotFoundAsync(ctx);
            }

            return;
        }

        using ConcurrencyGate.Lease? gate = _rt.ConcurrencyGate.TryAcquire();
        if (gate is null)
        {
            if (authed)
            {
                await WriteEnvelopeAsync(ctx, StatusCodes.Status429TooManyRequests, "rate_limited", "too many concurrent requests", retryAfterSeconds: 1, started);
            }
            else
            {
                await BareNotFoundAsync(ctx);
            }

            return;
        }

        // 3. Auth. Bad/missing token → bare 404 (contract §5), including /health.
        if (!authed)
        {
            await BareNotFoundAsync(ctx);
            return;
        }

        // 4. Route dispatch.
        string route = NormalizeSubPath(subPath);
        switch (route)
        {
            case "/health":
                await HealthAsync(ctx, started);
                break;
            case "/echo":
                if (_rt.EnableEcho)
                {
                    await EchoAsync(ctx, started);
                }
                else
                {
                    await BareNotFoundAsync(ctx);
                }

                break;
            case "/download":
                if (_rt.EnableDownload)
                {
                    await DownloadAsync(ctx, started);
                }
                else
                {
                    await BareNotFoundAsync(ctx);
                }

                break;
            case "/upload":
                if (_rt.EnableUpload)
                {
                    await UploadAsync(ctx, started);
                }
                else
                {
                    await BareNotFoundAsync(ctx);
                }

                break;
            case "/info":
                if (_rt.EnableInfo)
                {
                    await InfoAsync(ctx, started);
                }
                else
                {
                    await BareNotFoundAsync(ctx);
                }

                break;
            default:
                // Unknown subpath under the prefix → bare 404 (contract §7).
                await BareNotFoundAsync(ctx);
                break;
        }
    }

    // ---- routes ---------------------------------------------------------

    private async Task HealthAsync(HttpContext ctx, long started)
    {
        if (!ctx.Request.Method.Equals("GET", StringComparison.OrdinalIgnoreCase))
        {
            await MethodNotAllowedAsync(ctx, started);
            return;
        }

        await WriteJsonAsync(ctx, StatusCodes.Status200OK, JsonBodies.Health(_rt), started);
    }

    private async Task InfoAsync(HttpContext ctx, long started)
    {
        if (!ctx.Request.Method.Equals("GET", StringComparison.OrdinalIgnoreCase))
        {
            await MethodNotAllowedAsync(ctx, started);
            return;
        }

        await WriteJsonAsync(ctx, StatusCodes.Status200OK, JsonBodies.Info(_rt), started);
    }

    private async Task EchoAsync(HttpContext ctx, long started)
    {
        if (!ctx.Request.Method.Equals("GET", StringComparison.OrdinalIgnoreCase))
        {
            await MethodNotAllowedAsync(ctx, started);
            return;
        }

        // Reject oversized bodies on /echo (contract §3.2, §6.1) without draining.
        if (ctx.Request.ContentLength is long len && len > LagHoundRuntime.EchoBodyMaxBytes)
        {
            await WriteEnvelopeAsync(ctx, StatusCodes.Status413PayloadTooLarge, "payload_too_large", "payload too large", null, started);
            return;
        }

        await WriteJsonAsync(ctx, StatusCodes.Status200OK, _rt.EchoBody, started);
    }

    private async Task DownloadAsync(HttpContext ctx, long started)
    {
        if (!ctx.Request.Method.Equals("GET", StringComparison.OrdinalIgnoreCase))
        {
            await MethodNotAllowedAsync(ctx, started);
            return;
        }

        long requested = _rt.DownloadCapBytes;
        if (ctx.Request.Query.TryGetValue("bytes", out var raw))
        {
            string s = raw.ToString();
            if (!long.TryParse(s, NumberStyles.None, CultureInfo.InvariantCulture, out long parsed))
            {
                // Unparsable / negative (NumberStyles.None rejects sign + whitespace) → 400.
                await WriteEnvelopeAsync(ctx, StatusCodes.Status400BadRequest, "invalid_param", "invalid bytes parameter", null, started);
                return;
            }

            requested = parsed;
        }

        long effective = Math.Min(Math.Min(requested, _rt.DownloadCapBytes), LagHoundOptions.AbsoluteMaxBytes);

        // Transfer concurrency cap.
        using ConcurrencyGate.Lease? transfer = _rt.TransferGate.TryAcquire();
        if (transfer is null)
        {
            await WriteEnvelopeAsync(ctx, StatusCodes.Status429TooManyRequests, "rate_limited", "too many concurrent transfers", retryAfterSeconds: 1, started);
            return;
        }

        // Byte budget: reserve the effective size (contract §6.4).
        if (_rt.Budget is not null && !_rt.Budget.TryReserve(effective, out int retryAfter))
        {
            await WriteEnvelopeAsync(ctx, StatusCodes.Status429TooManyRequests, "rate_limited", "byte budget exhausted", retryAfter, started);
            return;
        }

        double appMs = ElapsedMs(started); // setup time before first chunk (contract §3.3).

        ctx.Response.StatusCode = StatusCodes.Status200OK;
        ctx.Response.ContentType = ContentTypeOctet;
        ctx.Response.ContentLength = effective;
        ctx.Response.Headers["X-LagHound-Bytes"] = effective.ToString(CultureInfo.InvariantCulture);
        SetCommonHeaders(ctx, appMs, null);

        byte[] buf = _rt.DownloadBuffer;
        long remaining = effective;
        Stream body = ctx.Response.Body;
        while (remaining > 0)
        {
            int chunk = (int)Math.Min(remaining, buf.Length);
            await body.WriteAsync(buf.AsMemory(0, chunk), ctx.RequestAborted);
            remaining -= chunk;
        }
    }

    private async Task UploadAsync(HttpContext ctx, long started)
    {
        if (!ctx.Request.Method.Equals("POST", StringComparison.OrdinalIgnoreCase))
        {
            await MethodNotAllowedAsync(ctx, started);
            return;
        }

        long cap = Math.Min(_rt.UploadCapBytes, LagHoundOptions.AbsoluteMaxBytes);

        using ConcurrencyGate.Lease? transfer = _rt.TransferGate.TryAcquire();
        if (transfer is null)
        {
            await WriteEnvelopeAsync(ctx, StatusCodes.Status429TooManyRequests, "rate_limited", "too many concurrent transfers", retryAfterSeconds: 1, started);
            return;
        }

        // Content-Length over cap → 413 WITHOUT reading the body (contract §3.4).
        if (ctx.Request.ContentLength is long declared && declared > cap)
        {
            await WriteEnvelopeAsync(ctx, StatusCodes.Status413PayloadTooLarge, "payload_too_large", "payload too large", null, started);
            return;
        }

        // Byte budget exhaustion check before the drain (contract §6.4).
        if (_rt.Budget is not null && _rt.Budget.IsExhausted(out int retryAfter))
        {
            await WriteEnvelopeAsync(ctx, StatusCodes.Status429TooManyRequests, "rate_limited", "byte budget exhausted", retryAfter, started);
            return;
        }

        // Drain-and-count, never buffer. Peak memory O(chunk) (contract §3.4).
        long recvStart = Stopwatch.GetTimestamp();
        byte[] scratch = new byte[LagHoundRuntime.ChunkBytes];
        long received = 0;
        bool truncated = false;
        Stream body = ctx.Request.Body;
        while (true)
        {
            int read = await body.ReadAsync(scratch, ctx.RequestAborted);
            if (read == 0)
            {
                break;
            }

            received += read;
            if (received > cap)
            {
                // Chunked/unknown length over cap → 413, stop reading, close connection.
                truncated = true;
                break;
            }
        }

        double recvMs = ElapsedMs(recvStart);
        _rt.Budget?.Record(received);

        if (truncated)
        {
            ctx.Response.Headers["Connection"] = "close";
            await WriteEnvelopeAsync(ctx, StatusCodes.Status413PayloadTooLarge, "payload_too_large", "payload too large", null, started);
            return;
        }

        double appMs = ElapsedMs(started) - recvMs;
        if (appMs < 0)
        {
            appMs = 0;
        }

        byte[] json = JsonBodies.UploadReceived(received);
        ctx.Response.StatusCode = StatusCodes.Status200OK;
        ctx.Response.ContentType = ContentTypeJson;
        ctx.Response.ContentLength = json.Length;
        ctx.Response.Headers["X-LagHound-Bytes"] = received.ToString(CultureInfo.InvariantCulture);
        // Server-Timing: recv;dur=<ms>, app;dur=<ms> (contract §3.4).
        ctx.Response.Headers["Server-Timing"] = ServerTimingHeader.Build(
            new[] { ("recv", recvMs), ("app", appMs), ("total", recvMs + appMs) },
            LagHoundMarks.Get(ctx));
        ctx.Response.Headers["Cache-Control"] = CacheControl;
        ctx.Response.Headers["Timing-Allow-Origin"] = "*";
        await ctx.Response.Body.WriteAsync(json, ctx.RequestAborted);
    }

    // ---- helpers --------------------------------------------------------

    private bool IsAuthenticated(HttpContext ctx)
    {
        // X-LagHound-Token wins when both present; the other is ignored (contract §5).
        if (ctx.Request.Headers.TryGetValue("X-LagHound-Token", out var custom) && custom.Count > 0)
        {
            return TokenMatches(custom[custom.Count - 1]);
        }

        if (ctx.Request.Headers.TryGetValue("Authorization", out var auth) && auth.Count > 0)
        {
            string v = auth[auth.Count - 1] ?? string.Empty;
            const string prefix = "Bearer ";
            if (v.StartsWith(prefix, StringComparison.Ordinal))
            {
                return TokenMatches(v.Substring(prefix.Length));
            }
        }

        return false;
    }

    private bool TokenMatches(string? presented)
    {
        if (presented is null)
        {
            presented = string.Empty;
        }

        return _rt.TokenMatches(System.Text.Encoding.UTF8.GetBytes(presented));
    }

    private static string NormalizeSubPath(string subPath)
    {
        if (string.IsNullOrEmpty(subPath))
        {
            return "/";
        }

        // Strip a single trailing slash so "/health/" matches "/health".
        if (subPath.Length > 1 && subPath[^1] == '/')
        {
            subPath = subPath[..^1];
        }

        return subPath;
    }

    private async Task MethodNotAllowedAsync(HttpContext ctx, long started)
        => await WriteEnvelopeAsync(ctx, StatusCodes.Status405MethodNotAllowed, "method_not_allowed", "method not allowed", null, started);

    private static async Task BareNotFoundAsync(HttpContext ctx)
    {
        // Bare, body-less 404 — no LagHound headers, no envelope, no Server-Timing (contract §5).
        ctx.Response.StatusCode = StatusCodes.Status404NotFound;
        await ctx.Response.CompleteAsync();
    }

    private async Task WriteJsonAsync(HttpContext ctx, int status, byte[] body, long started)
    {
        double appMs = ElapsedMs(started);
        ctx.Response.StatusCode = status;
        ctx.Response.ContentType = ContentTypeJson;
        ctx.Response.ContentLength = body.Length;
        SetCommonHeaders(ctx, appMs, LagHoundMarks.Get(ctx));
        await ctx.Response.Body.WriteAsync(body, ctx.RequestAborted);
    }

    private async Task WriteEnvelopeAsync(HttpContext ctx, int status, string code, string message, int? retryAfterSeconds, long started)
    {
        long? retryMs = retryAfterSeconds is int s ? s * 1000L : null;
        byte[] body = JsonBodies.Error(code, message, retryMs);
        double appMs = ElapsedMs(started);
        ctx.Response.StatusCode = status;
        ctx.Response.ContentType = ContentTypeJson;
        ctx.Response.ContentLength = body.Length;
        if (retryAfterSeconds is int ra)
        {
            ctx.Response.Headers["Retry-After"] = ra.ToString(CultureInfo.InvariantCulture);
        }

        SetCommonHeaders(ctx, appMs, LagHoundMarks.Get(ctx));
        await ctx.Response.Body.WriteAsync(body, ctx.RequestAborted);
    }

    private static void SetCommonHeaders(HttpContext ctx, double appMs, IReadOnlyList<KeyValuePair<string, double>>? marks)
    {
        // app + total (compat alias) on every enveloped/success response (contract §4.2).
        ctx.Response.Headers["Server-Timing"] = ServerTimingHeader.Build(
            new[] { ("app", appMs), ("total", appMs) },
            marks);
        ctx.Response.Headers["Cache-Control"] = CacheControl;
        ctx.Response.Headers["Timing-Allow-Origin"] = "*";
    }

    private static double ElapsedMs(long startTimestamp)
        => Stopwatch.GetElapsedTime(startTimestamp).TotalMilliseconds;
}
