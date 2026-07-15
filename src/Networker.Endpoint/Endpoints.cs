using System.Diagnostics;
using System.Globalization;
using System.IO.Compression;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using Microsoft.AspNetCore.Http.Extensions;

namespace Networker.Endpoint;

/// <summary>
/// All HTTP route handlers for the diagnostics endpoint, ported faithfully from
/// the Rust <c>routes.rs</c>. Registered via <see cref="MapEndpoints"/>.
/// </summary>
public static class Endpoints
{
    // Compact JSON, no indentation — matches serde_json's default output.
    // UnsafeRelaxedJsonEscaping so '+', '<', '>', '&' are emitted literally
    // (serde_json only escapes '"', '\\', and control chars).
    private static readonly JsonSerializerOptions Compact = new(JsonSerializerDefaults.Web)
    {
        WriteIndented = false,
        Encoder = System.Text.Encodings.Web.JavaScriptEncoder.UnsafeRelaxedJsonEscaping,
    };

    /// <summary>RFC3339 timestamp with offset, matching chrono <c>Utc::now().to_rfc3339()</c>.</summary>
    private static string Rfc3339Now() =>
        DateTimeOffset.UtcNow.ToString("yyyy-MM-ddTHH:mm:ss.ffffffzzz", CultureInfo.InvariantCulture);

    private static IResult JsonRaw(string json, int status = 200) =>
        Results.Text(json, "application/json", Encoding.UTF8, status);

