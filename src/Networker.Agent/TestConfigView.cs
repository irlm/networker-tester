using System.Text.Json;

namespace Networker.Agent;

/// <summary>
/// A read-only projection over the opaque <c>config</c> JsonElement carried by
/// <c>assign_run</c>. The agent never re-models the full canonical
/// <c>TestConfig</c> (crates/networker-common/src/test_config.rs) — it only
/// reads the handful of fields the Rust executor's <c>build_args</c> /
/// <c>endpoint_to_target</c> / artifact synthesis touch. Unknown members are
/// ignored, so the config can grow additively.
/// </summary>
public sealed class TestConfigView
{
    public required Guid Id { get; init; }
    public required string EndpointKind { get; init; }
    public EndpointNetwork? Network { get; init; }
    public required IReadOnlyList<string> Modes { get; init; }
    public required uint Runs { get; init; }
    public required uint Concurrency { get; init; }
    public required uint TimeoutMs { get; init; }
    public required IReadOnlyList<uint> PayloadSizes { get; init; }
    public required bool Insecure { get; init; }
    public required bool IsBenchmark { get; init; }
    public JsonElement Methodology { get; init; }

    public sealed record EndpointNetwork(string Host, ushort? Port);

    /// <summary>Parse the assign_run <c>config</c> element into a view. Throws
    /// <see cref="JsonException"/> on a structurally broken document.</summary>
    public static TestConfigView From(JsonElement config)
    {
        var endpoint = config.GetProperty("endpoint");
        var kind = endpoint.TryGetProperty("kind", out var k) ? k.GetString() ?? "unknown" : "unknown";

        EndpointNetwork? network = null;
        if (kind == "network")
        {
            var host = endpoint.TryGetProperty("host", out var h) ? h.GetString() ?? "" : "";
            ushort? port = endpoint.TryGetProperty("port", out var p) && p.ValueKind == JsonValueKind.Number
                ? (ushort)p.GetUInt32()
                : null;
            network = new EndpointNetwork(host, port);
        }

        var workload = config.GetProperty("workload");

        var modes = new List<string>();
        if (workload.TryGetProperty("modes", out var modesEl) && modesEl.ValueKind == JsonValueKind.Array)
        {
            foreach (var m in modesEl.EnumerateArray())
            {
                if (m.GetString() is { } s)
                    modes.Add(s);
            }
        }

        var payloadSizes = new List<uint>();
        if (workload.TryGetProperty("payload_sizes", out var psEl) && psEl.ValueKind == JsonValueKind.Array)
        {
            foreach (var n in psEl.EnumerateArray())
            {
                if (n.ValueKind == JsonValueKind.Number)
                    payloadSizes.Add(n.GetUInt32());
            }
        }

        var methodologyPresent = config.TryGetProperty("methodology", out var methEl)
                                 && methEl.ValueKind is not JsonValueKind.Null and not JsonValueKind.Undefined;

        return new TestConfigView
        {
            Id = config.TryGetProperty("id", out var idEl) && idEl.TryGetGuid(out var id) ? id : Guid.Empty,
            EndpointKind = kind,
            Network = network,
            Modes = modes,
            Runs = GetUInt(workload, "runs", 1),
            Concurrency = GetUInt(workload, "concurrency", 1),
            TimeoutMs = GetUInt(workload, "timeout_ms", 30_000),
            PayloadSizes = payloadSizes,
            Insecure = workload.TryGetProperty("insecure", out var insEl) && insEl.ValueKind == JsonValueKind.True,
            IsBenchmark = methodologyPresent,
            Methodology = methodologyPresent ? methEl.Clone() : default,
        };
    }

    private static uint GetUInt(JsonElement obj, string name, uint fallback) =>
        obj.TryGetProperty(name, out var el) && el.ValueKind == JsonValueKind.Number
            ? el.GetUInt32()
            : fallback;
}
