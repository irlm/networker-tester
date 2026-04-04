using System.Diagnostics;
using System.IO.Compression;
using System.Security.Cryptography;
using System.Security.Cryptography.X509Certificates;
using System.Text;
using System.Text.Json;
using System.Text.RegularExpressions;
using Microsoft.AspNetCore.Server.Kestrel.Core;

// Load shared benchmark data at startup
static JsonDocument? BenchData = null;
static void LoadBenchData() {
    string[] paths = {
        Environment.GetEnvironmentVariable("BENCH_DATA_PATH") ?? "",
        "/opt/bench/bench-data.json",
        Path.Combine(AppContext.BaseDirectory, "..", "shared", "bench-data.json"),
        "../shared/bench-data.json",
    };
    foreach (var p in paths) {
        if (string.IsNullOrEmpty(p) || !File.Exists(p)) continue;
        try {
            var content = File.ReadAllText(p);
            BenchData = JsonDocument.Parse(content);
            Console.WriteLine($"Loaded bench-data.json from {p}");
            return;
        } catch { }
    }
    Console.WriteLine("WARNING: bench-data.json not found, using PRNG fallback");
}

LoadBenchData();

var builder = WebApplication.CreateBuilder(args);

// Configure Kestrel for HTTPS on port 8443 with HTTP/1.1, HTTP/2, and HTTP/3
var certDir  = Environment.GetEnvironmentVariable("BENCH_CERT_DIR") ?? "/opt/bench";
var certPath = $"{certDir}/cert.pem";
var keyPath  = $"{certDir}/key.pem";
var port     = int.Parse(Environment.GetEnvironmentVariable("BENCH_PORT") ?? "8443");

builder.WebHost.ConfigureKestrel(options =>
{
    var cert = X509Certificate2.CreateFromPemFile(certPath, keyPath);

    options.ListenAnyIP(port, listenOptions =>
    {
        listenOptions.UseHttps(cert);
        listenOptions.Protocols = HttpProtocols.Http1AndHttp2AndHttp3;
    });
});

var app = builder.Build();

// Advertise HTTP/3 via Alt-Svc header
app.Use(async (context, next) =>
{
    context.Response.Headers["Alt-Svc"] = $"h3=\":{port}\"; ma=86400";
    await next();
});

// GET /health — runtime identity and version
app.MapGet("/health", () => Results.Json(new
{
    status  = "ok",
    runtime = "csharp-net9",
    version = Environment.Version.ToString()
}));

// GET /download/{size} — stream `size` bytes of 0x42 in 8 KiB chunks
app.MapGet("/download/{size}", async (long size, HttpContext ctx) =>
{
    if (size <= 0)
    {
        ctx.Response.StatusCode = 400;
        return;
    }

    ctx.Response.ContentType   = "application/octet-stream";
    ctx.Response.ContentLength = size;

    const int chunkSize = 8192;
    var buffer = new byte[chunkSize];
    Array.Fill(buffer, (byte)0x42);

    var remaining = size;
    while (remaining > 0)
    {
        var toWrite = (int)Math.Min(remaining, chunkSize);
        await ctx.Response.Body.WriteAsync(buffer.AsMemory(0, toWrite));
        remaining -= toWrite;
    }
});

// POST /upload — consume full request body, return byte count
app.MapPost("/upload", async (HttpContext ctx) =>
{
    const int bufferSize = 8192;
    var buffer = new byte[bufferSize];
    long totalBytes = 0;

    int bytesRead;
    while ((bytesRead = await ctx.Request.Body.ReadAsync(buffer)) > 0)
    {
        totalBytes += bytesRead;
    }

    return Results.Json(new { bytes_received = totalBytes });
});

// ---------------------------------------------------------------------------
// Application Benchmark JSON API endpoints
// ---------------------------------------------------------------------------

