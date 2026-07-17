// ─────────────────────────────────────────────────────────────────────────────
// .NET Framework 4.8 reference API — Application Benchmark mode.
// Contract: benchmarks/shared/API-SPEC.md (frozen v1, shape family C).
//
// SINGLE SOURCE OF TRUTH: benchmarks/reference-apis/csharp-template/Server.cs
// The csharp-net48/Server.cs copy is byte-identical, written by
// csharp-template/generate-variants.py. Do NOT edit the copy — edit this
// template and regenerate.
//
// net48 is the ONE variant that cannot share the modern template: .NET
// Framework has no Kestrel/minimal APIs, no System.Text.Json in-box, and no
// ZLibStream. Divergences from the modern template (all documented):
//   - HttpListener instead of Kestrel (HTTP/1.1 only, Windows-only TLS via
//     netsh sslcert binding — see deploy.sh).
//   - JavaScriptSerializer (System.Web.Extensions) for JSON parsing; a small
//     escaping-correct writer for output (double formatting must keep the
//     ".0" suffix on integral floats for canonical-JSON validators, §7).
//   - zlib (RFC 1950) built as 0x78 0x9C header + DeflateStream + Adler-32.
//     .NET Framework's deflate is not zlib itself; compressed_size is
//     validated with tolerance per §5.8.
//
// Compile: csc /out:Server.exe /reference:System.IO.Compression.dll
//              /reference:System.Web.Extensions.dll Server.cs
// Run:     Server.exe [port] [cert_dir]
//
// C# 5 compatible — no string interpolation, no null-conditional operators,
// no expression-bodied members (the in-box Framework csc is C# 5).
// ─────────────────────────────────────────────────────────────────────────────

using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.IO.Compression;
using System.Linq;
using System.Net;
using System.Security.Cryptography;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading.Tasks;
using System.Web.Script.Serialization;

namespace BenchmarkServer
{
    // ── Shared dataset (API-SPEC.md §2) — load failure is FATAL ─────────────
    sealed class UserRec
    {
        public long Id;
        public string Name;
        public string Email;
        public double Score;
        public string CreatedAt;
    }

    static class BenchData
    {
        public static string SourcePath;
        public static List<UserRec> Users;
        public static List<string> SearchCorpus;
        public static double[] TimeseriesValues;
        public static Dictionary<string, string> ExpectedChecksums;

        static readonly string[] RequiredChecksumKeys = new string[]
        {
            "users_page1", "aggregate_default", "search_network_top10", "transform_input0"
        };

        public static void LoadOrExit()
        {
            var envPath = Environment.GetEnvironmentVariable("BENCH_DATA_PATH");
            string path = null;
            if (!string.IsNullOrEmpty(envPath))
            {
                // §2: when BENCH_DATA_PATH is set, that exact file must load.
                path = envPath;
            }
            else
            {
                string[] candidates = new string[]
                {
                    @"C:\opt\bench\bench-data.json",
                    "/opt/bench/bench-data.json",
                    Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "..", "shared", "bench-data.json"),
                    Path.Combine("..", "shared", "bench-data.json"),
                };
                foreach (var c in candidates)
                {
                    if (File.Exists(c)) { path = c; break; }
                }
                if (path == null)
                {
                    Fatal("bench-data.json not found (BENCH_DATA_PATH unset; tried: " +
                          string.Join(", ", candidates) + ")");
                }
            }

            Dictionary<string, object> root = null;
            try
            {
                var serializer = new JavaScriptSerializer();
                serializer.MaxJsonLength = int.MaxValue;
                root = serializer.DeserializeObject(File.ReadAllText(path)) as Dictionary<string, object>;
            }
            catch (Exception ex)
            {
                Fatal(string.Format("failed to load {0}: {1}", path, ex.Message));
            }
            if (root == null) Fatal(string.Format("failed to load {0}: not a JSON object", path));