    public static void MapEndpoints(WebApplication app)
    {
        app.MapGet("/", Landing_);
        app.MapGet("/health", Health);
        app.MapPost("/echo", EchoPost);
        app.MapGet("/echo", EchoGet);
        app.MapGet("/download", Download);
        app.MapPost("/upload", Upload);
        app.MapGet("/delay", Delay);
        app.MapGet("/headers", HeadersEcho);
        app.MapGet("/status/{code}", StatusCode);
        app.MapGet("/http-version", HttpVersion);
        app.MapGet("/info", ServerInfoHandler);
        app.MapGet("/page", PageManifest);
        app.MapGet("/browser-page", BrowserPage);
        app.MapGet("/asset", AssetHandler);
        // ── JSON API benchmark endpoints ──
        app.MapGet("/api/users", ApiUsers);
        app.MapPost("/api/transform", ApiTransform);
        app.MapGet("/api/aggregate", ApiAggregate);
        app.MapGet("/api/search", ApiSearch);
        app.MapPost("/api/upload/process", ApiUploadProcess);
        app.MapGet("/api/delayed", ApiDelayed);
        app.MapGet("/api/validate", ApiValidate);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Landing page
    // ─────────────────────────────────────────────────────────────────────────

    private static string FormatUptime(ulong secs)
    {
        var d = secs / 86400;
        var h = (secs % 86400) / 3600;
        var m = (secs % 3600) / 60;
        var s = secs % 60;
        if (d > 0) return $"{d}d {h}h {m}m";
        if (h > 0) return $"{h}h {m}m {s}s";
        if (m > 0) return $"{m}m {s}s";
        return $"{s}s";
    }

    private static IResult Landing_(AppState state)
    {
        var version = ServerInfo.Version;
        var elapsed = state.UptimeSecs();
        var uptime = FormatUptime(elapsed);
        var hostname = HostnameResolver.Get();
        var timestamp = DateTime.UtcNow.ToString("yyyy-MM-dd HH:mm:ss", CultureInfo.InvariantCulture) + " UTC";
        var started = DateTime.UtcNow.AddSeconds(-(double)elapsed)
            .ToString("yyyy-MM-dd HH:mm:ss", CultureInfo.InvariantCulture) + " UTC";

        var h3PortDisplay = state.H3Port?.ToString() ?? "n/a";
        var h3Proto = state.H3Port.HasValue ? "<span class=\"proto\">HTTP/3</span>" : "";

        var sb = new StringBuilder(8 * 1024);
        sb.Append(Landing.Head);

        sb.Append($"<h1>networker-endpoint</h1>\n<div class=\"meta\">v{version} &middot; {hostname}</div>\n<div class=\"status\"><span class=\"dot\"></span>running &nbsp; uptime {uptime}</div>\n");

        sb.Append("<div class=\"grid\">\n");

        sb.Append($"<div class=\"card\">\n   <div class=\"card-title\">Ports</div>\n   <div class=\"row\"><span class=\"lbl\">HTTP</span><span class=\"val\">:{state.HttpPort}</span></div>\n   <div class=\"row\"><span class=\"lbl\">HTTPS / H2</span><span class=\"val\">:{state.HttpsPort}</span></div>\n   <div class=\"row\"><span class=\"lbl\">HTTP/3 QUIC</span><span class=\"val\">{h3PortDisplay}</span></div>\n   <div class=\"row\"><span class=\"lbl\">UDP echo</span><span class=\"val\">:{state.UdpPort}</span></div>\n   <div class=\"row\"><span class=\"lbl\">UDP throughput</span><span class=\"val\">:{state.UdpThroughputPort}</span></div>\n </div>\n");

        sb.Append($"<div class=\"card\">\n   <div class=\"card-title\">Protocols</div>\n   <div class=\"proto-list\">\n     <span class=\"proto\">HTTP/1.1</span>\n     <span class=\"proto\">HTTP/2</span>\n     {h3Proto}\n     <span class=\"proto\">UDP</span>\n   </div>\n </div>\n");

        sb.Append($"<div class=\"card\">\n   <div class=\"card-title\">Server</div>\n   <div class=\"row\"><span class=\"lbl\">Version</span><span class=\"val\">{version}</span></div>\n   <div class=\"row\"><span class=\"lbl\">Started</span><span class=\"val\">{started}</span></div>\n   <div class=\"row\"><span class=\"lbl\">Now</span><span class=\"val\">{timestamp}</span></div>\n </div>\n");

        sb.Append("</div>\n");

        sb.Append("<div class=\"card full\">\n   <div class=\"card-title\">Endpoints</div>\n   <table>\n     <thead><tr><th>Path</th><th>Method</th><th>Description</th></tr></thead>\n     <tbody>\n       <tr><td>/</td><td class=\"method\">GET</td><td class=\"desc\">This status page</td></tr>\n       <tr><td>/health</td><td class=\"method\">GET</td><td class=\"desc\">Health check — 200 + JSON</td></tr>\n       <tr><td>/info</td><td class=\"method\">GET</td><td class=\"desc\">Server capabilities as JSON</td></tr>\n       <tr><td>/echo</td><td class=\"method\">GET / POST</td><td class=\"desc\">Echo request body and headers</td></tr>\n       <tr><td>/download</td><td class=\"method\">GET</td><td class=\"desc\">Stream N zero bytes — ?bytes=N</td></tr>\n       <tr><td>/upload</td><td class=\"method\">POST</td><td class=\"desc\">Drain request body, return byte count</td></tr>\n       <tr><td>/delay</td><td class=\"method\">GET</td><td class=\"desc\">Delay response by N ms — ?ms=N (max 30 s)</td></tr>\n       <tr><td>/headers</td><td class=\"method\">GET</td><td class=\"desc\">Echo all request headers as JSON</td></tr>\n       <tr><td>/status/:code</td><td class=\"method\">GET</td><td class=\"desc\">Return specified HTTP status code</td></tr>\n       <tr><td>/http-version</td><td class=\"method\">GET</td><td class=\"desc\">Return HTTP version used by the client</td></tr>\n       <tr><td>/page</td><td class=\"method\">GET</td><td class=\"desc\">Page-load asset manifest — ?assets=N&amp;bytes=B</td></tr>\n       <tr><td>/browser-page</td><td class=\"method\">GET</td><td class=\"desc\">HTML page with img tags for browser probes</td></tr>\n       <tr><td>/asset</td><td class=\"method\">GET</td><td class=\"desc\">Single binary asset — ?id=X&amp;bytes=B</td></tr>\n       <tr><td>/api/users</td><td class=\"method\">GET</td><td class=\"desc\">Paginated users — ?page=N&amp;sort=field&amp;order=asc</td></tr>\n       <tr><td>/api/transform</td><td class=\"method\">POST</td><td class=\"desc\">SHA-256 hash fields, reverse values</td></tr>\n       <tr><td>/api/aggregate</td><td class=\"method\">GET</td><td class=\"desc\">Time-series stats — ?range=start,end</td></tr>\n       <tr><td>/api/search</td><td class=\"method\">GET</td><td class=\"desc\">Regex search — ?q=term&amp;limit=N</td></tr>\n       <tr><td>/api/upload/process</td><td class=\"method\">POST</td><td class=\"desc\">CRC32 + SHA-256 + zlib compress body</td></tr>\n       <tr><td>/api/delayed</td><td class=\"method\">GET</td><td class=\"desc\">Controlled delay — ?ms=N&amp;work=light</td></tr>\n       <tr><td>/api/validate</td><td class=\"method\">GET</td><td class=\"desc\">Endpoint output checksums — ?seed=N</td></tr>\n     </tbody>\n   </table>\n </div>\n");

        sb.Append($"<div class=\"footer\">   <a href=\"/health\">/health</a> &nbsp;&middot;&nbsp;    <a href=\"/info\">/info</a>    &nbsp;&middot;&nbsp; networker-endpoint v{version} </div>\n");

        sb.Append(Landing.Foot);

        return Results.Text(sb.ToString(), "text/html; charset=utf-8", Encoding.UTF8, 200);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Core handlers
    // ─────────────────────────────────────────────────────────────────────────

    private static IResult Health()
    {
        var obj = new JsonObject
        {
            ["status"] = "ok",
            ["timestamp"] = Rfc3339Now(),
            ["service"] = ServerInfo.Service,
            ["version"] = ServerInfo.Version,
        };
        return JsonRaw(obj.ToJsonString(Compact));
    }

    private static IResult EchoGet(HttpRequest req)
    {
        var hdrs = new JsonObject();
        foreach (var (k, v) in req.Headers)
            hdrs[k.ToLowerInvariant()] = v.ToString();
        var obj = new JsonObject
        {
            ["method"] = "GET",
            ["headers"] = hdrs,
            ["body_bytes"] = 0,
        };
        return JsonRaw(obj.ToJsonString(Compact));
    }

    private static async Task EchoPost(HttpContext ctx)
    {
        var body = await ReadBodyAsync(ctx.Request);
        var bodyLen = body.Length;
        var headerCount = ctx.Request.Headers.Count;

        if (bodyLen <= 1_048_576)
        {
            ctx.Response.StatusCode = 200;
            ctx.Response.ContentType = "application/octet-stream";
            ctx.Response.Headers["x-echo-body-bytes"] = bodyLen.ToString();
            ctx.Response.Headers["x-echo-received-headers"] = headerCount.ToString();
            await ctx.Response.Body.WriteAsync(body);
        }
        else
        {
            ctx.Response.StatusCode = 413;
            await ctx.Response.WriteAsync("Payload too large (> 1 MiB)");
        }
    }

    private const long DownloadCap = 2L * 1024 * 1024 * 1024;
    private const int ChunkSize = 64 * 1024;

    private static async Task Download(HttpContext ctx)
    {
        var n = Math.Min(ParseQueryLong(ctx.Request, "bytes") ?? 1024, DownloadCap);
        var t0 = Stopwatch.GetTimestamp();

        var procMs = Stopwatch.GetElapsedTime(t0).TotalMilliseconds;
        var timing = $"proc;dur={procMs.ToString("0.000", CultureInfo.InvariantCulture)}";

        ctx.Response.StatusCode = 200;
        ctx.Response.ContentType = "application/octet-stream";
        ctx.Response.ContentLength = n;
        ctx.Response.Headers["x-download-bytes"] = n.ToString();
        ctx.Response.Headers["server-timing"] = timing;

        // Stream zero bytes in 64 KiB chunks without buffering the full payload.
        var chunk = new byte[ChunkSize];
        long remaining = n;
        while (remaining > 0)
        {
            var toWrite = (int)Math.Min(remaining, ChunkSize);
            await ctx.Response.Body.WriteAsync(chunk.AsMemory(0, toWrite));
            remaining -= toWrite;
        }
    }

    private static async Task Upload(HttpContext ctx)
    {
        string? requestId = ctx.Request.Headers.TryGetValue("x-networker-request-id", out var rid)
            ? rid.ToString()
            : null;

        var t0 = Stopwatch.GetTimestamp();
        long receivedBytes = 0;
        var buf = new byte[ChunkSize];
        int read;
        while ((read = await ctx.Request.Body.ReadAsync(buf)) > 0)
            receivedBytes += read;
        var recvMs = Stopwatch.GetElapsedTime(t0).TotalMilliseconds;

        var obj = new JsonObject
        {
            ["received_bytes"] = receivedBytes,
            ["timestamp"] = Rfc3339Now(),
        };

        ctx.Response.StatusCode = 200;
        ctx.Response.ContentType = "application/json";
        ctx.Response.Headers["server-timing"] = $"recv;dur={recvMs.ToString("0.000", CultureInfo.InvariantCulture)}";
        ctx.Response.Headers["x-networker-received-bytes"] = receivedBytes.ToString();
        if (requestId is not null)
            ctx.Response.Headers["x-networker-request-id"] = requestId;

        await ctx.Response.WriteAsync(obj.ToJsonString(Compact));
    }

    private static async Task<IResult> Delay(HttpRequest req)
    {
        var ms = Math.Min(ParseQueryLong(req, "ms") ?? 0, 30_000);
        await Task.Delay((int)ms);
        var obj = new JsonObject
        {
            ["delayed_ms"] = ms,
            ["timestamp"] = Rfc3339Now(),
        };
        return JsonRaw(obj.ToJsonString(Compact));
    }

    private static IResult HeadersEcho(HttpRequest req)
    {
        var map = new JsonObject();
        foreach (var (k, v) in req.Headers)
            map[k.ToLowerInvariant()] = v.ToString();
        return JsonRaw(map.ToJsonString(Compact));
    }

    private static IResult StatusCode(string code)
    {
        // Rust: StatusCode::from_u16(code).unwrap_or(BAD_REQUEST) — the JSON
        // "status" field always reflects the raw requested code.
        var parsedCode = ushort.TryParse(code, out var c) ? c : (ushort)0;
        var (status, reason) = ResolveStatus(parsedCode);
        var obj = new JsonObject
        {
            ["status"] = parsedCode,
            ["description"] = reason,
        };
        return JsonRaw(obj.ToJsonString(Compact), status);
    }

    private static (int status, string reason) ResolveStatus(ushort code)
    {
        // Valid HTTP status range for http::StatusCode is 100..=999.
        if (code is < 100 or > 999)
            return (400, ReasonPhrases.Get(400));
        var reason = ReasonPhrases.Get(code);
        return (code, reason);
    }

    private static IResult HttpVersion(HttpRequest req)
    {
        var version = req.Protocol switch
        {
            "HTTP/0.9" => "HTTP/0.9",
            "HTTP/1.0" => "HTTP/1.0",
            "HTTP/1.1" => "HTTP/1.1",
            "HTTP/2" => "HTTP/2",
            "HTTP/3" => "HTTP/3",
            _ => "Unknown",
        };
        var obj = new JsonObject
        {
            ["version"] = version,
            ["timestamp"] = Rfc3339Now(),
        };
        return JsonRaw(obj.ToJsonString(Compact));
    }

    private static IResult ServerInfoHandler(AppState state)
    {
        var protocols = new JsonArray { "HTTP/1.1", "HTTP/2" };
        if (Http3.Enabled)
            protocols.Add("HTTP/3");

        var endpoints = new JsonArray
        {
            "/health", "/echo", "/download", "/upload",
            "/delay", "/headers", "/status/:code", "/http-version", "/info",
            "/api/users", "/api/transform", "/api/aggregate", "/api/search",
            "/api/upload/process", "/api/delayed", "/api/validate",
        };

        var system = JsonSerializer.SerializeToNode(state.SystemMeta, Compact);
        var region = state.SystemMeta.Region is null ? null : JsonValue.Create(state.SystemMeta.Region);

        var obj = new JsonObject
        {
            ["service"] = ServerInfo.Service,
            ["version"] = ServerInfo.Version,
            ["protocols"] = protocols,
            ["http3"] = Http3.Enabled,
            ["endpoints"] = endpoints,
            ["system"] = system,
            ["region"] = region,
            ["uptime_secs"] = state.UptimeSecs(),
            ["timestamp"] = Rfc3339Now(),
        };
        return JsonRaw(obj.ToJsonString(Compact));
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Page-load simulation
    // ─────────────────────────────────────────────────────────────────────────

    private static IResult PageManifest(HttpRequest req)
    {
        var n = Math.Min((int)(ParseQueryLong(req, "assets") ?? 20), 500);
        var b = (int)(ParseQueryLong(req, "bytes") ?? 10_240);
        var assets = new JsonArray();
        for (var i = 0; i < n; i++)
            assets.Add($"/asset?id={i}&bytes={b}");
        var obj = new JsonObject
        {
            ["asset_count"] = n,
            ["asset_bytes"] = b,
            ["assets"] = assets,
        };
        return JsonRaw(obj.ToJsonString(Compact));
    }

    private static IResult BrowserPage(HttpRequest req)
    {
        var n = Math.Min((int)(ParseQueryLong(req, "assets") ?? 20), 500);
        var b = (int)(ParseQueryLong(req, "bytes") ?? 10_240);
        var sb = new StringBuilder();
        sb.Append("<!DOCTYPE html>\n         <html><head><title>Networker Page Load Test</title><link rel=\"icon\" href=\"data:,\"></head>\n         <body>\n");
        for (var i = 0; i < n; i++)
            sb.Append($"<img src=\"/asset?id={i}&bytes={b}\" width=\"1\" height=\"1\" alt=\"\">\n");
        sb.Append("</body></html>\n");
        return Results.Text(sb.ToString(), "text/html; charset=utf-8", Encoding.UTF8, 200);
    }

    private static async Task AssetHandler(HttpContext ctx)
    {
        var n = Math.Min((int)(ParseQueryLong(ctx.Request, "bytes") ?? 10_240), 100 * 1024 * 1024);
        ctx.Response.StatusCode = 200;
        ctx.Response.ContentType = "application/octet-stream";
        ctx.Response.ContentLength = n;
        // Rust allocates vec![0u8; n]; stream in chunks to avoid a giant alloc.
        var chunk = new byte[Math.Min(n, ChunkSize)];
        var remaining = n;
        while (remaining > 0)
        {
            var toWrite = Math.Min(remaining, chunk.Length);
            await ctx.Response.Body.WriteAsync(chunk.AsMemory(0, toWrite));
            remaining -= toWrite;
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // JSON API benchmark endpoints
    // ─────────────────────────────────────────────────────────────────────────

    private static void ApplyBenchHeaders(HttpResponse resp, double durMs)
    {
        resp.Headers["server-timing"] = $"app;dur={durMs.ToString("0.0", CultureInfo.InvariantCulture)}";
        resp.Headers["cache-control"] = "no-store, no-cache, must-revalidate";
        resp.Headers["timing-allow-origin"] = "*";
        resp.Headers["access-control-allow-origin"] = "*";
    }

    private static async Task ApiUsers(HttpContext ctx)
    {
        var t0 = Stopwatch.GetTimestamp();
        var req = ctx.Request;
        var page = Math.Max(ParseQueryULong(req, "page") ?? 1, 1);
        var sortField = req.Query["sort"].FirstOrDefault() ?? "id";
        var ascending = (req.Query["order"].FirstOrDefault() ?? "asc") != "desc";

        List<JsonNode?> users;
        var data = BenchData.Instance;
        if (data is not null)
        {
            var start = (int)((page - 1) * 100);
            if (start < data.Users.Count)
            {
                var end = Math.Min(start + 100, data.Users.Count);
                users = data.Users.GetRange(start, end - start)
                    .Select(u => u?.DeepClone()).ToList();
            }
            else
            {
                users = new List<JsonNode?>();
            }
        }
        else
        {
            users = PrngFallback.GenUsers(page);
        }

        // Sort by requested field (mirrors the Rust comparator, incl. the
        // "name" branch's self-comparison quirk which is effectively a no-op).
        users.Sort((a, b) =>
        {
            int cmp = sortField switch
            {
                "name" => string.CompareOrdinal(Str(a, "name"), Str(b, "name")),
                "email" => string.CompareOrdinal(Str(a, "email"), Str(b, "email")),
                "score" => F64(a, "score").CompareTo(F64(b, "score")),
                "created_at" => string.CompareOrdinal(Str(a, "created_at"), Str(b, "created_at")),
                _ => U64(a, "id").CompareTo(U64(b, "id")),
            };
            return ascending ? cmp : -cmp;
        });

        var paginated = new JsonArray();
        foreach (var u in users.Take(20))
            paginated.Add(u?.DeepClone());

        var durMs = Stopwatch.GetElapsedTime(t0).TotalMilliseconds;
        ApplyBenchHeaders(ctx.Response, durMs);
        ctx.Response.StatusCode = 200;
        ctx.Response.ContentType = "application/json";
        await ctx.Response.WriteAsync(paginated.ToJsonString(Compact));
    }

    private static async Task ApiTransform(HttpContext ctx)
    {
        var t0 = Stopwatch.GetTimestamp();
        JsonObject? body = null;
        try
        {
            var raw = await ReadBodyAsync(ctx.Request);
            body = JsonNode.Parse(raw)?.AsObject();
        }
        catch { /* invalid JSON => 400-ish; Rust would reject too */ }

        if (body is null)
        {
            ctx.Response.StatusCode = 400;
            return;
        }

        var hashedFields = new JsonArray();
        if (body["fields"] is JsonArray fields)
        {
            foreach (var f in fields)
            {
                var s = f?.GetValue<string>() ?? "";
                var hash = Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(s)));
                hashedFields.Add(hash);
            }
        }

        var reversedValues = new JsonArray();
        if (body["values"] is JsonArray values)
        {
            var items = values.Select(v => v?.DeepClone()).ToList();
            items.Reverse();
            foreach (var v in items)
                reversedValues.Add(v);
        }

        ulong seed = body["seed"] is { } sn && sn.GetValueKind() == JsonValueKind.Number
            ? sn.GetValue<ulong>()
            : 0;

        var result = new JsonObject
        {
            ["seed"] = seed,
            ["hashed_fields"] = hashedFields,
            ["reversed_values"] = reversedValues,
        };

        var durMs = Stopwatch.GetElapsedTime(t0).TotalMilliseconds;
        ApplyBenchHeaders(ctx.Response, durMs);
        ctx.Response.StatusCode = 200;
        ctx.Response.ContentType = "application/json";
        await ctx.Response.WriteAsync(result.ToJsonString(Compact));
    }

    private static async Task ApiAggregate(HttpContext ctx)
    {
        var t0 = Stopwatch.GetTimestamp();
        var range = ctx.Request.Query["range"].FirstOrDefault();
        ulong start = 1;
        if (range is not null)
        {
            var parts = range.Split(',');
            if (parts.Length > 0 && ulong.TryParse(parts[0], out var s)) start = s;
        }

        List<double> values;
        var data = BenchData.Instance;
        if (data is not null)
        {
            values = data.Timeseries
                .Select(v => v?["value"])
                .Where(v => v is not null && v.GetValueKind() == JsonValueKind.Number)
                .Select(v => v!.GetValue<double>())
                .ToList();
        }
        else
        {
            values = PrngFallback.GenTimeseries(start);
        }
        values.Sort();

        var n = (double)values.Count;
        var sum = values.Sum();
        var mean = sum / n;
        var p50 = values[(int)(values.Count * 0.50)];
        var p95 = values[(int)(values.Count * 0.95)];
        var max = values.Count > 0 ? values[^1] : 0.0;

        var chunkSize = values.Count / 5;
        var categories = new JsonArray();
        for (var i = 0; i < 5; i++)
        {
            var chunk = values.GetRange(i * chunkSize, chunkSize);
            var catSum = chunk.Sum();
            var catMean = catSum / chunk.Count;
            categories.Add(new JsonObject
            {
                ["category"] = $"q{i + 1}",
                ["count"] = chunk.Count,
                ["mean"] = Round2(catMean),
                ["min"] = Round2(chunk[0]),
                ["max"] = Round2(chunk[^1]),
            });
        }

        var result = new JsonObject
        {
            ["total_points"] = 10_000,
            ["mean"] = Round2(mean),
            ["p50"] = Round2(p50),
            ["p95"] = Round2(p95),
            ["max"] = Round2(max),
            ["categories"] = categories,
        };

        var durMs = Stopwatch.GetElapsedTime(t0).TotalMilliseconds;
        ApplyBenchHeaders(ctx.Response, durMs);
        ctx.Response.StatusCode = 200;
        ctx.Response.ContentType = "application/json";
        await ctx.Response.WriteAsync(result.ToJsonString(Compact));
    }

    private static async Task ApiSearch(HttpContext ctx)
    {
        var t0 = Stopwatch.GetTimestamp();
        var query = ctx.Request.Query["q"].FirstOrDefault() ?? "test";
        var limit = Math.Min((int)(ParseQueryLong(ctx.Request, "limit") ?? 20), 100);

        List<string> items;
        var data = BenchData.Instance;
        items = data is not null ? new List<string>(data.SearchCorpus) : PrngFallback.GenSearchCorpus();

        Regex? re = null;
        try { re = new Regex(query); } catch { re = null; }

        var scored = new List<(int pos, string item)>();
        foreach (var item in items)
        {
            int? matched;
            if (re is not null)
            {
                var m = re.Match(item);
                matched = m.Success ? m.Index : null;
            }
            else
            {
                var idx = item.IndexOf(query, StringComparison.Ordinal);
                matched = idx >= 0 ? idx : null;
            }
            if (matched.HasValue)
                scored.Add((matched.Value, item));
        }

        // Sort by match position, then alphabetically (ordinal).
        scored.Sort((a, b) =>
        {
            var c = a.pos.CompareTo(b.pos);
            return c != 0 ? c : string.CompareOrdinal(a.item, b.item);
        });

        var results = new JsonArray();
        var rank = 1;
        foreach (var (pos, item) in scored.Take(limit))
        {
            results.Add(new JsonObject
            {
                ["rank"] = rank++,
                ["item"] = item,
                ["match_position"] = pos,
            });
        }

        var result = new JsonObject
        {
            ["query"] = query,
            ["total_matches"] = scored.Count,
            ["returned"] = results.Count,
            ["results"] = results,
        };

        var durMs = Stopwatch.GetElapsedTime(t0).TotalMilliseconds;
        ApplyBenchHeaders(ctx.Response, durMs);
        ctx.Response.StatusCode = 200;
        ctx.Response.ContentType = "application/json";
        await ctx.Response.WriteAsync(result.ToJsonString(Compact));
    }

    private static async Task ApiUploadProcess(HttpContext ctx)
    {
        var t0 = Stopwatch.GetTimestamp();
        var bodyData = await ReadBodyAsync(ctx.Request);
        var originalSize = bodyData.Length;

        var crc = Crc32.Hash(bodyData);
        var sha = Convert.ToHexStringLower(SHA256.HashData(bodyData));

        byte[] compressed;
        using (var ms = new MemoryStream())
        {
            using (var zlib = new ZLibStream(ms, CompressionLevel.Optimal, leaveOpen: true))
                zlib.Write(bodyData, 0, bodyData.Length);
            compressed = ms.ToArray();
        }

        var result = new JsonObject
        {
            ["original_size"] = originalSize,
            ["compressed_size"] = compressed.Length,
            ["crc32"] = crc.ToString("x8"),
            ["sha256"] = sha,
        };

        var durMs = Stopwatch.GetElapsedTime(t0).TotalMilliseconds;
        ApplyBenchHeaders(ctx.Response, durMs);
        ctx.Response.StatusCode = 200;
        ctx.Response.ContentType = "application/json";
        await ctx.Response.WriteAsync(result.ToJsonString(Compact));
    }

    private static async Task ApiDelayed(HttpContext ctx)
    {
        var t0 = Stopwatch.GetTimestamp();
        var ms = Math.Clamp(ParseQueryLong(ctx.Request, "ms") ?? 10, 1, 100);
        await Task.Delay((int)ms);
        var actualMs = Stopwatch.GetElapsedTime(t0).TotalMilliseconds;

        var result = new JsonObject
        {
            ["requested_ms"] = ms,
            ["actual_ms"] = Round2(actualMs),
        };

        var durMs = Stopwatch.GetElapsedTime(t0).TotalMilliseconds;
        ApplyBenchHeaders(ctx.Response, durMs);
        ctx.Response.StatusCode = 200;
        ctx.Response.ContentType = "application/json";
        await ctx.Response.WriteAsync(result.ToJsonString(Compact));
    }

    private static async Task ApiValidate(HttpContext ctx)
    {
        var t0 = Stopwatch.GetTimestamp();
        var seed = ParseQueryULong(ctx.Request, "seed") ?? 42;

        JsonObject result;
        var data = BenchData.Instance;
        if (data is not null)
        {
            result = new JsonObject
            {
                ["seed"] = seed,
                ["checksums"] = (JsonObject)data.ExpectedChecksums.DeepClone(),
            };
        }
        else
        {
            var users = PrngFallback.GenUsers(seed);
            var usersJson = new JsonArray(users.Select(u => u?.DeepClone()).ToArray()).ToJsonString(Compact);
            var usersHash = Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(usersJson)));

            var values = PrngFallback.GenTimeseries(seed);
            values.Sort();
            var mean = values.Sum() / values.Count;
            var aggStr = mean.ToString("F6", CultureInfo.InvariantCulture);
            var aggregateHash = Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(aggStr)));

            var transformCheck = Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes("test")));
            var transformHash = Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(transformCheck)));

            var items = PrngFallback.GenSearchCorpus();
            var searchJson = new JsonArray(items.Select(i => (JsonNode?)i).ToArray()).ToJsonString(Compact);
            var searchHash = Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(searchJson)));

            result = new JsonObject
            {
                ["seed"] = seed,
                ["checksums"] = new JsonObject
                {
                    ["users"] = usersHash,
                    ["aggregate"] = aggregateHash,
                    ["transform"] = transformHash,
                    ["search"] = searchHash,
                },
            };
        }

        var durMs = Stopwatch.GetElapsedTime(t0).TotalMilliseconds;
        ApplyBenchHeaders(ctx.Response, durMs);
        ctx.Response.StatusCode = 200;
        ctx.Response.ContentType = "application/json";
        await ctx.Response.WriteAsync(result.ToJsonString(Compact));
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Helpers
    // ─────────────────────────────────────────────────────────────────────────

    private static double Round2(double v) => Math.Round(v * 100.0) / 100.0;

    private static string Str(JsonNode? node, string key) =>
        node?[key] is { } n && n.GetValueKind() == JsonValueKind.String ? n.GetValue<string>() : "";

    private static double F64(JsonNode? node, string key) =>
        node?[key] is { } n && n.GetValueKind() == JsonValueKind.Number ? n.GetValue<double>() : 0.0;

    private static ulong U64(JsonNode? node, string key)
    {
        if (node?[key] is { } n && n.GetValueKind() == JsonValueKind.Number)
        {
            try { return n.GetValue<ulong>(); } catch { return 0; }
        }
        return 0;
    }

    private static long? ParseQueryLong(HttpRequest req, string key)
    {
        var v = req.Query[key].FirstOrDefault();
        return long.TryParse(v, out var l) ? l : null;
    }

    private static ulong? ParseQueryULong(HttpRequest req, string key)
    {
        var v = req.Query[key].FirstOrDefault();
        return ulong.TryParse(v, out var l) ? l : null;
    }

    private static async Task<byte[]> ReadBodyAsync(HttpRequest req)
    {
        using var ms = new MemoryStream();
        await req.Body.CopyToAsync(ms);
        return ms.ToArray();
    }
}