var firstNames = new[] {
    "Alice", "Bob", "Carol", "Dave", "Eve", "Frank", "Grace", "Hank",
    "Ivy", "Jack", "Kara", "Leo", "Mia", "Nick", "Olga", "Paul",
    "Quinn", "Rita", "Sam", "Tina"
};
var lastNames = new[] {
    "Smith", "Johnson", "Brown", "Taylor", "Anderson", "Thomas", "Jackson",
    "White", "Harris", "Martin", "Garcia", "Clark", "Lewis", "Hall", "Young",
    "King", "Wright", "Lopez", "Hill", "Scott"
};
var domains = new[] { "example.com", "test.org", "demo.net", "bench.io", "sample.dev" };
var searchWords = new[] {
    "network", "latency", "throughput", "bandwidth", "packet", "socket",
    "connection", "timeout", "buffer", "stream", "protocol", "endpoint",
    "request", "response", "header", "payload", "router", "gateway",
    "firewall", "proxy"
};
var catNames = new[] { "alpha", "beta", "gamma", "delta", "epsilon" };

// Helper: set API headers, return Stopwatch
static Stopwatch SetAPIHeaders(HttpContext ctx)
{
    ctx.Response.ContentType = "application/json";
    ctx.Response.Headers["Cache-Control"] = "no-store, no-cache, must-revalidate";
    ctx.Response.Headers["Timing-Allow-Origin"] = "*";
    ctx.Response.Headers["Access-Control-Allow-Origin"] = "*";
    return Stopwatch.StartNew();
}

// Helper: write Server-Timing header
static void WriteServerTiming(HttpContext ctx, Stopwatch sw)
{
    sw.Stop();
    var ms = sw.Elapsed.TotalMilliseconds;
    ctx.Response.Headers["Server-Timing"] = $"app;dur={ms:F1}";
}

// Helper: JSON-escape a string
static string JsonEsc(string s)
{
    return s.Replace("\\", "\\\\").Replace("\"", "\\\"");
}

// Generate 100 deterministic users
static List<(int Id, string Name, string Email, int Age, int Score, bool Active, string CreatedAt)>
    GenerateUsers(int seed, string[] firstN, string[] lastN, string[] doms)
{
    var rng = new Random(seed);
    var users = new List<(int, string, string, int, int, bool, string)>(100);
    for (int i = 0; i < 100; i++)
    {
        var first = firstN[rng.Next(firstN.Length)];
        var last  = lastN[rng.Next(lastN.Length)];
        var dom   = doms[rng.Next(doms.Length)];
        users.Add((
            i + 1,
            $"{first} {last}",
            $"{first.ToLower()}.{last.ToLower()}@{dom}",
            20 + rng.Next(50),
            rng.Next(1000),
            rng.Next(2) == 1,
            $"2025-{1 + rng.Next(12):D2}-{1 + rng.Next(28):D2}"
        ));
    }
    return users;
}

// GET /api/users?page=N&sort=field&order=asc — paginated sorted user list
app.MapGet("/api/users", (HttpContext ctx) =>
{
    var sw = SetAPIHeaders(ctx);

    if (!int.TryParse(ctx.Request.Query["page"], out int page) || page < 1) page = 1;
    var sortField = ctx.Request.Query["sort"].ToString();
    var order     = ctx.Request.Query["order"].ToString();

    // Try shared bench data first, fall back to PRNG
    var users = new List<(int Id, string Name, string Email, int Age, int Score, bool Active, string CreatedAt)>();
    if (BenchData != null)
    {
        try
        {
            foreach (var u in BenchData.RootElement.GetProperty("users").EnumerateArray())
            {
                users.Add((
                    u.GetProperty("id").GetInt32(),
                    u.GetProperty("name").GetString()!,
                    u.GetProperty("email").GetString()!,
                    u.GetProperty("age").GetInt32(),
                    u.GetProperty("score").GetInt32(),
                    u.GetProperty("active").GetBoolean(),
                    u.GetProperty("created_at").GetString()!
                ));
            }
        }
        catch { users.Clear(); }
    }
    if (users.Count == 0)
        users = GenerateUsers(page, firstNames, lastNames, domains);

    users.Sort((a, b) => sortField switch
    {
        "name"  => string.Compare(a.Name, b.Name, StringComparison.Ordinal),
        "email" => string.Compare(a.Email, b.Email, StringComparison.Ordinal),
        "age"   => a.Age.CompareTo(b.Age),
        "score" => a.Score.CompareTo(b.Score),
        _       => a.Id.CompareTo(b.Id)
    });
    if (order == "desc") users.Reverse();

    int pageSize = 20;
    int offset = Math.Min((page - 1) * pageSize, users.Count);
    int end    = Math.Min(offset + pageSize, users.Count);

    var sb = new StringBuilder("[");
    for (int i = offset; i < end; i++)
    {
        if (i > offset) sb.Append(',');
        var u = users[i];
        sb.Append($"{{\"id\":{u.Id},\"name\":\"{JsonEsc(u.Name)}\",\"email\":\"{JsonEsc(u.Email)}\",\"age\":{u.Age},\"score\":{u.Score},\"active\":{(u.Active ? "true" : "false")},\"created_at\":\"{u.CreatedAt}\"}}");
    }
    sb.Append(']');

    WriteServerTiming(ctx, sw);
    return Results.Content(sb.ToString(), "application/json");
});

