using System.Text.Json;
using System.Text.Json.Nodes;

namespace Networker.Endpoint;

/// <summary>
/// Shared benchmark dataset loaded once from <c>bench-data.json</c>, mirroring
/// the Rust <c>BenchData</c> struct and <c>load_bench_data()</c> search path.
///
/// When the file is present, the JSON API endpoints produce byte-identical
/// output to the Rust server (they read the same records). When it is absent,
/// both servers fall back to PRNG-generated data — see the note in the port
/// report: the fallback data is NOT byte-identical to Rust because Rust seeds a
/// ChaCha-based <c>StdRng</c>, which cannot be reproduced from managed code.
/// The deterministic path used in production (shared file present) matches.
/// </summary>
public sealed class BenchData
{
    public List<JsonNode?> Users { get; init; } = new();
    public List<string> SearchCorpus { get; init; } = new();
    public List<JsonNode?> Timeseries { get; init; } = new();
    public JsonObject ExpectedChecksums { get; init; } = new();

    private static readonly Lazy<BenchData?> _cached = new(Load);

    public static BenchData? Instance => _cached.Value;

    /// <summary>
    /// Eagerly resolves the dataset at startup so a misconfigured
    /// <c>BENCH_DATA_PATH</c> (or a corrupt on-disk dataset) is fatal
    /// immediately instead of silently benchmarking different data
    /// (API-SPEC.md §2, audit F2).
    /// </summary>
    public static void EnsureLoaded() => _ = Instance;

    private static BenchData? Load()
    {
        // Explicitly configured path: any failure is fatal (API-SPEC.md §2).
        var envPath = Environment.GetEnvironmentVariable("BENCH_DATA_PATH");
        if (!string.IsNullOrEmpty(envPath))
        {
            try
            {
                return Parse(File.ReadAllText(envPath));
            }
            catch (Exception e)
            {
                Console.Error.WriteLine(
                    $"FATAL: BENCH_DATA_PATH={envPath} could not be loaded: {e.Message} " +
                    "(dataset load failure must not fall back to PRNG data)");
                Environment.Exit(1);
            }
        }

        // Fallback paths: a file that exists but fails to parse is fatal too.
        foreach (var p in new[]
                 {
                     "/opt/bench/bench-data.json",
                     "benchmarks/reference-apis/shared/bench-data.json",
                 })
        {
            if (!File.Exists(p)) continue;
            try
            {
                return Parse(File.ReadAllText(p));
            }
            catch (Exception e)
            {
                Console.Error.WriteLine(
                    $"FATAL: bench-data.json exists at {p} but could not be loaded: {e.Message}");
                Environment.Exit(1);
            }
        }

        return null;
    }

    private static BenchData Parse(string content)
    {
        var root = JsonNode.Parse(content)?.AsObject()
            ?? throw new JsonException("root is not a JSON object");

        return new BenchData
        {
            Users = ArrayNodes(root["users"]),
            SearchCorpus = StringArray(root["search_corpus"]),
            Timeseries = ArrayNodes(root["timeseries"]),
            ExpectedChecksums = root["expected_checksums"]?.AsObject() is { } cs
                ? (JsonObject)cs.DeepClone()
                : new JsonObject(),
        };
    }

    private static List<JsonNode?> ArrayNodes(JsonNode? node)
    {
        var list = new List<JsonNode?>();
        if (node is JsonArray arr)
            foreach (var el in arr)
                list.Add(el?.DeepClone());
        return list;
    }

    private static List<string> StringArray(JsonNode? node)
    {
        var list = new List<string>();
        if (node is JsonArray arr)
            foreach (var el in arr)
                if (el is not null && el.GetValueKind() == JsonValueKind.String)
                    list.Add(el.GetValue<string>());
        return list;
    }
}
