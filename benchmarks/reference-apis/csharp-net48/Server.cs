// .NET Framework 4.8 Reference API for Application Benchmark Mode
// Compile: csc /out:Server.exe /reference:System.IO.Compression.dll Server.cs
// Run:     Server.exe [port] [cert_dir]
//
// Uses HttpListener (Windows-only, no Kestrel).
// Requires: netsh http add urlacl url=https://+:8443/ user=Everyone
//           netsh http add sslcert ipport=0.0.0.0:8443 certhash=<thumbprint> appid={...}
//
// C# 5 compatible — no string interpolation, no expression-bodied members,
// no pattern matching, no exception filters.

using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.IO.Compression;
using System.Linq;
using System.Net;
using System.Security.Cryptography;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading;
using System.Threading.Tasks;

namespace BenchmarkServer
{
    // Simple leveled logger
    static class Log
    {
        enum Level { Error = 0, Warn = 1, Info = 2, Debug = 3 }
        static Level _level = Level.Info;

        public static void Init()
        {
            var env = Environment.GetEnvironmentVariable("LOG_LEVEL") ?? "info";
            switch (env.ToLower())
            {
                case "error": _level = Level.Error; break;
                case "warn":  _level = Level.Warn;  break;
                case "debug": _level = Level.Debug; break;
                default:      _level = Level.Info;  break;
            }
        }

        static void Write(Level lvl, string msg)
        {
            if (lvl > _level) return;
            Console.Error.WriteLine(string.Format("[{0}] [{1}] {2}",
                DateTime.UtcNow.ToString("O"), lvl.ToString().ToUpper(), msg));
        }

        public static void Error(string msg)
        {
            Write(Level.Error, msg);
        }

        public static void Warn(string msg)
        {
            Write(Level.Warn, msg);
        }

        public static void Info(string msg)
        {
            Write(Level.Info, msg);
        }

        public static void Debug(string msg)
        {
            Write(Level.Debug, msg);
        }
    }

    // Shared benchmark data
    static class BenchDataStore
    {
        // Raw JSON arrays/objects loaded from bench-data.json
        public static string RawJson;
        public static List<Dictionary<string, object>> Users;
        public static List<string> SearchCorpus;
        public static List<Dictionary<string, object>> Timeseries;
        public static Dictionary<string, string> ExpectedChecksums;
        public static bool Loaded;

        public static void Load()
        {
            string[] paths = {
                Environment.GetEnvironmentVariable("BENCH_DATA_PATH") ?? "",
                @"C:\opt\bench\bench-data.json",
                "/opt/bench/bench-data.json",
                Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "..", "shared", "bench-data.json"),
                @"..\shared\bench-data.json",
            };

            foreach (var p in paths)
            {
                if (string.IsNullOrEmpty(p) || !File.Exists(p)) continue;
                try
                {
                    RawJson = File.ReadAllText(p);
                    ParseBenchData(RawJson);
                    Log.Info(string.Format("Loaded bench-data.json from {0}", p));
                    Loaded = true;
                    return;
                }
                catch (Exception ex)
                {
                    Log.Warn(string.Format("Failed to parse {0}: {1}", p, ex.Message));
                }
            }
            Log.Warn("bench-data.json not found, using PRNG fallback");
            Loaded = false;
        }

