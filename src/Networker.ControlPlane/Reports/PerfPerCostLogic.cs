namespace Networker.ControlPlane.Reports;

/// <summary>
/// Pure computation for the performance-per-cost report — the mode-family
/// mapping and the two value formulas, kept free of I/O so the numbers are
/// unit-testable. The formulas are deliberately simple and dimensioned so
/// they can be defended to a network engineer (docs/reports-perf-per-cost.md):
///
/// <list type="bullet">
///   <item><b>Latency families</b> (net / http / page):
///     <c>latency_cost_index = p95_ms × hourly_usd</c> — dollar-weighted tail
///     latency, LOWER is better. A VM that is twice as expensive must halve
///     p95 to break even.</item>
///   <item><b>Throughput family</b> (thru):
///     <c>mbps_per_dollar_hour = median_mbps ÷ hourly_usd</c> — sustained
///     megabits you get per dollar-hour, HIGHER is better.</item>
/// </list>
/// </summary>
public static class PerfPerCostLogic
{
    public const string FamilyNet = "net";
    public const string FamilyHttp = "http";
    public const string FamilyPage = "page";
    public const string FamilyThru = "thru";

    /// <summary>
    /// Tester-protocol wire id → dashboard mode family. Must stay in sync
    /// with <c>shared/modes.json</c> (tester-level entries); guarded by
    /// <c>PerfPerCostLogicTests.Family_map_matches_shared_modes_manifest</c>.
    /// </summary>
    public static readonly IReadOnlyDictionary<string, string> ProtocolFamily =
        new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase)
        {
            ["tcp"] = FamilyNet,
            ["dns"] = FamilyNet,
            ["tls"] = FamilyNet,
            ["tlsresume"] = FamilyNet,
            ["native"] = FamilyNet,
            ["udp"] = FamilyNet,
            ["rpm"] = FamilyNet,
            ["http1"] = FamilyHttp,
            ["http2"] = FamilyHttp,
            ["http3"] = FamilyHttp,
            ["curl"] = FamilyHttp,
            ["sdkprobe"] = FamilyHttp,
            ["pageload"] = FamilyPage,
            ["pageload2"] = FamilyPage,
            ["pageload3"] = FamilyPage,
            ["browser"] = FamilyPage,
            ["browser1"] = FamilyPage,
            ["browser2"] = FamilyPage,
            ["browser3"] = FamilyPage,
            ["download"] = FamilyThru,
            ["download1"] = FamilyThru,
            ["download2"] = FamilyThru,
            ["download3"] = FamilyThru,
            ["upload"] = FamilyThru,
            ["upload1"] = FamilyThru,
            ["upload2"] = FamilyThru,
            ["upload3"] = FamilyThru,
            ["webdownload"] = FamilyThru,
            ["webupload"] = FamilyThru,
            ["udpdownload"] = FamilyThru,
            ["udpupload"] = FamilyThru,
        };

    /// <summary>
    /// The SQL CASE expression classifying <c>RequestAttempt.Protocol</c> into
    /// a family — generated from <see cref="ProtocolFamily"/> so SQL and C#
    /// cannot drift. Protocol ids are compile-time constants from the map
    /// (never user input), so inlining them is injection-safe.
    /// </summary>
    public static string FamilyCaseSql(string protocolColumn)
    {
        var arms = ProtocolFamily
            .GroupBy(kv => kv.Value)
            .OrderBy(g => g.Key, StringComparer.Ordinal)
            .Select(g =>
                $"WHEN LOWER({protocolColumn}) IN ({string.Join(", ", g.Select(kv => $"'{kv.Key.ToLowerInvariant()}'").OrderBy(s => s, StringComparer.Ordinal))}) THEN '{g.Key}'");
        // Unknown protocols (future modes) land in 'net' rather than vanishing.
        return $"CASE {string.Join(" ", arms)} ELSE '{FamilyNet}' END";
    }

    /// <summary>Is the family's primary metric a latency (ms, lower-better)?
    /// (thru is throughput, higher-better; everything else is latency.)</summary>
    public static bool IsLatencyFamily(string family) => family != FamilyThru;

    /// <summary>
    /// <c>p95_ms × hourly_usd</c> — dollar-weighted p95, lower is better.
    /// Null when either input is unknown (missing cost row / no samples):
    /// a value score is never fabricated.
    /// </summary>
    public static double? LatencyCostIndex(double? p95Ms, decimal? hourlyUsd) =>
        p95Ms is double p && hourlyUsd is decimal c && p >= 0 && c > 0
            ? Math.Round(p * (double)c, 4)
            : null;

    /// <summary>
    /// <c>median_mbps ÷ hourly_usd</c> — sustained Mbps per dollar-hour,
    /// higher is better. Null when either input is unknown.
    /// </summary>
    public static double? MbpsPerDollarHour(double? medianMbps, decimal? hourlyUsd) =>
        medianMbps is double m && hourlyUsd is decimal c && m >= 0 && c > 0
            ? Math.Round(m / (double)c, 4)
            : null;

    /// <summary>Human-readable formula strings embedded in every response so
    /// the score is self-documenting on the wire.</summary>
    public static class Formulas
    {
        public const string LatencyCostIndexText =
            "latency_cost_index = p95_ms * hourly_usd (dollar-weighted p95 latency; lower is better)";

        public const string MbpsPerDollarHourText =
            "mbps_per_dollar_hour = median_throughput_mbps / hourly_usd (sustained Mbps per dollar-hour; higher is better)";
    }
}