            try
            {
                if (Convert.ToInt64(root["_version"], CultureInfo.InvariantCulture) != 2)
                    Fatal(string.Format("{0}: _version != 2", path));

                var users = (object[])root["users"];
                if (users.Length != 100)
                    Fatal(string.Format("{0}: users count is {1}, expected 100", path, users.Length));
                Users = new List<UserRec>(users.Length);
                foreach (var u in users)
                {
                    var d = (Dictionary<string, object>)u;
                    var rec = new UserRec();
                    rec.Id = Convert.ToInt64(d["id"], CultureInfo.InvariantCulture);
                    rec.Name = (string)d["name"];
                    rec.Email = (string)d["email"];
                    rec.Score = Convert.ToDouble(d["score"], CultureInfo.InvariantCulture);
                    rec.CreatedAt = (string)d["created_at"];
                    Users.Add(rec);
                }

                var corpus = (object[])root["search_corpus"];
                if (corpus.Length != 1000)
                    Fatal(string.Format("{0}: search_corpus count is {1}, expected 1000", path, corpus.Length));
                SearchCorpus = corpus.Select(delegate(object o) { return (string)o; }).ToList();

                var series = (object[])root["timeseries"];
                if (series.Length != 10000)
                    Fatal(string.Format("{0}: timeseries count is {1}, expected 10000", path, series.Length));
                TimeseriesValues = new double[series.Length];
                for (int i = 0; i < series.Length; i++)
                {
                    // §2: timeseries entries are objects — read the `value` field.
                    var d = (Dictionary<string, object>)series[i];
                    TimeseriesValues[i] = Convert.ToDouble(d["value"], CultureInfo.InvariantCulture);
                }

                var inputs = (object[])root["transform_inputs"];
                if (inputs.Length != 10)
                    Fatal(string.Format("{0}: transform_inputs count is {1}, expected 10", path, inputs.Length));

                var sums = (Dictionary<string, object>)root["expected_checksums"];
                ExpectedChecksums = new Dictionary<string, string>();
                foreach (var kv in sums)
                    ExpectedChecksums[kv.Key] = (string)kv.Value;
                foreach (var key in RequiredChecksumKeys)
                {
                    if (!ExpectedChecksums.ContainsKey(key))
                        Fatal(string.Format("{0}: expected_checksums missing key '{1}'", path, key));
                }
            }
            catch (Exception ex)
            {
                Fatal(string.Format("{0}: schema mismatch (API-SPEC.md §2): {1}", path, ex.Message));
            }

            SourcePath = path;
        }