        // Minimal JSON parsing without System.Text.Json (not available in .NET 4.8)
        // We use a simple approach: deserialize with JavaScriptSerializer or manual parsing
        static void ParseBenchData(string json)
        {
            // Use System.Web.Script.Serialization if available, otherwise manual
            // For .NET 4.8 without System.Web reference, use manual parsing
            var serializer = new System.Web.Script.Serialization.JavaScriptSerializer();
            serializer.MaxJsonLength = int.MaxValue;
            var data = serializer.Deserialize<Dictionary<string, object>>(json);

            // Users
            Users = new List<Dictionary<string, object>>();
            if (data.ContainsKey("users"))
            {
                var usersObj = data["users"] as object[];
                if (usersObj != null)
                {
                    foreach (var item in usersObj)
                    {
                        var u = item as Dictionary<string, object>;
                        if (u != null)
                            Users.Add(u);
                    }
                }
            }

            // Search corpus
            SearchCorpus = new List<string>();
            if (data.ContainsKey("search_corpus"))
            {
                var corpusObj = data["search_corpus"] as object[];
                if (corpusObj != null)
                {
                    foreach (var s in corpusObj)
                        SearchCorpus.Add(s.ToString());
                }
            }

            // Timeseries
            Timeseries = new List<Dictionary<string, object>>();
            if (data.ContainsKey("timeseries"))
            {
                var tsObj = data["timeseries"] as object[];
                if (tsObj != null)
                {
                    foreach (var item in tsObj)
                    {
                        var t = item as Dictionary<string, object>;
                        if (t != null)
                            Timeseries.Add(t);
                    }
                }
            }

            // Expected checksums
            ExpectedChecksums = new Dictionary<string, string>();
            if (data.ContainsKey("expected_checksums"))
            {
                var checksums = data["expected_checksums"] as Dictionary<string, object>;
                if (checksums != null)
                {
                    foreach (var kv in checksums)
                        ExpectedChecksums[kv.Key] = kv.Value.ToString();
                }
            }
        }
    }

    class Program
    {
        static readonly string Runtime = "csharp-net48";
        static readonly Random FallbackRng = new Random(42);

        static void Main(string[] args)
        {
            Log.Init();
            BenchDataStore.Load();

            var port = args.Length > 0 ? int.Parse(args[0]) : 8443;
            var prefix = string.Format("https://+:{0}/", port);

            var listener = new HttpListener();
            listener.Prefixes.Add(prefix);

            try
            {
                listener.Start();
            }
            catch (HttpListenerException ex)
            {
                Log.Error(string.Format("Failed to start listener on {0}: {1}", prefix, ex.Message));
                Log.Error("Run as admin or: netsh http add urlacl url=" + prefix + " user=Everyone");
                return;
            }

            Log.Info(string.Format("{0} reference API listening on {1}", Runtime, prefix));

            // Accept connections in a loop
            var cts = new CancellationTokenSource();
            Console.CancelKeyPress += (s, e) => { e.Cancel = true; cts.Cancel(); };

            Task.Run(async () =>
            {
                while (!cts.IsCancellationRequested)
                {
                    try
                    {
                        var ctx = await listener.GetContextAsync();
                        var ignored = Task.Run(() => HandleRequest(ctx));
                    }
                    catch (Exception ex)
                    {
                        if (!cts.IsCancellationRequested)
                        {
                            Log.Error(string.Format("Accept error: {0}", ex.Message));
                        }
                    }
                }
            }).Wait();

            listener.Stop();
        }

        static void HandleRequest(HttpListenerContext ctx)
        {
            var sw = Stopwatch.StartNew();
            var req = ctx.Request;
            var resp = ctx.Response;

            try
            {
                var path = req.Url.AbsolutePath.TrimEnd('/');
                var method = req.HttpMethod;

                // Route dispatch
                string body;
                switch (path)
                {
                    case "/health":
                        body = HandleHealth();
                        break;
                    case "/download":
                        HandleDownload(req, resp);
                        return;
                    case "/upload":
                        body = HandleUpload(req);
                        break;
                    case "/api/users":
                        body = HandleApiUsers(req);
                        break;
                    case "/api/transform":
                        body = HandleApiTransform(req);
                        break;
                    case "/api/aggregate":
                        body = HandleApiAggregate(req);
                        break;
                    case "/api/search":
                        body = HandleApiSearch(req);
                        break;
                    case "/api/upload/process":
                        body = HandleApiUploadProcess(req);
                        break;
                    case "/api/delayed":
                        body = HandleApiDelayed(req);
                        break;
                    case "/api/validate":
                        body = HandleApiValidate(req);
                        break;
                    default:
                        resp.StatusCode = 404;
                        body = "{\"error\":\"not found\"}";
                        break;
                }

                sw.Stop();
                SetBenchHeaders(resp, sw.Elapsed.TotalMilliseconds);
                var bytes = Encoding.UTF8.GetBytes(body);
                resp.ContentType = "application/json";
                resp.ContentLength64 = bytes.Length;
                resp.OutputStream.Write(bytes, 0, bytes.Length);
            }
            catch (Exception ex)
            {
                Log.Error(string.Format("Request error {0}: {1}", req.Url, ex.Message));
                resp.StatusCode = 500;
            }
            finally
            {
                resp.Close();
            }
        }

        static void SetBenchHeaders(HttpListenerResponse resp, double durationMs)
        {
            resp.AddHeader("Server-Timing", string.Format("app;dur={0:F1}", durationMs));
            resp.AddHeader("Cache-Control", "no-store, no-cache, must-revalidate");
            resp.AddHeader("Timing-Allow-Origin", "*");
            resp.AddHeader("Access-Control-Allow-Origin", "*");
            resp.AddHeader("Alt-Svc", "h3=\":8443\"; ma=86400");
        }

        // -- /health ------------------------------------------------------
        static string HandleHealth()
        {
            return string.Format("{{\"status\":\"ok\",\"runtime\":\"{0}\",\"version\":\"{1}\"}}",
                Runtime, Environment.Version);
        }

        // -- /download ----------------------------------------------------
        static void HandleDownload(HttpListenerRequest req, HttpListenerResponse resp)
        {
            var sizeStr = req.QueryString["bytes"] ?? "1024";
            long size;
            if (!long.TryParse(sizeStr, out size)) size = 1024;
            size = Math.Min(size, 2L * 1024 * 1024 * 1024);

            resp.ContentType = "application/octet-stream";
            resp.ContentLength64 = size;
            var chunk = new byte[8192];
            for (int i = 0; i < chunk.Length; i++) chunk[i] = 0x42;

            long remaining = size;
            while (remaining > 0)
            {
                int toWrite = (int)Math.Min(remaining, chunk.Length);
                resp.OutputStream.Write(chunk, 0, toWrite);
                remaining -= toWrite;
            }
            resp.Close();
        }

        // -- /upload ------------------------------------------------------
        static string HandleUpload(HttpListenerRequest req)
        {
            long total = 0;
            var buf = new byte[8192];
            int read;
            while ((read = req.InputStream.Read(buf, 0, buf.Length)) > 0)
                total += read;
            return string.Format("{{\"bytes_received\":{0}}}", total);
        }

        // -- /api/users ---------------------------------------------------
        static string HandleApiUsers(HttpListenerRequest req)
        {
            var pageStr = req.QueryString["page"] ?? "1";
            var sort = req.QueryString["sort"] ?? "name";
            var order = req.QueryString["order"] ?? "asc";
            int p;
            int page = int.TryParse(pageStr, out p) ? p : 1;

            List<Dictionary<string, object>> users;
            if (BenchDataStore.Loaded && BenchDataStore.Users != null)
            {
                users = new List<Dictionary<string, object>>(BenchDataStore.Users);
            }
            else
            {
                users = GenerateUsersFallback(page);
            }

            // Sort
            switch (sort)
            {
                case "name":
                    users.Sort((a, b) => string.Compare(a["name"].ToString(), b["name"].ToString(), StringComparison.Ordinal));
                    break;
                case "email":
                    users.Sort((a, b) => string.Compare(a["email"].ToString(), b["email"].ToString(), StringComparison.Ordinal));
                    break;
                case "score":
                    users.Sort((a, b) => Convert.ToDouble(a["score"]).CompareTo(Convert.ToDouble(b["score"])));
                    break;
                case "id":
                    users.Sort((a, b) => Convert.ToInt32(a["id"]).CompareTo(Convert.ToInt32(b["id"])));
                    break;
            }
            if (order == "desc") users.Reverse();

            // Paginate (20 per page)
            int skip = (page - 1) * 20;
            var paged = users.Skip(skip).Take(20).ToList();

            var sb = new StringBuilder("[");
            for (int i = 0; i < paged.Count; i++)
            {
                if (i > 0) sb.Append(",");
                var u = paged[i];
                sb.Append(string.Format("{{\"id\":{0},\"name\":\"{1}\",\"email\":\"{2}\",\"score\":{3:F2},\"created_at\":\"{4}\"}}",
                    u["id"],
                    Escape(u["name"].ToString()),
                    Escape(u["email"].ToString()),
                    Convert.ToDouble(u["score"]),
                    Escape(u["created_at"].ToString())));
            }
            sb.Append("]");
            return sb.ToString();
        }

        static List<Dictionary<string, object>> GenerateUsersFallback(int seed)
        {
            var rng = new Random(seed);
            var firstNames = new[] { "Alice", "Bob", "Charlie", "Diana", "Eve", "Frank", "Grace", "Hector" };
            var lastNames = new[] { "Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller", "Davis" };
            var domains = new[] { "example.com", "test.org", "bench.dev" };
            var users = new List<Dictionary<string, object>>();
            for (int i = 0; i < 100; i++)
            {
                var first = firstNames[rng.Next(firstNames.Length)];
                var last = lastNames[rng.Next(lastNames.Length)];
                users.Add(new Dictionary<string, object>
                {
                    {"id", i + 1},
                    {"name", string.Format("{0} {1}", first, last)},
                    {"email", string.Format("{0}.{1}{2}@{3}",
                        first.ToLower(), last.ToLower(), rng.Next(1, 999),
                        domains[rng.Next(domains.Length)])},
                    {"score", Math.Round(rng.NextDouble() * 100, 2)},
                    {"created_at", string.Format("2026-{0:D2}-{1:D2}T{2:D2}:{3:D2}:00Z",
                        rng.Next(1, 13), rng.Next(1, 29), rng.Next(0, 24), rng.Next(0, 60))},
                });
            }
            return users;
        }

        // -- /api/transform -----------------------------------------------
        static string HandleApiTransform(HttpListenerRequest req)
        {
            var input = ReadBody(req);
            // Simple manual JSON parsing for {fields:[...], values:[...]}
            var serializer = new System.Web.Script.Serialization.JavaScriptSerializer();
            var data = serializer.Deserialize<Dictionary<string, object>>(input);

            var sb = new StringBuilder("{");

            // Hash fields
            if (data.ContainsKey("fields"))
            {
                var fields = data["fields"] as object[];
                if (fields != null)
                {
                    sb.Append("\"hashed_fields\":[");
                    for (int i = 0; i < fields.Length; i++)
                    {
                        if (i > 0) sb.Append(",");
                        var hash = Sha256Hex(fields[i].ToString());
                        sb.Append(string.Format("\"{0}\"", hash));
                    }
                    sb.Append("],");
                }
            }

            // Reverse values
            if (data.ContainsKey("values"))
            {
                var values = data["values"] as object[];
                if (values != null)
                {
                    sb.Append("\"reversed_values\":[");
                    for (int i = values.Length - 1; i >= 0; i--)
                    {
                        if (i < values.Length - 1) sb.Append(",");
                        sb.Append(values[i]);
                    }
                    sb.Append("],");
                }
            }

            // Pass through seed
            if (data.ContainsKey("seed"))
                sb.Append(string.Format("\"seed\":{0}", data["seed"]));

            sb.Append("}");
            return sb.ToString();
        }

        // -- /api/aggregate -----------------------------------------------
        static string HandleApiAggregate(HttpListenerRequest req)
        {
            var rangeStr = req.QueryString["range"] ?? "1,100";
            var parts = rangeStr.Split(',');

            double[] values;
            string[] categories;

            if (BenchDataStore.Loaded && BenchDataStore.Timeseries != null)
            {
                values = BenchDataStore.Timeseries.Select(t => Convert.ToDouble(t["value"])).ToArray();
                categories = new[] { "alpha", "beta", "gamma", "delta", "epsilon" };
            }
            else
            {
                int s;
                int seed = parts.Length > 0 && int.TryParse(parts[0], out s) ? s : 1;
                var rng = new Random(seed);
                values = new double[10000];
                for (int i = 0; i < 10000; i++)
                    values[i] = 50 + 20 * Math.Sin(i * 0.01) + (rng.NextDouble() - 0.5) * 10;
                categories = new[] { "alpha", "beta", "gamma", "delta", "epsilon" };
            }

            Array.Sort(values);
            int n = values.Length;
            double mean = values.Average();
            double p50 = values[n / 2];
            double p95 = values[(int)(n * 0.95)];
            double max = values[n - 1];

            // Group by category
            var groups = new StringBuilder("[");
            for (int c = 0; c < categories.Length; c++)
            {
                if (c > 0) groups.Append(",");
                var catValues = new List<double>();
                for (int i = c; i < values.Length; i += 5)
                    catValues.Add(values[i]);
                var catMean = catValues.Average();
                groups.Append(string.Format("{{\"category\":\"{0}\",\"count\":{1},\"mean\":{2:F4}}}",
                    categories[c], catValues.Count, catMean));
            }
            groups.Append("]");

            return string.Format("{{\"count\":{0},\"mean\":{1:F4},\"p50\":{2},\"p95\":{3},\"max\":{4},\"categories\":{5}}}",
                n, mean, p50, p95, max, groups);
        }

        // -- /api/search --------------------------------------------------
        static string HandleApiSearch(HttpListenerRequest req)
        {
            var query = req.QueryString["q"] ?? "test";
            var limitStr = req.QueryString["limit"] ?? "10";
            int l;
            int limit = int.TryParse(limitStr, out l) ? l : 10;

            List<string> corpus;
            if (BenchDataStore.Loaded && BenchDataStore.SearchCorpus != null)
            {
                corpus = BenchDataStore.SearchCorpus;
            }
            else
            {
                var rng = new Random(42);
                var words = new[] { "network", "latency", "throughput", "bandwidth", "packet", "protocol",
                    "server", "client", "proxy", "benchmark", "performance", "metric" };
                corpus = new List<string>();
                for (int i = 0; i < 1000; i++)
                    corpus.Add(string.Format("{0}-{1}-{2}",
                        words[rng.Next(words.Length)], words[rng.Next(words.Length)], rng.Next(1, 999)));
            }

            Regex re;
            try { re = new Regex(query); }
            catch { re = new Regex(Regex.Escape(query)); }

            var scored = new List<Tuple<string, int>>();
            foreach (var item in corpus)
            {
                var m = re.Match(item);
                if (m.Success)
                    scored.Add(Tuple.Create(item, 1000 - m.Index));
            }
            scored.Sort((a, b) => b.Item2.CompareTo(a.Item2));
            var results = scored.Take(limit).ToList();

            var sb = new StringBuilder("[");
            for (int i = 0; i < results.Count; i++)
            {
                if (i > 0) sb.Append(",");
                sb.Append(string.Format("{{\"item\":\"{0}\",\"score\":{1}}}",
                    Escape(results[i].Item1), results[i].Item2));
            }
            sb.Append("]");
            return sb.ToString();
        }

        // -- /api/upload/process ------------------------------------------
        static string HandleApiUploadProcess(HttpListenerRequest req)
        {
            var bodyBytes = ReadBodyBytes(req);
            int originalSize = bodyBytes.Length;

            // CRC32
            uint crc = Crc32(bodyBytes);

            // SHA-256
            string sha256;
            using (var sha = SHA256.Create())
                sha256 = BitConverter.ToString(sha.ComputeHash(bodyBytes)).Replace("-", "").ToLower();

            // Zlib compress (DeflateStream)
            byte[] compressed;
            using (var ms = new MemoryStream())
            {
                using (var ds = new DeflateStream(ms, CompressionLevel.Fastest))
                    ds.Write(bodyBytes, 0, bodyBytes.Length);
                compressed = ms.ToArray();
            }

            return string.Format("{{\"original_size\":{0},\"compressed_size\":{1},\"crc32\":\"{2:x8}\",\"sha256\":\"{3}\"}}",
                originalSize, compressed.Length, crc, sha256);
        }

        // -- /api/delayed -------------------------------------------------
        static string HandleApiDelayed(HttpListenerRequest req)
        {
            var msStr = req.QueryString["ms"] ?? "10";
            int m;
            int ms = int.TryParse(msStr, out m) ? m : 10;
            ms = Math.Max(1, Math.Min(100, ms)); // Clamp 1-100

            var sw = Stopwatch.StartNew();
            Thread.Sleep(ms);
            sw.Stop();

            return string.Format("{{\"requested_ms\":{0},\"actual_ms\":{1:F1},\"work\":\"light\"}}",
                ms, sw.Elapsed.TotalMilliseconds);
        }

        // -- /api/validate ------------------------------------------------
        static string HandleApiValidate(HttpListenerRequest req)
        {
            if (BenchDataStore.Loaded && BenchDataStore.ExpectedChecksums != null)
            {
                var sb = new StringBuilder("{\"seed\":42,\"version\":1,\"checksums\":{");
                int i = 0;
                foreach (var kv in BenchDataStore.ExpectedChecksums)
                {
                    if (i > 0) sb.Append(",");
                    sb.Append(string.Format("\"{0}\":\"{1}\"", Escape(kv.Key), Escape(kv.Value)));
                    i++;
                }
                sb.Append("}}");
                return sb.ToString();
            }

            // Fallback: compute from PRNG data
            return "{\"seed\":42,\"version\":1,\"checksums\":{\"note\":\"PRNG fallback -- install bench-data.json for cross-language validation\"}}";
        }

        // -- Helpers ------------------------------------------------------

        static string ReadBody(HttpListenerRequest req)
        {
            using (var sr = new StreamReader(req.InputStream, req.ContentEncoding))
                return sr.ReadToEnd();
        }

        static byte[] ReadBodyBytes(HttpListenerRequest req)
        {
            using (var ms = new MemoryStream())
            {
                req.InputStream.CopyTo(ms);
                return ms.ToArray();
            }
        }

        static string Sha256Hex(string input)
        {
            using (var sha = SHA256.Create())
            {
                var bytes = sha.ComputeHash(Encoding.UTF8.GetBytes(input));
                return BitConverter.ToString(bytes).Replace("-", "").ToLower();
            }
        }

        // IEEE CRC32
        static readonly uint[] Crc32Table;
        static Program()
        {
            Crc32Table = new uint[256];
            for (uint i = 0; i < 256; i++)
            {
                uint c = i;
                for (int j = 0; j < 8; j++)
                    c = (c & 1) != 0 ? 0xEDB88320 ^ (c >> 1) : c >> 1;
                Crc32Table[i] = c;
            }
        }

        static uint Crc32(byte[] data)
        {
            uint crc = 0xFFFFFFFF;
            foreach (var b in data)
                crc = Crc32Table[(crc ^ b) & 0xFF] ^ (crc >> 8);
            return crc ^ 0xFFFFFFFF;
        }

        static string Escape(string s)
        {
            return s.Replace("\\", "\\\\").Replace("\"", "\\\"");
        }
    }
}
