// ─────────────────────────────────────────────────────────────────────────────
// C# reference API — Application Benchmark mode.
// Contract: benchmarks/shared/API-SPEC.md (frozen v1, shape family C).
//
// SINGLE SOURCE OF TRUTH: benchmarks/reference-apis/csharp-template/Program.cs
// The eight modern variant copies (csharp-net6 … csharp-net10-aot) are
// byte-identical to this file, written by csharp-template/generate-variants.py.
// Do NOT edit a variant copy — edit the template and regenerate
// (`./generate-variants.py`; verify with `./generate-variants.py --check`).
//
// Per-variant differences are limited to (API-SPEC.md §8):
//   - TFM / AssemblyName / PublishAot   → generated .csproj
//   - HTTP/3                            → #if NET7_0_OR_GREATER (net6 has no
//                                         stable Kestrel H3)
//   - CreateSlimBuilder                 → #if NET8_0_OR_GREATER (the supported
//                                         Native AOT builder; net6/7 lack it)
//   - X509CertificateLoader             → #if NET9_0_OR_GREATER (pre-9 uses the
//                                         X509Certificate2 ctor)
// The runtime identity reported by /health is the AssemblyName set by the
// generated .csproj (e.g. "csharp-net8-aot"), so this source stays identical.
//
// Language level: keep this file C# 10 compatible — the oldest toolchain in
// the ladder (the net6 SDK image) compiles it. No collection expressions,
// no raw string literals, no primary constructors.
// ─────────────────────────────────────────────────────────────────────────────

using System.Diagnostics;
using System.Globalization;
using System.IO.Compression;
using System.Security.Cryptography;
using System.Security.Cryptography.X509Certificates;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using System.Text.Json.Serialization.Metadata;
using System.Text.RegularExpressions;
using Microsoft.AspNetCore.Server.Kestrel.Core;