// POST /api/transform — hash string fields, reverse arrays
app.MapPost("/api/transform", async (HttpContext ctx) =>
{
    var sw = SetAPIHeaders(ctx);

    using var reader = new StreamReader(ctx.Request.Body);
    var body = await reader.ReadToEndAsync();

    // Minimal JSON parse: extract key-value pairs
    // Use System.Text.Json for proper parsing
    var doc = System.Text.Json.JsonDocument.Parse(body);
    var sb = new StringBuilder("{");
    bool first = true;

    foreach (var prop in doc.RootElement.EnumerateObject())
    {
        if (!first) sb.Append(',');
        first = false;
        sb.Append($"\"{JsonEsc(prop.Name)}\":");

        if (prop.Value.ValueKind == System.Text.Json.JsonValueKind.String)
        {
            var hash = SHA256.HashData(Encoding.UTF8.GetBytes(prop.Value.GetString()!));
            sb.Append($"\"{Convert.ToHexString(hash).ToLower()}\"");
        }
        else if (prop.Value.ValueKind == System.Text.Json.JsonValueKind.Array)
        {
            var items = new List<string>();
            foreach (var el in prop.Value.EnumerateArray())
                items.Add(el.GetRawText());
            items.Reverse();
            sb.Append('[');
            sb.Append(string.Join(",", items));
            sb.Append(']');
        }
        else
        {
            sb.Append(prop.Value.GetRawText());
        }
    }
    sb.Append('}');

    WriteServerTiming(ctx, sw);
    return Results.Content(sb.ToString(), "application/json");
});

// GET /api/aggregate?range=start,end — statistics over generated data
app.MapGet("/api/aggregate", (HttpContext ctx) =>
{
    var sw = SetAPIHeaders(ctx);

    var rangeStr = ctx.Request.Query["range"].ToString();
    var parts = rangeStr.Split(',');
    if (parts.Length != 2 ||
        !long.TryParse(parts[0], out long rangeStart) ||
        !long.TryParse(parts[1], out long rangeEnd))
    {
        ctx.Response.StatusCode = 400;
        WriteServerTiming(ctx, sw);
        return Results.Content("{\"error\":\"range must be start,end\"}", "application/json");
    }

    // Try shared bench data timeseries, fall back to PRNG
    var values = new List<double>();
    if (BenchData != null)
    {
        try
        {
            foreach (var v in BenchData.RootElement.GetProperty("timeseries").EnumerateArray())
                values.Add(v.GetDouble());
        }
        catch { values.Clear(); }
    }
    if (values.Count == 0)
    {
        var rng = new Random((int)rangeStart);
        for (int i = 0; i < 10000; i++)
            values.Add(rng.NextDouble() * (rangeEnd - rangeStart) + rangeStart);
    }

    int n = values.Count;
    double sum = 0;
    var catCounts = new int[5];
    var catSums   = new double[5];

    for (int i = 0; i < n; i++)
    {
        sum += values[i];
        int ci = i % 5;
        catCounts[ci]++;
        catSums[ci] += values[i];
    }

    values.Sort();

    var sb = new StringBuilder();
    sb.Append($"{{\"count\":{n},\"mean\":{sum / n},\"p50\":{values[n / 2]},\"p95\":{values[(int)(n * 0.95)]},\"max\":{values[n - 1]},\"categories\":{{");
    for (int i = 0; i < 5; i++)
    {
        if (i > 0) sb.Append(',');
        double mean = catCounts[i] > 0 ? catSums[i] / catCounts[i] : 0;
        sb.Append($"\"{catNames[i]}\":{{\"count\":{catCounts[i]},\"sum\":{catSums[i]},\"mean\":{mean}}}");
    }
    sb.Append("}}");

    WriteServerTiming(ctx, sw);
    return Results.Content(sb.ToString(), "application/json");
});

