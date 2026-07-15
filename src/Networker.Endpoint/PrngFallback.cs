using System.Text.Json.Nodes;

namespace Networker.Endpoint;

/// <summary>
/// Deterministic PRNG fallback data generators used only when
/// <c>bench-data.json</c> is absent, mirroring the structure of the Rust
/// fallback branches (<c>gen_user</c>, aggregate/search corpora).
///
/// NOTE: the concrete byte values differ from Rust here because Rust seeds a
/// ChaCha12 <c>StdRng</c>. This managed PRNG is seed-deterministic (same input
/// always yields the same output) so responses remain stable per seed, but they
/// are not bit-identical to the Rust fallback. In production both servers load
/// the shared <c>bench-data.json</c> and produce identical output.
/// </summary>
internal static class PrngFallback
{
    // splitmix64 — deterministic, seedable, matches "same seed => same stream".
    private sealed class SplitMix64
    {
        private ulong _state;
        public SplitMix64(ulong seed) => _state = seed;

        public ulong NextU64()
        {
            _state += 0x9E3779B97F4A7C15UL;
            var z = _state;
            z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9UL;
            z = (z ^ (z >> 27)) * 0x94D049BB133111EBUL;
            return z ^ (z >> 31);
        }

        /// <summary>Uniform in [0, 1).</summary>
        public double NextF64() => (NextU64() >> 11) * (1.0 / (1UL << 53));

        /// <summary>Uniform integer in [lo, hi).</summary>
        public int Range(int lo, int hi) => lo + (int)(NextU64() % (ulong)(hi - lo));
    }

    private static readonly string[] FirstNames =
    {
        "Alice", "Bob", "Charlie", "Diana", "Eve", "Frank", "Grace", "Hector", "Iris", "Jack", "Karen",
        "Leo", "Mona", "Nick", "Olivia", "Paul", "Quinn", "Rosa", "Steve", "Tina", "Uma", "Victor",
        "Wendy", "Xander", "Yuki", "Zane",
    };

    private static readonly string[] LastNames =
    {
        "Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller", "Davis", "Rodriguez",
        "Martinez", "Hernandez", "Lopez", "Gonzalez", "Wilson", "Anderson", "Thomas", "Taylor",
        "Moore", "Jackson", "Martin", "Lee", "Perez", "Thompson", "White", "Harris", "Sanchez",
    };

    private static readonly string[] Domains =
    {
        "example.com", "test.org", "mail.net", "corp.io", "bench.dev",
    };

    internal static readonly string[] SearchWords =
    {
        "network", "latency", "throughput", "bandwidth", "packet", "server", "client", "request",
        "response", "timeout", "connection", "socket", "protocol", "testing", "benchmark",
        "performance", "endpoint", "proxy", "firewall", "router", "switch", "gateway", "dns",
        "tls", "quic",
    };

    private static JsonObject GenUser(SplitMix64 rng, ulong id)
    {
        var first = FirstNames[rng.Range(0, FirstNames.Length)];
        var last = LastNames[rng.Range(0, LastNames.Length)];
        var domain = Domains[rng.Range(0, Domains.Length)];
        var score = Math.Round(rng.NextF64() * 10000.0) / 100.0;
        var day = rng.Range(1, 29);
        var month = rng.Range(1, 13);
        var year = rng.Range(2018, 2026);
        return new JsonObject
        {
            ["id"] = id,
            ["name"] = $"{first} {last}",
            ["email"] = $"{first.ToLowerInvariant()}.{last.ToLowerInvariant()}@{domain}",
            ["score"] = score,
            ["created_at"] = $"{year:D4}-{month:D2}-{day:D2}T00:00:00Z",
        };
    }

    public static List<JsonNode?> GenUsers(ulong page)
    {
        var rng = new SplitMix64(page);
        var list = new List<JsonNode?>();
        for (ulong i = 0; i < 100; i++)
            list.Add(GenUser(rng, (page - 1) * 100 + i + 1));
        return list;
    }

    public static List<double> GenTimeseries(ulong start)
    {
        var rng = new SplitMix64(start);
        var list = new List<double>(10_000);
        for (var i = 0; i < 10_000; i++)
            list.Add(rng.NextF64() * 1000.0);
        return list;
    }

    public static List<string> GenSearchCorpus()
    {
        var rng = new SplitMix64(42);
        var list = new List<string>(1_000);
        for (var i = 0; i < 1_000; i++)
        {
            var w1 = SearchWords[rng.Range(0, SearchWords.Length)];
            var w2 = SearchWords[rng.Range(0, SearchWords.Length)];
            var n = rng.Range(1, 1000);
            list.Add($"{w1}-{w2}-{n}");
        }
        return list;
    }
}