internal static class Program
{
    private static void Main(string[] args)
    {
        // §5.1: runtime identity == the reference directory name. The generated
        // .csproj pins <AssemblyName> to the variant directory name.
        var runtimeId = typeof(Program).Assembly.GetName().Name ?? "csharp";

        // §2: dataset load failure is FATAL — no PRNG fallback.
        var data = BenchData.LoadOrExit();

        // §3: BENCH_WORKERS. For C# the knob is advisory (Kestrel + ThreadPool
        // saturate all logical CPUs in-process); nproc and the effective value
        // are logged so runs can record them next to results.
        var nproc = Environment.ProcessorCount;
        var workersEnv = Environment.GetEnvironmentVariable("BENCH_WORKERS");
        var workers = int.TryParse(workersEnv, out var w) && w > 0 ? w : nproc;

        var port = int.TryParse(Environment.GetEnvironmentVariable("BENCH_PORT"), out var p) ? p : 8443;
        var certDir = Environment.GetEnvironmentVariable("BENCH_CERT_DIR") ?? "/opt/bench";
        var certPath = Path.Combine(certDir, "cert.pem");
        var keyPath = Path.Combine(certDir, "key.pem");
        var hasTls = File.Exists(certPath) && File.Exists(keyPath);

#if NET8_0_OR_GREATER
        var builder = WebApplication.CreateSlimBuilder(args);
        builder.WebHost.UseKestrelHttpsConfiguration();
#else
        var builder = WebApplication.CreateBuilder(args);
#endif

        builder.WebHost.ConfigureKestrel(options =>
        {
            if (hasTls)
            {
                using var pem = X509Certificate2.CreateFromPemFile(certPath, keyPath);
                var pkcs12 = pem.Export(X509ContentType.Pkcs12);
#if NET9_0_OR_GREATER
                var cert = X509CertificateLoader.LoadPkcs12(pkcs12, null);
#else
                var cert = new X509Certificate2(pkcs12);
#endif
                options.ListenAnyIP(port, listen =>
                {
#if NET7_0_OR_GREATER
                    // Kestrel adds the Alt-Svc h3 advertisement automatically.
                    listen.Protocols = HttpProtocols.Http1AndHttp2AndHttp3;
#else
                    listen.Protocols = HttpProtocols.Http1AndHttp2;
#endif
                    listen.UseHttps(cert);
                });
            }
            else
            {
                // Application-mode topology (audit F8): when the proxy
                // terminates TLS and no certs are present, serve plain HTTP.
                options.ListenAnyIP(port, listen =>
                {
                    listen.Protocols = HttpProtocols.Http1AndHttp2;
                });
            }
        });

        var app = builder.Build();

        Handlers.Init(runtimeId, data);
        Console.WriteLine(
            $"{runtimeId} reference API listening on port {port} " +
            $"(tls={hasTls.ToString().ToLowerInvariant()}, nproc={nproc}, " +
            $"bench_workers={workers} [advisory: Kestrel + ThreadPool in-process scheduler], " +
            $"dataset={data.SourcePath})");

        // §1: bearer auth on every route except /health.
        var token = Environment.GetEnvironmentVariable("BENCH_API_TOKEN");
        if (!string.IsNullOrEmpty(token))
        {
            var expected = "Bearer " + token;
            var rejection = Encoding.UTF8.GetBytes("{\"error\":\"unauthorized\"}");
            app.Use(async (ctx, next) =>
            {
                if (ctx.Request.Path != "/health")
                {
                    var auth = ctx.Request.Headers.Authorization.FirstOrDefault() ?? "";
                    if (auth != expected)
                    {
                        var resp = ctx.Response;
                        resp.StatusCode = 401;
                        resp.ContentType = "application/json";
                        resp.ContentLength = rejection.Length;
                        // §10.7: /api/* responses (including rejections) carry
                        // the benchmark headers.
                        Handlers.SetBenchHeaders(resp, 0.0);
                        await resp.Body.WriteAsync(rejection);
                        return;
                    }
                }
                await next();
            });
        }

        // GET routes also answer HEAD (the validator probes headers with
        // `curl -I`; axum — the canonical baseline — does the same).
        // NOTE: the template must stay C# 10 compatible — the net6/net7 SDK
        // images compile with the C# 10 Roslyn (no collection expressions).
        string[] getHead = { "GET", "HEAD" };
        app.MapMethods("/health", getHead, Handlers.Health);
        app.MapMethods("/download/{size}", getHead, Handlers.Download);
        app.MapPost("/upload", Handlers.Upload);
        app.MapMethods("/api/users", getHead, Handlers.ApiUsers);
        app.MapPost("/api/transform", Handlers.ApiTransform);
        app.MapMethods("/api/aggregate", getHead, Handlers.ApiAggregate);
        app.MapMethods("/api/search", getHead, Handlers.ApiSearch);
        app.MapPost("/api/upload/process", Handlers.ApiUploadProcess);
        app.MapMethods("/api/delayed", getHead, Handlers.ApiDelayed);
        app.MapMethods("/api/validate", getHead, Handlers.ApiValidate);

        app.Run();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Route handlers (API-SPEC.md §5)
// ─────────────────────────────────────────────────────────────────────────────

internal static class Handlers
{
    private const long DownloadCap = 2_147_483_648; // §5.2: 2 GiB
    private const int DownloadChunk = 8192;         // §5.2: 8 KiB chunks
    private const byte DownloadFill = 0x42;         // §5.2: 'B'

    private static readonly byte[] FillChunk = CreateFillChunk();
    private static byte[] HealthBytes = Array.Empty<byte>();
    private static BenchData Data = null!;

    private static byte[] CreateFillChunk()
    {
        var chunk = new byte[DownloadChunk];
        Array.Fill(chunk, DownloadFill);
        return chunk;
    }

    public static void Init(string runtimeId, BenchData data)
    {
        Data = data;
        // §5.1: constant-work /health — the body is precomputed once; two
        // requests must return byte-identical bodies.
        HealthBytes = JsonSerializer.SerializeToUtf8Bytes(
            new HealthResponse
            {
                Status = "ok",
                Runtime = runtimeId,
                Version = Environment.Version.ToString(),
            },
            BenchJson.Default.HealthResponse);
    }

    // §5.1 GET /health — infrastructure endpoint, never ranked, auth-exempt.
    public static Task Health(HttpContext ctx)
    {
        var resp = ctx.Response;
        resp.StatusCode = 200;
        resp.ContentType = "application/json";
        resp.ContentLength = HealthBytes.Length;
        if (HttpMethods.IsHead(ctx.Request.Method))
            return Task.CompletedTask;
        return resp.Body.WriteAsync(HealthBytes, 0, HealthBytes.Length);
    }

    // §5.2 GET /download/{size} — exactly `size` bytes of 0x42 in 8 KiB chunks.
    public static async Task Download(HttpContext ctx)
    {
        var sw = Stopwatch.StartNew();
        var raw = ctx.Request.RouteValues["size"]?.ToString() ?? "";
        // NumberStyles.None rejects signs — negative or non-integer → 400.
        if (!long.TryParse(raw, NumberStyles.None, CultureInfo.InvariantCulture, out var size))
        {
            await WriteError(ctx, 400, "invalid size");
            return;
        }
        if (size > DownloadCap) size = DownloadCap; // clamp above the 2 GiB cap

        var resp = ctx.Response;
        resp.StatusCode = 200;
        resp.ContentType = "application/octet-stream";
        resp.ContentLength = size;
        resp.Headers["X-Download-Bytes"] = size.ToString(CultureInfo.InvariantCulture);
        resp.Headers["Server-Timing"] = "proc;dur=" + Ms1(sw);
        if (HttpMethods.IsHead(ctx.Request.Method))
            return;

        var remaining = size;
        while (remaining > 0)
        {
            var n = (int)Math.Min(remaining, DownloadChunk);
            await resp.Body.WriteAsync(FillChunk.AsMemory(0, n));
            remaining -= n;
        }
    }

    // §5.3 POST /upload — drain the body without wholesale buffering.
    public static async Task Upload(HttpContext ctx)
    {
        var sw = Stopwatch.StartNew();
        long received = 0;
        var buf = new byte[DownloadChunk];
        int read;
        while ((read = await ctx.Request.Body.ReadAsync(buf)) > 0)
            received += read;

        var resp = ctx.Response;
        resp.Headers["Server-Timing"] = "recv;dur=" + Ms1(sw);
        resp.Headers["X-Networker-Received-Bytes"] = received.ToString(CultureInfo.InvariantCulture);
        if (ctx.Request.Headers.TryGetValue("X-Networker-Request-Id", out var rid))
            resp.Headers["X-Networker-Request-Id"] = (string?)rid;

        var payload = JsonSerializer.SerializeToUtf8Bytes(
            new UploadResponse { ReceivedBytes = received },
            BenchJson.Default.UploadResponse);
        resp.StatusCode = 200;
        resp.ContentType = "application/json";
        resp.ContentLength = payload.Length;
        await resp.Body.WriteAsync(payload);
    }

    // §5.4 GET /api/users?page=N&sort=<field>&order=<asc|desc>
    public static Task ApiUsers(HttpContext ctx)
    {
        var sw = Stopwatch.StartNew();
        var q = ctx.Request.Query;

        var page = QueryLong(ctx, "page") ?? 1;
        if (page < 1) page = 1; // minimum clamp 1
        var sort = q["sort"].FirstOrDefault() ?? "id";
        var desc = (q["order"].FirstOrDefault() ?? "") == "desc";

        // 100-user window; page beyond the dataset yields [] with 200.
        // lastPage is compared first so (page-1)*100 can never overflow.
        List<BenchUser> window;
        var lastPage = (Data.Users.Count + 99) / 100;
        if (page > lastPage)
        {
            window = new List<BenchUser>();
        }
        else
        {
            var start = (int)((page - 1) * 100);
            var count = Math.Min(100, Data.Users.Count - start);
            window = Data.Users.GetRange(start, count);
        }

        // OrderBy is a stable sort — dataset order breaks ties (§5.4); string
        // fields compare ordinally (bytewise for this ASCII dataset), score as
        // float64. `desc` reverses the ascending result.
        var sorted = (sort switch
        {
            "name" => window.OrderBy(u => u.Name, StringComparer.Ordinal),
            "email" => window.OrderBy(u => u.Email, StringComparer.Ordinal),
            "score" => window.OrderBy(u => u.Score),
            "created_at" => window.OrderBy(u => u.CreatedAt, StringComparer.Ordinal),
            _ => window.OrderBy(u => u.Id),
        }).ToList();
        if (desc) sorted.Reverse();

        var result = sorted.Take(20).ToList();
        return WriteJson(ctx, sw, result, BenchJson.Default.ListBenchUser);
    }

    // §5.5 POST /api/transform — SHA-256 the field strings, reverse values.
    public static async Task ApiTransform(HttpContext ctx)
    {
        var sw = Stopwatch.StartNew();

        JsonDocument doc;
        try
        {
            doc = await JsonDocument.ParseAsync(ctx.Request.Body);
        }
        catch (JsonException)
        {
            await WriteApiError(ctx, sw, 400, "invalid json");
            return;
        }

        using (doc)
        {
            var root = doc.RootElement;
            if (root.ValueKind != JsonValueKind.Object)
            {
                await WriteApiError(ctx, sw, 400, "expected a JSON object");
                return;
            }

            long seed = 0; // §5.5 default
            if (root.TryGetProperty("seed", out var seedEl) &&
                seedEl.ValueKind == JsonValueKind.Number &&
                seedEl.TryGetInt64(out var seedVal))
            {
                seed = seedVal;
            }

            var hashedFields = new List<string>();
            if (root.TryGetProperty("fields", out var fields) && fields.ValueKind == JsonValueKind.Array)
            {
                foreach (var el in fields.EnumerateArray())
                {
                    var s = el.ValueKind == JsonValueKind.String ? el.GetString()! : el.GetRawText();
                    hashedFields.Add(HexLower(SHA256.HashData(Encoding.UTF8.GetBytes(s))));
                }
            }

            var reversed = new List<JsonElement>();
            if (root.TryGetProperty("values", out var values) && values.ValueKind == JsonValueKind.Array)
            {
                foreach (var el in values.EnumerateArray())
                    reversed.Add(el.Clone()); // pass through unmodified
                reversed.Reverse();
            }

            var result = new TransformResponse
            {
                Seed = seed,
                HashedFields = hashedFields,
                ReversedValues = reversed,
            };
            await WriteJson(ctx, sw, result, BenchJson.Default.TransformResponse);
        }
    }

    // §5.6 GET /api/aggregate — full-series stats; `range` accepted + ignored.
    public static Task ApiAggregate(HttpContext ctx)
    {
        var sw = Stopwatch.StartNew();

        // Copy the cached dataset-order values, then run the normative
        // float64 algorithm: sort → sequential sum over the SORTED values →
        // truncated-index percentiles → quintile categories.
        var values = (double[])Data.TimeseriesValues.Clone();
        Array.Sort(values);
        var n = values.Length;

        double sum = 0.0;
        for (var i = 0; i < n; i++)
            sum += values[i];

        var chunk = n / 5;
        var categories = new List<AggregateCategory>(5);
        for (var i = 0; i < 5; i++)
        {
            double chunkSum = 0.0;
            for (var j = i * chunk; j < (i + 1) * chunk; j++)
                chunkSum += values[j];
            categories.Add(new AggregateCategory
            {
                Category = "q" + (i + 1),
                Count = chunk,
                Mean = R2(chunkSum / chunk),
                Min = R2(values[i * chunk]),
                Max = R2(values[(i + 1) * chunk - 1]),
            });
        }

        var result = new AggregateResponse
        {
            TotalPoints = n,
            Mean = R2(sum / n),
            P50 = R2(values[(int)(n * 0.50)]),
            P95 = R2(values[(int)(n * 0.95)]),
            Max = R2(values[n - 1]),
            Categories = categories,
        };
        return WriteJson(ctx, sw, result, BenchJson.Default.AggregateResponse);
    }

    // §5.7 GET /api/search?q=<term>&limit=N — case-sensitive regex over the
    // corpus, literal-substring fallback when the pattern does not compile.
    public static Task ApiSearch(HttpContext ctx)
    {
        var sw = Stopwatch.StartNew();
        var query = ctx.Request.Query["q"].FirstOrDefault() ?? "test";
        var limit = QueryLong(ctx, "limit") ?? 20;
        if (limit > 100) limit = 100;
        var take = (int)Math.Max(0, limit);

        Regex? re = null;
        try { re = new Regex(query); }
        catch (ArgumentException) { /* literal fallback below */ }

        var scored = new List<(int Pos, string Item)>();
        foreach (var item in Data.SearchCorpus)
        {
            int pos;
            if (re is not null)
            {
                var m = re.Match(item);
                if (!m.Success) continue;
                pos = m.Index;
            }
            else
            {
                pos = item.IndexOf(query, StringComparison.Ordinal);
                if (pos < 0) continue;
            }
            scored.Add((pos, item));
        }

        scored.Sort((a, b) =>
        {
            var c = a.Pos.CompareTo(b.Pos);
            return c != 0 ? c : string.CompareOrdinal(a.Item, b.Item);
        });

        var results = new List<SearchResult>(Math.Min(take, scored.Count));
        for (var i = 0; i < scored.Count && i < take; i++)
        {
            results.Add(new SearchResult
            {
                Rank = i + 1,
                Item = scored[i].Item,
                MatchPosition = scored[i].Pos,
            });
        }

        var result = new SearchResponse
        {
            Query = query,
            TotalMatches = scored.Count, // before truncation
            Returned = results.Count,
            Results = results,
        };
        return WriteJson(ctx, sw, result, BenchJson.Default.SearchResponse);
    }

    // §5.8 POST /api/upload/process — CRC-32 + SHA-256 + zlib level 6.
    public static async Task ApiUploadProcess(HttpContext ctx)
    {
        var sw = Stopwatch.StartNew();
        byte[] body;
        using (var ms = new MemoryStream())
        {
            await ctx.Request.Body.CopyToAsync(ms);
            body = ms.ToArray();
        }

        var crc = Crc32.Hash(body);
        var sha = HexLower(SHA256.HashData(body));

        // ZLibStream = RFC 1950 (zlib header + adler); CompressionLevel.Optimal
        // maps to zlib's default level 6 — the §5.8 canonical algorithm.
        long compressedSize;
        using (var outMs = new MemoryStream())
        {
            using (var zlib = new ZLibStream(outMs, CompressionLevel.Optimal, leaveOpen: true))
                zlib.Write(body, 0, body.Length);
            compressedSize = outMs.Length;
        }

        var result = new UploadProcessResponse
        {
            OriginalSize = body.Length,
            CompressedSize = compressedSize,
            Crc32 = crc.ToString("x8", CultureInfo.InvariantCulture),
            Sha256 = sha,
        };
        await WriteJson(ctx, sw, result, BenchJson.Default.UploadProcessResponse);
    }

    // §5.9 GET /api/delayed?ms=N — async timer delay, clamped to [1, 100];
    // `work` is reserved: accepted and ignored.
    public static async Task ApiDelayed(HttpContext ctx)
    {
        var sw = Stopwatch.StartNew();
        var ms = QueryLong(ctx, "ms") ?? 10;
        ms = Math.Clamp(ms, 1, 100);
        await Task.Delay((int)ms);

        var result = new DelayedResponse
        {
            RequestedMs = ms,
            ActualMs = R2(sw.Elapsed.TotalMilliseconds),
        };
        await WriteJson(ctx, sw, result, BenchJson.Default.DelayedResponse);
    }

    // §5.10 GET /api/validate?seed=N — echo the dataset's expected_checksums.
    public static Task ApiValidate(HttpContext ctx)
    {
        var sw = Stopwatch.StartNew();
        var seed = QueryLong(ctx, "seed") ?? 42;
        var result = new ValidateResponse
        {
            Seed = seed,
            Checksums = Data.ExpectedChecksums,
        };
        return WriteJson(ctx, sw, result, BenchJson.Default.ValidateResponse);
    }

    // ── helpers ──────────────────────────────────────────────────────────────

    /// <summary>§5.6 rounding: half away from zero to 2 decimals
    /// (bit-identical to the generator's `floor(x*100 + 0.5) / 100`).</summary>
    private static double R2(double x) => Math.Floor(x * 100.0 + 0.5) / 100.0;

    private static string Ms1(Stopwatch sw) =>
        sw.Elapsed.TotalMilliseconds.ToString("0.0", CultureInfo.InvariantCulture);

    private static string HexLower(byte[] bytes) =>
        Convert.ToHexString(bytes).ToLowerInvariant();

    private static long? QueryLong(HttpContext ctx, string key)
    {
        var v = ctx.Request.Query[key].FirstOrDefault();
        return long.TryParse(v, NumberStyles.Integer, CultureInfo.InvariantCulture, out var l) ? l : null;
    }

    /// <summary>§1 benchmark headers, required on every /api/* response.</summary>
    public static void SetBenchHeaders(HttpResponse resp, double durMs)
    {
        resp.Headers["Server-Timing"] =
            "app;dur=" + durMs.ToString("0.0", CultureInfo.InvariantCulture);
        resp.Headers["Cache-Control"] = "no-store, no-cache, must-revalidate";
        resp.Headers["Timing-Allow-Origin"] = "*";
        resp.Headers["Access-Control-Allow-Origin"] = "*";
    }

    private static Task WriteJson<T>(HttpContext ctx, Stopwatch sw, T value, JsonTypeInfo<T> typeInfo)
    {
        var payload = JsonSerializer.SerializeToUtf8Bytes(value, typeInfo);
        var resp = ctx.Response;
        resp.StatusCode = 200;
        resp.ContentType = "application/json";
        resp.ContentLength = payload.Length;
        SetBenchHeaders(resp, sw.Elapsed.TotalMilliseconds);
        if (HttpMethods.IsHead(ctx.Request.Method))
            return Task.CompletedTask;
        return resp.Body.WriteAsync(payload, 0, payload.Length);
    }

    private static Task WriteApiError(HttpContext ctx, Stopwatch sw, int status, string message)
    {
        var payload = JsonSerializer.SerializeToUtf8Bytes(
            new ErrorResponse { Error = message }, BenchJson.Default.ErrorResponse);
        var resp = ctx.Response;
        resp.StatusCode = status;
        resp.ContentType = "application/json";
        resp.ContentLength = payload.Length;
        SetBenchHeaders(resp, sw.Elapsed.TotalMilliseconds);
        return resp.Body.WriteAsync(payload, 0, payload.Length);
    }

    private static Task WriteError(HttpContext ctx, int status, string message)
    {
        var payload = JsonSerializer.SerializeToUtf8Bytes(
            new ErrorResponse { Error = message }, BenchJson.Default.ErrorResponse);
        var resp = ctx.Response;
        resp.StatusCode = status;
        resp.ContentType = "application/json";
        resp.ContentLength = payload.Length;
        return resp.Body.WriteAsync(payload, 0, payload.Length);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared dataset (API-SPEC.md §2) — load is FATAL on failure, no PRNG fallback
// ─────────────────────────────────────────────────────────────────────────────

internal sealed class BenchData
{
    public string SourcePath = "";
    public List<BenchUser> Users = new();
    public List<string> SearchCorpus = new();
    public double[] TimeseriesValues = Array.Empty<double>();
    public Dictionary<string, string> ExpectedChecksums = new();

    private static readonly string[] RequiredChecksumKeys =
        { "users_page1", "aggregate_default", "search_network_top10", "transform_input0" };

    public static BenchData LoadOrExit()
    {
        var envPath = Environment.GetEnvironmentVariable("BENCH_DATA_PATH");
        string path;
        if (!string.IsNullOrEmpty(envPath))
        {
            // §2: when BENCH_DATA_PATH is set, that exact file must exist and
            // parse — no fallback.
            path = envPath;
        }
        else
        {
            string[] candidates =
            {
                "/opt/bench/bench-data.json",
                Path.Combine(AppContext.BaseDirectory, "..", "shared", "bench-data.json"),
                Path.Combine("..", "shared", "bench-data.json"),
            };
            var found = candidates.FirstOrDefault(File.Exists);
            if (found is null)
                Fatal("bench-data.json not found (BENCH_DATA_PATH unset; tried: " +
                      string.Join(", ", candidates) + ")");
            path = found!;
        }

        BenchDatasetFile? file = null;
        try
        {
            // byte[] overload: available on every TFM in the ladder (the sync
            // Stream overload only exists on net7+).
            file = JsonSerializer.Deserialize(File.ReadAllBytes(path), BenchJson.Default.BenchDatasetFile);
        }
        catch (Exception ex)
        {
            Fatal($"failed to load {path}: {ex.Message}");
        }
        if (file is null)
            Fatal($"failed to load {path}: null document");

        // §2: verify the counts; exit non-zero on mismatch.
        var f = file!;
        if (f.Version != 2)
            Fatal($"{path}: _version is {f.Version}, expected 2");
        if (f.Users is not { Count: 100 })
            Fatal($"{path}: users count is {f.Users?.Count ?? 0}, expected 100");
        if (f.SearchCorpus is not { Count: 1000 })
            Fatal($"{path}: search_corpus count is {f.SearchCorpus?.Count ?? 0}, expected 1000");
        if (f.Timeseries is not { Count: 10000 })
            Fatal($"{path}: timeseries count is {f.Timeseries?.Count ?? 0}, expected 10000");
        if (f.TransformInputs is not { Count: 10 })
            Fatal($"{path}: transform_inputs count is {f.TransformInputs?.Count ?? 0}, expected 10");
        foreach (var key in RequiredChecksumKeys)
        {
            if (f.ExpectedChecksums is null || !f.ExpectedChecksums.ContainsKey(key))
                Fatal($"{path}: expected_checksums missing key '{key}'");
        }

        return new BenchData
        {
            SourcePath = path,
            Users = f.Users!,
            SearchCorpus = f.SearchCorpus!,
            TimeseriesValues = f.Timeseries!.Select(t => t.Value).ToArray(),
            ExpectedChecksums = f.ExpectedChecksums!,
        };
    }

    private static void Fatal(string message)
    {
        Console.Error.WriteLine("FATAL: " + message);
        Environment.Exit(1);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DTOs + System.Text.Json source generation (idiomatic, AOT/trim-safe)
// ─────────────────────────────────────────────────────────────────────────────

/// <summary>
/// Writes integral doubles as "N.0" so that canonical-JSON validators
/// (Python <c>json.load</c> → <c>json.dumps</c>, API-SPEC.md §7) parse them as
/// floats, not ints. The frozen dataset's aggregate response really contains
/// an exact 39.0 (q2 mean) — System.Text.Json's shortest-round-trip "39"
/// would canonicalize to int 39 and break the pinned checksum.
/// </summary>
internal sealed class PyFloatConverter : JsonConverter<double>
{
    public override double Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
        => reader.GetDouble();

    public override void Write(Utf8JsonWriter writer, double value, JsonSerializerOptions options)
    {
        if (double.IsFinite(value) && Math.Floor(value) == value && Math.Abs(value) < 1e15)
            writer.WriteRawValue(((long)value).ToString(CultureInfo.InvariantCulture) + ".0",
                skipInputValidation: true);
        else
            writer.WriteNumberValue(value);
    }
}

internal sealed class BenchDatasetFile
{
    [JsonPropertyName("_version")] public int Version { get; set; }
    [JsonPropertyName("users")] public List<BenchUser>? Users { get; set; }
    [JsonPropertyName("search_corpus")] public List<string>? SearchCorpus { get; set; }
    [JsonPropertyName("timeseries")] public List<TimeseriesPoint>? Timeseries { get; set; }
    [JsonPropertyName("transform_inputs")] public List<JsonElement>? TransformInputs { get; set; }
    [JsonPropertyName("expected_checksums")] public Dictionary<string, string>? ExpectedChecksums { get; set; }
}

// §2 user schema: id/name/email/score/created_at — no age/active/department.
internal sealed class BenchUser
{
    [JsonPropertyName("id")] public long Id { get; set; }
    [JsonPropertyName("name")] public string Name { get; set; } = "";
    [JsonPropertyName("email")] public string Email { get; set; } = "";
    [JsonPropertyName("score")]
    [JsonConverter(typeof(PyFloatConverter))]
    public double Score { get; set; }
    [JsonPropertyName("created_at")] public string CreatedAt { get; set; } = "";
}

// §2 timeseries entries are objects, not bare floats.
internal sealed class TimeseriesPoint
{
    [JsonPropertyName("ts")] public long Ts { get; set; }
    [JsonPropertyName("value")] public double Value { get; set; }
    [JsonPropertyName("category")] public string Category { get; set; } = "";
}

internal sealed class HealthResponse
{
    [JsonPropertyName("status")] public string Status { get; set; } = "";
    [JsonPropertyName("runtime")] public string Runtime { get; set; } = "";
    [JsonPropertyName("version")] public string Version { get; set; } = "";
}

internal sealed class UploadResponse
{
    [JsonPropertyName("received_bytes")] public long ReceivedBytes { get; set; }
}

internal sealed class TransformResponse
{
    [JsonPropertyName("seed")] public long Seed { get; set; }
    [JsonPropertyName("hashed_fields")] public List<string> HashedFields { get; set; } = new();
    [JsonPropertyName("reversed_values")] public List<JsonElement> ReversedValues { get; set; } = new();
}

internal sealed class AggregateResponse
{
    [JsonPropertyName("total_points")] public int TotalPoints { get; set; }
    [JsonPropertyName("mean")]
    [JsonConverter(typeof(PyFloatConverter))]
    public double Mean { get; set; }
    [JsonPropertyName("p50")]
    [JsonConverter(typeof(PyFloatConverter))]
    public double P50 { get; set; }
    [JsonPropertyName("p95")]
    [JsonConverter(typeof(PyFloatConverter))]
    public double P95 { get; set; }
    [JsonPropertyName("max")]
    [JsonConverter(typeof(PyFloatConverter))]
    public double Max { get; set; }
    [JsonPropertyName("categories")] public List<AggregateCategory> Categories { get; set; } = new();
}

internal sealed class AggregateCategory
{
    [JsonPropertyName("category")] public string Category { get; set; } = "";
    [JsonPropertyName("count")] public int Count { get; set; }
    [JsonPropertyName("mean")]
    [JsonConverter(typeof(PyFloatConverter))]
    public double Mean { get; set; }
    [JsonPropertyName("min")]
    [JsonConverter(typeof(PyFloatConverter))]
    public double Min { get; set; }
    [JsonPropertyName("max")]
    [JsonConverter(typeof(PyFloatConverter))]
    public double Max { get; set; }
}

internal sealed class SearchResponse
{
    [JsonPropertyName("query")] public string Query { get; set; } = "";
    [JsonPropertyName("total_matches")] public int TotalMatches { get; set; }
    [JsonPropertyName("returned")] public int Returned { get; set; }
    [JsonPropertyName("results")] public List<SearchResult> Results { get; set; } = new();
}

internal sealed class SearchResult
{
    [JsonPropertyName("rank")] public int Rank { get; set; }
    [JsonPropertyName("item")] public string Item { get; set; } = "";
    [JsonPropertyName("match_position")] public int MatchPosition { get; set; }
}

internal sealed class UploadProcessResponse
{
    [JsonPropertyName("original_size")] public int OriginalSize { get; set; }
    [JsonPropertyName("compressed_size")] public long CompressedSize { get; set; }
    [JsonPropertyName("crc32")] public string Crc32 { get; set; } = "";
    [JsonPropertyName("sha256")] public string Sha256 { get; set; } = "";
}

internal sealed class DelayedResponse
{
    [JsonPropertyName("requested_ms")] public long RequestedMs { get; set; }
    [JsonPropertyName("actual_ms")]
    [JsonConverter(typeof(PyFloatConverter))]
    public double ActualMs { get; set; }
}

internal sealed class ValidateResponse
{
    [JsonPropertyName("seed")] public long Seed { get; set; }
    [JsonPropertyName("checksums")] public Dictionary<string, string> Checksums { get; set; } = new();
}

internal sealed class ErrorResponse
{
    [JsonPropertyName("error")] public string Error { get; set; } = "";
}

[JsonSourceGenerationOptions(WriteIndented = false)]
[JsonSerializable(typeof(BenchDatasetFile))]
[JsonSerializable(typeof(List<BenchUser>))]
[JsonSerializable(typeof(HealthResponse))]
[JsonSerializable(typeof(UploadResponse))]
[JsonSerializable(typeof(TransformResponse))]
[JsonSerializable(typeof(AggregateResponse))]
[JsonSerializable(typeof(SearchResponse))]
[JsonSerializable(typeof(UploadProcessResponse))]
[JsonSerializable(typeof(DelayedResponse))]
[JsonSerializable(typeof(ValidateResponse))]
[JsonSerializable(typeof(ErrorResponse))]
internal partial class BenchJson : JsonSerializerContext
{
}

// IEEE CRC-32 with a hoisted table (computed once, audit P1#11).
internal static class Crc32
{
    private static readonly uint[] Table = BuildTable();

    private static uint[] BuildTable()
    {
        var table = new uint[256];
        for (uint i = 0; i < 256; i++)
        {
            var crc = i;
            for (var j = 0; j < 8; j++)
                crc = (crc & 1) != 0 ? (crc >> 1) ^ 0xEDB88320u : crc >> 1;
            table[i] = crc;
        }
        return table;
    }

    public static uint Hash(ReadOnlySpan<byte> data)
    {
        var crc = 0xFFFFFFFFu;
        foreach (var b in data)
            crc = Table[(crc ^ b) & 0xFF] ^ (crc >> 8);
        return crc ^ 0xFFFFFFFFu;
    }
}