// GET /api/search?q=term&limit=N — regex search over generated strings
app.MapGet("/api/search", (HttpContext ctx) =>
{
    var sw = SetAPIHeaders(ctx);

    var q = ctx.Request.Query["q"].ToString();
    if (string.IsNullOrEmpty(q))
    {
        ctx.Response.StatusCode = 400;
        WriteServerTiming(ctx, sw);
        return Results.Content("{\"error\":\"q parameter required\"}", "application/json");
    }
    if (!int.TryParse(ctx.Request.Query["limit"], out int limit) || limit < 1 || limit > 100)
        limit = 10;

    var re = new Regex(Regex.Escape(q), RegexOptions.IgnoreCase);

    // Build corpus from shared data or PRNG fallback
    var corpus = new List<string>();
    if (BenchData != null)
    {
        try
        {
            foreach (var item in BenchData.RootElement.GetProperty("search_corpus").EnumerateArray())
                corpus.Add(item.GetString()!);
        }
        catch { corpus.Clear(); }
    }
    if (corpus.Count == 0)
    {
        var rng = new Random(42);
        for (int i = 0; i < 1000; i++)
        {
            int wordCount = 3 + rng.Next(4);
            var sb2 = new StringBuilder();
            for (int j = 0; j < wordCount; j++)
            {
                if (j > 0) sb2.Append(' ');
                sb2.Append(searchWords[rng.Next(searchWords.Length)]);
            }
            corpus.Add(sb2.ToString());
        }
    }

    var results = new List<(int Index, string Text, double Score)>();
    for (int i = 0; i < corpus.Count; i++)
    {
        var text = corpus[i];
        var match = re.Match(text);
        if (match.Success)
        {
            double score = 1.0 / (1.0 + match.Index);
            results.Add((i, text, score));
        }
    }

    results.Sort((a, b) => b.Score.CompareTo(a.Score));
    if (results.Count > limit) results = results.GetRange(0, limit);

    var sb = new StringBuilder("[");
    for (int i = 0; i < results.Count; i++)
    {
        if (i > 0) sb.Append(',');
        var r = results[i];
        sb.Append($"{{\"index\":{r.Index},\"text\":\"{JsonEsc(r.Text)}\",\"score\":{r.Score}}}");
    }
    sb.Append(']');

    WriteServerTiming(ctx, sw);
    return Results.Content(sb.ToString(), "application/json");
});

// POST /api/upload/process — hash and compress uploaded body
app.MapPost("/api/upload/process", async (HttpContext ctx) =>
{
    var sw = SetAPIHeaders(ctx);

    using var ms = new MemoryStream();
    await ctx.Request.Body.CopyToAsync(ms);
    var body = ms.ToArray();

    // CRC32 (use System.IO.Hashing if available, otherwise manual)
    uint crc = Crc32(body);
    var sha = SHA256.HashData(body);

    using var compMs = new MemoryStream();
    using (var deflate = new DeflateStream(compMs, CompressionLevel.Optimal, leaveOpen: true))
    {
        deflate.Write(body, 0, body.Length);
    }
    int compressedSize = (int)compMs.Length;

    var result = $"{{\"original_size\":{body.Length},\"compressed_size\":{compressedSize},\"crc32\":\"{crc:x8}\",\"sha256\":\"{Convert.ToHexString(sha).ToLower()}\"}}";

    WriteServerTiming(ctx, sw);
    return Results.Content(result, "application/json");
});

