using System.Text.Json;
using Networker.Data;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// One compatibility boundary for generic config writers. New product flows
/// send an explicit test_kind; older clients are classified from the workload
/// and methodology so the stored taxonomy remains useful during rollout.
/// </summary>
internal static class TestConfigKindClassifier
{
    internal static bool TryResolve(
        string? requested,
        JsonElement workload,
        JsonElement? methodology,
        out string testKind)
    {
        if (!string.IsNullOrWhiteSpace(requested))
        {
            testKind = requested.Trim().ToLowerInvariant();
            return TestConfigKinds.IsValid(testKind);
        }

        testKind = HasMode(workload, "sdkprobe")
            ? TestConfigKinds.SdkProbe
            : HasMethodology(methodology) || HasMode(workload, "apibench")
                ? TestConfigKinds.Benchmark
                : TestConfigKinds.Network;
        return true;
    }

    private static bool HasMethodology(JsonElement? methodology) =>
        methodology is { ValueKind: not JsonValueKind.Null and not JsonValueKind.Undefined };

    private static bool HasMode(JsonElement workload, string expected)
    {
        if (workload.ValueKind != JsonValueKind.Object
            || !workload.TryGetProperty("modes", out var modes)
            || modes.ValueKind != JsonValueKind.Array)
        {
            return false;
        }

        return modes.EnumerateArray().Any(mode =>
            mode.ValueKind == JsonValueKind.String
            && string.Equals(mode.GetString(), expected, StringComparison.OrdinalIgnoreCase));
    }
}
