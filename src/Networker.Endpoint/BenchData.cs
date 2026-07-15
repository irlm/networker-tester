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

    private static BenchData? Load()
    {
        var paths = new[]
        {
            Environment.GetEnvironmentVariable("BENCH_DATA_PATH") ?? "",
            "/opt/bench/bench-data.json",
            "benchmarks/reference-apis/shared/bench-data.json",
        };

        foreach (var p in paths)
        {
            if (string.IsNullOrEmpty(p)) continue;
            try
            {
                if (!File.Exists(p)) continue;
                var content = File.ReadAllText(p);
                var root = JsonNode.Parse(content)?.AsObject();
                if (root is null) continue;

                var data = new BenchData
                {
                    Users = ArrayNodes(root["users"]),
                    SearchCorpus = StringArray(root["search_corpus"]),
                    Timeseries = ArrayNodes(root["timeseries"]),
                    ExpectedChecksums = root["expected_checksums"]?.AsObject() is { } cs
                        ? (JsonObject)cs.DeepClone()
                        : new JsonObject(),
                };
                return data;
            }
            catch
            {
                // try next path
            }
        }

        return null;
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