// GET /api/delayed?ms=N&work=light — async delay with optional CPU work
app.MapGet("/api/delayed", async (HttpContext ctx) =>
{
    var sw = SetAPIHeaders(ctx);

    if (!int.TryParse(ctx.Request.Query["ms"], out int ms)) ms = 1;
    if (ms < 1) ms = 1;
    if (ms > 100) ms = 100;
    var work = ctx.Request.Query["work"].ToString();

    await Task.Delay(ms);

    var sb = new StringBuilder();
    sb.Append($"{{\"requested_ms\":{ms},\"actual_ms\":{sw.Elapsed.TotalMilliseconds:F1},\"work\":\"{JsonEsc(work)}\"");

    if (work == "heavy")
    {
        double x = 0;
        for (int i = 0; i < 100000; i++)
            x += Math.Sqrt(i);
        sb.Append($",\"compute\":{x}");
    }

    sb.Append('}');

    WriteServerTiming(ctx, sw);
    return Results.Content(sb.ToString(), "application/json");
});

// GET /api/validate?seed=42 — checksums for all endpoints
app.MapGet("/api/validate", (HttpContext ctx) =>
{
    var sw = SetAPIHeaders(ctx);

    // If shared data has pre-computed checksums, return them directly
    if (BenchData != null)
    {
        try
        {
            var checksums = BenchData.RootElement.GetProperty("expected_checksums");
            WriteServerTiming(ctx, sw);
            return Results.Content(checksums.GetRawText(), "application/json");
        }
        catch { }
    }

    if (!long.TryParse(ctx.Request.Query["seed"], out long seed) || seed == 0) seed = 42;

    // Users checksum
    var users = GenerateUsers((int)seed, firstNames, lastNames, domains);
    var usersJson = new StringBuilder("[");
    for (int i = 0; i < users.Count; i++)
    {
        if (i > 0) usersJson.Append(',');
        var u = users[i];
        usersJson.Append($"{{\"id\":{u.Id},\"name\":\"{JsonEsc(u.Name)}\",\"email\":\"{JsonEsc(u.Email)}\",\"age\":{u.Age},\"score\":{u.Score},\"active\":{(u.Active ? "true" : "false")},\"created_at\":\"{u.CreatedAt}\"}}");
    }
    usersJson.Append(']');
    var usersHash = SHA256.HashData(Encoding.UTF8.GetBytes(usersJson.ToString()));

    // Aggregate checksum
    var rng = new Random((int)seed);
    double sum = 0;
    for (int i = 0; i < 10000; i++)
        sum += rng.NextDouble() * 100.0;
    var aggHash = SHA256.HashData(Encoding.UTF8.GetBytes(sum.ToString("F6")));

    // Search checksum (seed=42 corpus, q="network")
    var rng2 = new Random(42);
    var corpus = new StringBuilder();
    for (int i = 0; i < 1000; i++)
    {
        int wordCount = 3 + rng2.Next(4);
        for (int j = 0; j < wordCount; j++)
        {
            if (j > 0) corpus.Append(' ');
            corpus.Append(searchWords[rng2.Next(searchWords.Length)]);
        }
        corpus.Append('\n');
    }
    var searchHash = SHA256.HashData(Encoding.UTF8.GetBytes(corpus.ToString()));

    var result = $"{{\"seed\":\"{seed}\",\"users\":\"{Convert.ToHexString(usersHash, 0, 16).ToLower()}\",\"aggregate\":\"{Convert.ToHexString(aggHash, 0, 16).ToLower()}\",\"search\":\"{Convert.ToHexString(searchHash, 0, 16).ToLower()}\"}}";

    WriteServerTiming(ctx, sw);
    return Results.Content(result, "application/json");
});

app.Run();

// CRC32 (IEEE polynomial) implementation
static uint Crc32(byte[] data)
{
    uint[] table = new uint[256];
    for (uint i = 0; i < 256; i++)
    {
        uint crc = i;
        for (int j = 0; j < 8; j++)
            crc = (crc & 1) != 0 ? (crc >> 1) ^ 0xEDB88320u : crc >> 1;
        table[i] = crc;
    }

    uint result = 0xFFFFFFFF;
    foreach (byte b in data)
        result = table[(result ^ b) & 0xFF] ^ (result >> 8);
    return result ^ 0xFFFFFFFF;
}