        static void Fatal(string message)
        {
            Console.Error.WriteLine("FATAL: " + message);
            Environment.Exit(1);
        }
    }

    // ── JSON output writer (escaping-correct, Python-float-compatible) ──────
    static class Json
    {
        public static void WriteString(StringBuilder sb, string s)
        {
            sb.Append('"');
            foreach (var ch in s)
            {
                switch (ch)
                {
                    case '"': sb.Append("\\\""); break;
                    case '\\': sb.Append("\\\\"); break;
                    case '\b': sb.Append("\\b"); break;
                    case '\f': sb.Append("\\f"); break;
                    case '\n': sb.Append("\\n"); break;
                    case '\r': sb.Append("\\r"); break;
                    case '\t': sb.Append("\\t"); break;
                    default:
                        if (ch < 0x20)
                            sb.Append("\\u").Append(((int)ch).ToString("x4", CultureInfo.InvariantCulture));
                        else
                            sb.Append(ch);
                        break;
                }
            }
            sb.Append('"');
        }

        // Canonical-JSON validators (§7) re-serialize through Python, which
        // distinguishes int 39 from float 39.0 — integral doubles must keep a
        // ".0" suffix. G17 always round-trips on .NET Framework ("R" does not).
        public static string Double(double v)
        {
            var s = v.ToString("G17", CultureInfo.InvariantCulture);
            if (s.IndexOf('.') < 0 && s.IndexOf('e') < 0 && s.IndexOf('E') < 0 &&
                s.IndexOf('N') < 0 && s.IndexOf('I') < 0)
            {
                s += ".0";
            }
            return s;
        }

        // Re-serialize a JavaScriptSerializer value tree (transform pass-through).
        // Decimals preserve the scale of the input literal ("39.0" stays "39.0").
        public static void WriteValue(StringBuilder sb, object v)
        {
            if (v == null) { sb.Append("null"); return; }
            if (v is bool) { sb.Append(((bool)v) ? "true" : "false"); return; }
            if (v is string) { WriteString(sb, (string)v); return; }
            if (v is int || v is long)
            {
                sb.Append(Convert.ToInt64(v).ToString(CultureInfo.InvariantCulture));
                return;
            }
            if (v is decimal)
            {
                sb.Append(((decimal)v).ToString(CultureInfo.InvariantCulture));
                return;
            }
            if (v is double || v is float)
            {
                sb.Append(Double(Convert.ToDouble(v, CultureInfo.InvariantCulture)));
                return;
            }
            var arr = v as object[];
            if (arr != null)
            {
                sb.Append('[');
                for (int i = 0; i < arr.Length; i++)
                {
                    if (i > 0) sb.Append(',');
                    WriteValue(sb, arr[i]);
                }
                sb.Append(']');
                return;
            }
            var obj = v as Dictionary<string, object>;
            if (obj != null)
            {
                sb.Append('{');
                bool first = true;
                foreach (var kv in obj)
                {
                    if (!first) sb.Append(',');
                    first = false;
                    WriteString(sb, kv.Key);
                    sb.Append(':');
                    WriteValue(sb, kv.Value);
                }
                sb.Append('}');
                return;
            }
            // Unknown scalar — stringify.
            WriteString(sb, v.ToString());
        }
    }

    // IEEE CRC-32 with a hoisted table (computed once, audit P1#11).
    static class Crc32
    {
        static readonly uint[] Table = BuildTable();

        static uint[] BuildTable()
        {
            var table = new uint[256];
            for (uint i = 0; i < 256; i++)
            {
                var crc = i;
                for (int j = 0; j < 8; j++)
                    crc = (crc & 1) != 0 ? (crc >> 1) ^ 0xEDB88320u : crc >> 1;
                table[i] = crc;
            }
            return table;
        }

        public static uint Hash(byte[] data)
        {
            var crc = 0xFFFFFFFFu;
            foreach (var b in data)
                crc = Table[(crc ^ b) & 0xFF] ^ (crc >> 8);
            return crc ^ 0xFFFFFFFFu;
        }
    }

    static class Server
    {
        const long DownloadCap = 2147483648L; // §5.2: 2 GiB
        const int DownloadChunk = 8192;       // §5.2: 8 KiB chunks
        const byte DownloadFill = 0x42;       // §5.2: 'B'

        static byte[] FillChunk;
        static string HealthBody;   // §5.1: precomputed, constant-work
        static string BearerToken;  // §1: BENCH_API_TOKEN ("" = auth disabled)

        static int Main(string[] args)
        {
            BenchData.LoadOrExit();

            int port = 8443;
            var portEnv = Environment.GetEnvironmentVariable("BENCH_PORT");
            if (args.Length > 0) int.TryParse(args[0], out port);
            else if (!string.IsNullOrEmpty(portEnv)) int.TryParse(portEnv, out port);

            var certDir = args.Length > 1
                ? args[1]
                : (Environment.GetEnvironmentVariable("BENCH_CERT_DIR") ?? @"C:\opt\bench");
            // TLS itself is bound with `netsh http add sslcert` (deploy.sh);
            // cert.pem presence is the signal that TLS was provisioned.
            // Application-mode fallback (audit F8): no certs → plain HTTP.
            var hasTls = File.Exists(Path.Combine(certDir, "cert.pem"));
            var scheme = hasTls ? "https" : "http";

            FillChunk = new byte[DownloadChunk];
            for (int i = 0; i < FillChunk.Length; i++) FillChunk[i] = DownloadFill;

            // §5.1: runtime identity = the reference directory name.
            HealthBody = "{\"status\":\"ok\",\"runtime\":\"csharp-net48\",\"version\":\"" +
                         Environment.Version.ToString() + "\"}";
            BearerToken = Environment.GetEnvironmentVariable("BENCH_API_TOKEN") ?? "";

            // §3: BENCH_WORKERS is advisory for C# (ThreadPool in-process
            // scheduling); nproc and the effective value are logged.
            int nproc = Environment.ProcessorCount;
            int workers = nproc;
            var workersEnv = Environment.GetEnvironmentVariable("BENCH_WORKERS");
            int parsedWorkers;
            if (!string.IsNullOrEmpty(workersEnv) && int.TryParse(workersEnv, out parsedWorkers) && parsedWorkers > 0)
                workers = parsedWorkers;

            var listener = new HttpListener();
            listener.Prefixes.Add(string.Format("{0}://+:{1}/", scheme, port));
            try
            {
                listener.Start();
            }
            catch (HttpListenerException ex)
            {
                Console.Error.WriteLine(string.Format(
                    "FATAL: cannot listen on {0}://+:{1}/ — {2} (run: netsh http add urlacl url={0}://+:{1}/ user=Everyone)",
                    scheme, port, ex.Message));
                return 1;
            }

            Console.WriteLine(string.Format(
                "csharp-net48 reference API listening on port {0} (tls={1}, nproc={2}, " +
                "bench_workers={3} [advisory: ThreadPool in-process scheduler], dataset={4})",
                port, hasTls ? "true" : "false", nproc, workers, BenchData.SourcePath));

            while (true)
            {
                HttpListenerContext ctx;
                try { ctx = listener.GetContext(); }
                catch (HttpListenerException) { break; }
                catch (ObjectDisposedException) { break; }
                Task.Run(delegate { return HandleAsync(ctx); });
            }
            return 0;
        }

        static async Task HandleAsync(HttpListenerContext ctx)
        {
            try
            {
                await RouteAsync(ctx);
            }
            catch (Exception ex)
            {
                try
                {
                    WriteJson(ctx, 500, "{\"error\":" + QuoteString(ex.Message) + "}", null);
                }
                catch { /* client gone */ }
            }
            finally
            {
                try { ctx.Response.Close(); } catch { }
            }
        }

        static async Task RouteAsync(HttpListenerContext ctx)
        {
            var path = ctx.Request.Url.AbsolutePath;
            var method = ctx.Request.HttpMethod;
            var sw = Stopwatch.StartNew();

            if (path == "/health" && (method == "GET" || method == "HEAD"))
            {
                WriteRaw(ctx, 200, "application/json", HealthBody);
                return;
            }

            // §1: bearer auth on every route except /health.
            if (BearerToken.Length > 0)
            {
                var auth = ctx.Request.Headers["Authorization"] ?? "";
                if (auth != "Bearer " + BearerToken)
                {
                    // §10.7: /api/* responses carry the benchmark headers.
                    WriteJson(ctx, 401, "{\"error\":\"unauthorized\"}",
                        path.StartsWith("/api/", StringComparison.Ordinal) ? sw : null);
                    return;
                }
            }

            if (path.StartsWith("/download/", StringComparison.Ordinal) && (method == "GET" || method == "HEAD"))
            {
                Download(ctx, path.Substring("/download/".Length), sw);
            }
            else if (path == "/upload" && method == "POST")
            {
                Upload(ctx, sw);
            }
            else if (path == "/api/users" && (method == "GET" || method == "HEAD"))
            {
                ApiUsers(ctx, sw);
            }
            else if (path == "/api/transform" && method == "POST")
            {
                ApiTransform(ctx, sw);
            }
            else if (path == "/api/aggregate" && (method == "GET" || method == "HEAD"))
            {
                ApiAggregate(ctx, sw);
            }
            else if (path == "/api/search" && (method == "GET" || method == "HEAD"))
            {
                ApiSearch(ctx, sw);
            }
            else if (path == "/api/upload/process" && method == "POST")
            {
                ApiUploadProcess(ctx, sw);
            }
            else if (path == "/api/delayed" && (method == "GET" || method == "HEAD"))
            {
                // §5.9: timer-based delay — awaited, so no worker thread blocks.
                await ApiDelayed(ctx, sw);
            }
            else if (path == "/api/validate" && (method == "GET" || method == "HEAD"))
            {
                ApiValidate(ctx, sw);
            }
            else
            {
                WriteJson(ctx, 404, "{\"error\":\"not found\"}", null);
            }
        }

        // §5.2 GET /download/{size} — exactly `size` bytes of 0x42, 8 KiB chunks.
        static void Download(HttpListenerContext ctx, string raw, Stopwatch sw)
        {
            long size;
            // NumberStyles.None rejects signs — negative or non-integer → 400.
            if (!long.TryParse(raw, NumberStyles.None, CultureInfo.InvariantCulture, out size))
            {
                WriteJson(ctx, 400, "{\"error\":\"invalid size\"}", null);
                return;
            }
            if (size > DownloadCap) size = DownloadCap;

            var resp = ctx.Response;
            resp.StatusCode = 200;
            resp.ContentType = "application/octet-stream";
            resp.Headers["X-Download-Bytes"] = size.ToString(CultureInfo.InvariantCulture);
            resp.Headers["Server-Timing"] = "proc;dur=" + Ms1(sw);
            if (ctx.Request.HttpMethod == "HEAD") return;

            resp.ContentLength64 = size;
            var remaining = size;
            while (remaining > 0)
            {
                var n = (int)Math.Min(remaining, DownloadChunk);
                resp.OutputStream.Write(FillChunk, 0, n);
                remaining -= n;
            }
        }

        // §5.3 POST /upload — drain the body without wholesale buffering.
        static void Upload(HttpListenerContext ctx, Stopwatch sw)
        {
            long received = 0;
            var buf = new byte[DownloadChunk];
            int read;
            while ((read = ctx.Request.InputStream.Read(buf, 0, buf.Length)) > 0)
                received += read;

            var resp = ctx.Response;
            resp.Headers["Server-Timing"] = "recv;dur=" + Ms1(sw);
            resp.Headers["X-Networker-Received-Bytes"] = received.ToString(CultureInfo.InvariantCulture);
            var requestId = ctx.Request.Headers["X-Networker-Request-Id"];
            if (requestId != null)
                resp.Headers["X-Networker-Request-Id"] = requestId;

            WriteRaw(ctx, 200, "application/json",
                "{\"received_bytes\":" + received.ToString(CultureInfo.InvariantCulture) + "}");
        }

        // §5.4 GET /api/users?page=N&sort=<field>&order=<asc|desc>
        static void ApiUsers(HttpListenerContext ctx, Stopwatch sw)
        {
            long page = QueryLong(ctx, "page", 1);
            if (page < 1) page = 1;
            var sort = ctx.Request.QueryString["sort"] ?? "id";
            var desc = (ctx.Request.QueryString["order"] ?? "") == "desc";

            // lastPage is compared first so (page-1)*100 can never overflow.
            List<UserRec> window;
            long lastPage = (BenchData.Users.Count + 99) / 100;
            if (page > lastPage)
            {
                window = new List<UserRec>();
            }
            else
            {
                var start = (int)((page - 1) * 100);
                var count = Math.Min(100, BenchData.Users.Count - start);
                window = BenchData.Users.GetRange(start, count);
            }

            // OrderBy is stable — dataset order breaks ties (§5.4); `desc`
            // reverses the ascending result.
            List<UserRec> sorted;
            switch (sort)
            {
                case "name": sorted = window.OrderBy(delegate(UserRec u) { return u.Name; }, StringComparer.Ordinal).ToList(); break;
                case "email": sorted = window.OrderBy(delegate(UserRec u) { return u.Email; }, StringComparer.Ordinal).ToList(); break;
                case "score": sorted = window.OrderBy(delegate(UserRec u) { return u.Score; }).ToList(); break;
                case "created_at": sorted = window.OrderBy(delegate(UserRec u) { return u.CreatedAt; }, StringComparer.Ordinal).ToList(); break;
                default: sorted = window.OrderBy(delegate(UserRec u) { return u.Id; }).ToList(); break;
            }
            if (desc) sorted.Reverse();

            var sb = new StringBuilder(4096);
            sb.Append('[');
            var limit = Math.Min(20, sorted.Count);
            for (int i = 0; i < limit; i++)
            {
                if (i > 0) sb.Append(',');
                var u = sorted[i];
                sb.Append("{\"id\":").Append(u.Id.ToString(CultureInfo.InvariantCulture));
                sb.Append(",\"name\":"); Json.WriteString(sb, u.Name);
                sb.Append(",\"email\":"); Json.WriteString(sb, u.Email);
                sb.Append(",\"score\":").Append(Json.Double(u.Score));
                sb.Append(",\"created_at\":"); Json.WriteString(sb, u.CreatedAt);
                sb.Append('}');
            }
            sb.Append(']');
            WriteJson(ctx, 200, sb.ToString(), sw);
        }

        // §5.5 POST /api/transform — SHA-256 the field strings, reverse values.
        static void ApiTransform(HttpListenerContext ctx, Stopwatch sw)
        {
            string body;
            using (var reader = new StreamReader(ctx.Request.InputStream, Encoding.UTF8))
                body = reader.ReadToEnd();

            Dictionary<string, object> root = null;
            try
            {
                var serializer = new JavaScriptSerializer();
                serializer.MaxJsonLength = int.MaxValue;
                root = serializer.DeserializeObject(body) as Dictionary<string, object>;
            }
            catch (Exception)
            {
                root = null;
            }
            if (root == null)
            {
                WriteJson(ctx, 400, "{\"error\":\"invalid json\"}", sw);
                return;
            }

            long seed = 0; // §5.5 default
            object seedObj;
            if (root.TryGetValue("seed", out seedObj) && seedObj != null && !(seedObj is string) && !(seedObj is bool))
            {
                try { seed = Convert.ToInt64(seedObj, CultureInfo.InvariantCulture); }
                catch (Exception) { seed = 0; }
            }

            var sb = new StringBuilder(1024);
            sb.Append("{\"seed\":").Append(seed.ToString(CultureInfo.InvariantCulture));
            sb.Append(",\"hashed_fields\":[");
            object fieldsObj;
            if (root.TryGetValue("fields", out fieldsObj))
            {
                var fields = fieldsObj as object[];
                if (fields != null)
                {
                    using (var sha = SHA256.Create())
                    {
                        for (int i = 0; i < fields.Length; i++)
                        {
                            if (i > 0) sb.Append(',');
                            var s = fields[i] as string;
                            if (s == null)
                            {
                                var tmp = new StringBuilder();
                                Json.WriteValue(tmp, fields[i]);
                                s = tmp.ToString();
                            }
                            var hash = sha.ComputeHash(Encoding.UTF8.GetBytes(s));
                            sb.Append('"').Append(HexLower(hash)).Append('"');
                        }
                    }
                }
            }
            sb.Append("],\"reversed_values\":[");
            object valuesObj;
            if (root.TryGetValue("values", out valuesObj))
            {
                var values = valuesObj as object[];
                if (values != null)
                {
                    for (int i = values.Length - 1; i >= 0; i--)
                    {
                        if (i < values.Length - 1) sb.Append(',');
                        Json.WriteValue(sb, values[i]); // pass through unmodified
                    }
                }
            }
            sb.Append("]}");
            WriteJson(ctx, 200, sb.ToString(), sw);
        }

        // §5.6 GET /api/aggregate — full-series stats; `range` accepted + ignored.
        static void ApiAggregate(HttpListenerContext ctx, Stopwatch sw)
        {
            var values = (double[])BenchData.TimeseriesValues.Clone();
            Array.Sort(values);
            var n = values.Length;

            double sum = 0.0;
            for (int i = 0; i < n; i++)
                sum += values[i];

            var sb = new StringBuilder(1024);
            sb.Append("{\"total_points\":").Append(n.ToString(CultureInfo.InvariantCulture));
            sb.Append(",\"mean\":").Append(Json.Double(R2(sum / n)));
            sb.Append(",\"p50\":").Append(Json.Double(R2(values[(int)(n * 0.50)])));
            sb.Append(",\"p95\":").Append(Json.Double(R2(values[(int)(n * 0.95)])));
            sb.Append(",\"max\":").Append(Json.Double(R2(values[n - 1])));
            sb.Append(",\"categories\":[");
            var chunk = n / 5;
            for (int i = 0; i < 5; i++)
            {
                if (i > 0) sb.Append(',');
                double chunkSum = 0.0;
                for (int j = i * chunk; j < (i + 1) * chunk; j++)
                    chunkSum += values[j];
                sb.Append("{\"category\":\"q").Append(i + 1).Append('"');
                sb.Append(",\"count\":").Append(chunk.ToString(CultureInfo.InvariantCulture));
                sb.Append(",\"mean\":").Append(Json.Double(R2(chunkSum / chunk)));
                sb.Append(",\"min\":").Append(Json.Double(R2(values[i * chunk])));
                sb.Append(",\"max\":").Append(Json.Double(R2(values[(i + 1) * chunk - 1])));
                sb.Append('}');
            }
            sb.Append("]}");
            WriteJson(ctx, 200, sb.ToString(), sw);
        }

        // §5.7 GET /api/search?q=<term>&limit=N — case-sensitive regex,
        // literal-substring fallback when the pattern does not compile.
        static void ApiSearch(HttpListenerContext ctx, Stopwatch sw)
        {
            var query = ctx.Request.QueryString["q"] ?? "test";
            var limit = QueryLong(ctx, "limit", 20);
            if (limit > 100) limit = 100;
            var take = (int)Math.Max(0, limit);

            Regex re = null;
            try { re = new Regex(query); }
            catch (ArgumentException) { re = null; }

            var positions = new List<int>();
            var items = new List<string>();
            foreach (var item in BenchData.SearchCorpus)
            {
                int pos;
                if (re != null)
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
                positions.Add(pos);
                items.Add(item);
            }

            var order = Enumerable.Range(0, items.Count).ToList();
            order.Sort(delegate(int a, int b)
            {
                var c = positions[a].CompareTo(positions[b]);
                return c != 0 ? c : string.CompareOrdinal(items[a], items[b]);
            });

            var sb = new StringBuilder(1024);
            sb.Append("{\"query\":"); Json.WriteString(sb, query);
            sb.Append(",\"total_matches\":").Append(items.Count.ToString(CultureInfo.InvariantCulture));
            sb.Append(",\"returned\":").Append(Math.Min(take, items.Count).ToString(CultureInfo.InvariantCulture));
            sb.Append(",\"results\":[");
            for (int i = 0; i < order.Count && i < take; i++)
            {
                if (i > 0) sb.Append(',');
                sb.Append("{\"rank\":").Append((i + 1).ToString(CultureInfo.InvariantCulture));
                sb.Append(",\"item\":"); Json.WriteString(sb, items[order[i]]);
                sb.Append(",\"match_position\":").Append(positions[order[i]].ToString(CultureInfo.InvariantCulture));
                sb.Append('}');
            }
            sb.Append("]}");
            WriteJson(ctx, 200, sb.ToString(), sw);
        }

        // §5.8 POST /api/upload/process — CRC-32 + SHA-256 + zlib level ~6.
        static void ApiUploadProcess(HttpListenerContext ctx, Stopwatch sw)
        {
            byte[] body;
            using (var ms = new MemoryStream())
            {
                ctx.Request.InputStream.CopyTo(ms);
                body = ms.ToArray();
            }

            var crc = Crc32.Hash(body);
            string sha;
            using (var sha256 = SHA256.Create())
                sha = HexLower(sha256.ComputeHash(body));
            var compressed = ZlibCompress(body);

            var sb = new StringBuilder(256);
            sb.Append("{\"original_size\":").Append(body.Length.ToString(CultureInfo.InvariantCulture));
            sb.Append(",\"compressed_size\":").Append(compressed.Length.ToString(CultureInfo.InvariantCulture));
            sb.Append(",\"crc32\":\"").Append(crc.ToString("x8", CultureInfo.InvariantCulture)).Append('"');
            sb.Append(",\"sha256\":\"").Append(sha).Append("\"}");
            WriteJson(ctx, 200, sb.ToString(), sw);
        }

        // §5.9 GET /api/delayed?ms=N — async timer delay, clamped to [1, 100];
        // `work` is reserved: accepted and ignored.
        static async Task ApiDelayed(HttpListenerContext ctx, Stopwatch sw)
        {
            var ms = QueryLong(ctx, "ms", 10);
            if (ms < 1) ms = 1;
            if (ms > 100) ms = 100;
            await Task.Delay((int)ms);

            var sb = new StringBuilder(128);
            sb.Append("{\"requested_ms\":").Append(ms.ToString(CultureInfo.InvariantCulture));
            sb.Append(",\"actual_ms\":").Append(Json.Double(R2(sw.Elapsed.TotalMilliseconds)));
            sb.Append('}');
            WriteJson(ctx, 200, sb.ToString(), sw);
        }

        // §5.10 GET /api/validate?seed=N — echo the dataset's expected_checksums.
        static void ApiValidate(HttpListenerContext ctx, Stopwatch sw)
        {
            var seed = QueryLong(ctx, "seed", 42);
            var sb = new StringBuilder(512);
            sb.Append("{\"seed\":").Append(seed.ToString(CultureInfo.InvariantCulture));
            sb.Append(",\"checksums\":{");
            bool first = true;
            foreach (var kv in BenchData.ExpectedChecksums)
            {
                if (!first) sb.Append(',');
                first = false;
                Json.WriteString(sb, kv.Key);
                sb.Append(':');
                Json.WriteString(sb, kv.Value);
            }
            sb.Append("}}");
            WriteJson(ctx, 200, sb.ToString(), sw);
        }

        // ── helpers ──────────────────────────────────────────────────────────

        // §5.6 rounding: half away from zero to 2 decimals (bit-identical to
        // the generator's floor(x*100 + 0.5) / 100).
        static double R2(double x)
        {
            return Math.Floor(x * 100.0 + 0.5) / 100.0;
        }

        static string Ms1(Stopwatch sw)
        {
            return sw.Elapsed.TotalMilliseconds.ToString("0.0", CultureInfo.InvariantCulture);
        }

        static string HexLower(byte[] bytes)
        {
            return BitConverter.ToString(bytes).Replace("-", "").ToLowerInvariant();
        }

        static string QuoteString(string s)
        {
            var sb = new StringBuilder(s.Length + 2);
            Json.WriteString(sb, s);
            return sb.ToString();
        }

        static long QueryLong(HttpListenerContext ctx, string key, long fallback)
        {
            var v = ctx.Request.QueryString[key];
            long parsed;
            if (v != null && long.TryParse(v, NumberStyles.Integer, CultureInfo.InvariantCulture, out parsed))
                return parsed;
            return fallback;
        }

        // zlib (RFC 1950): 0x78 0x9C header + raw deflate + big-endian Adler-32.
        static byte[] ZlibCompress(byte[] data)
        {
            using (var ms = new MemoryStream())
            {
                ms.WriteByte(0x78);
                ms.WriteByte(0x9C);
                using (var deflate = new DeflateStream(ms, CompressionLevel.Optimal, true))
                    deflate.Write(data, 0, data.Length);
                var adler = Adler32(data);
                ms.WriteByte((byte)((adler >> 24) & 0xFF));
                ms.WriteByte((byte)((adler >> 16) & 0xFF));
                ms.WriteByte((byte)((adler >> 8) & 0xFF));
                ms.WriteByte((byte)(adler & 0xFF));
                return ms.ToArray();
            }
        }

        static uint Adler32(byte[] data)
        {
            const uint Mod = 65521;
            uint a = 1, b = 0;
            foreach (var d in data)
            {
                a = (a + d) % Mod;
                b = (b + a) % Mod;
            }
            return (b << 16) | a;
        }

        /// <summary>Write a JSON body; a non-null Stopwatch adds the §1
        /// benchmark headers (required on every /api/* response).</summary>
        static void WriteJson(HttpListenerContext ctx, int status, string json, Stopwatch sw)
        {
            var resp = ctx.Response;
            if (sw != null)
            {
                resp.Headers["Server-Timing"] = "app;dur=" + Ms1(sw);
                resp.Headers["Cache-Control"] = "no-store, no-cache, must-revalidate";
                resp.Headers["Timing-Allow-Origin"] = "*";
                resp.Headers["Access-Control-Allow-Origin"] = "*";
            }
            WriteRaw(ctx, status, "application/json", json);
        }

        static void WriteRaw(HttpListenerContext ctx, int status, string contentType, string body)
        {
            var resp = ctx.Response;
            resp.StatusCode = status;
            resp.ContentType = contentType;
            var bytes = Encoding.UTF8.GetBytes(body);
            if (ctx.Request.HttpMethod == "HEAD")
                return; // headers only; HttpListener sends no body
            resp.ContentLength64 = bytes.Length;
            resp.OutputStream.Write(bytes, 0, bytes.Length);
        }
    }
}
