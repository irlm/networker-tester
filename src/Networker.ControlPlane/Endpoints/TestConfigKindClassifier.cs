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
        out string testKind,
        string? name = null)
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
                : IsUrlProbeName(name)
                    ? TestConfigKinds.UrlProbe
                    : TestConfigKinds.Network;
        return true;
    }

    /// <summary>Mirror of the V053 backfill's name rule, so an old dashboard
    /// bundle mid-rolling-deploy (or an API script) creating a diag-style
    /// config WITHOUT an explicit test_kind still lands under URL probes —
    /// find-or-create reuses configs by name and never re-classifies, so a
    /// rollout-window miss would otherwise be permanent. Covers all naming
    /// generations: "Diag: ", multi-URL "Diag set: " (v0.28.231), and the
    /// pre-rename watchlist "Probe: ".</summary>
    private static bool IsUrlProbeName(string? name) =>
        name is not null
        && (name.StartsWith("Diag: ", StringComparison.Ordinal)
            || name.StartsWith("Diag set: ", StringComparison.Ordinal)
            || name.StartsWith("Probe: ", StringComparison.Ordinal));

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
