using System.Text.Json;
using Networker.ControlPlane.Realtime;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// One detected metric regression. Successor of the Rust
/// <c>crates/networker-dashboard/src/regression.rs</c> <c>Regression</c> shape,
/// extended with the case identity, unit and severity the unified schema
/// stores (V047 <c>benchmark_regression</c>).
/// </summary>
public sealed record Regression(
    string CaseId,
    string Metric,
    string MetricUnit,
    double BaselineValue,
    double CurrentValue,
    double DeltaPercent,
    string Severity);

/// <summary>
/// The per-case summary statistics the regression policy consumes — parsed
/// from a benchmark artifact's <c>summaries</c> JSONB (the tester's
/// <c>BenchmarkSummary</c> array; see
/// <c>crates/networker-tester/src/output/json.rs</c>).
/// </summary>
public sealed record CaseStats(
    string CaseId,
    string MetricUnit,
    bool HigherIsBetter,
    double P50,
    long SuccessCount,
    long FailureCount,
    long IncludedSampleCount);

/// <summary>
/// Benchmark regression detection policy — the comparison logic the dashboard
/// promises on the Benchmark Regressions page (deep-measurement audit
/// M5/A12/G2: this used to be a documented stub the UI claimed was live).
///
/// <para><b>Policy.</b> When a benchmark run completes, each case's summary is
/// compared against the SAME case in the baseline run (the config's pinned
/// <c>baseline_run_id</c> when set, otherwise the previous completed run of
/// the same config — resolved by <see cref="BenchmarkRegressionDetector"/>):</para>
/// <list type="bullet">
///   <item><b>p50</b> — flagged when p50 worsens by more than
///   <see cref="P50ThresholdPct"/>% in the case's metric direction (increase
///   for latency-style cases, decrease for throughput-style
///   <c>higher_is_better</c> cases). Severity escalates to <c>critical</c>
///   beyond <see cref="P50CriticalPct"/>%.</item>
///   <item><b>success rate</b> — flagged when the case's success rate falls
///   below <see cref="SuccessRateFloorPct"/>%; <c>critical</c> below
///   <see cref="SuccessRateCriticalPct"/>%.</item>
///   <item><b>small-n guard</b> — a check is skipped unless BOTH sides have at
///   least <see cref="MinSamples"/> samples (included measured samples for
///   p50, total attempts for success rate). At smaller n the p50 estimate is
///   noise-driven (M5 §A1: an interpolated percentile at tiny n is the max
///   wearing a costume), and the bootstrap-CI machinery that could qualify it
///   has its own defect being fixed separately (M5 §A3) — so no flags on
///   noise-level runs.</item>
/// </list>
/// </summary>
public static class RegressionAnalyzer
{
    /// <summary>Relative p50 worsening (%) that flags a regression.</summary>
    public const double P50ThresholdPct = 10.0;

    /// <summary>Relative p50 worsening (%) that escalates to critical.</summary>
    public const double P50CriticalPct = 25.0;

    /// <summary>Success-rate floor (%): below this a case is flagged.</summary>
    public const double SuccessRateFloorPct = 99.0;

    /// <summary>Success-rate (%) below which the flag is critical.</summary>
    public const double SuccessRateCriticalPct = 95.0;

    /// <summary>
    /// Minimum per-case samples on BOTH sides before any comparison runs.
    /// </summary>
    public const int MinSamples = 10;

    public const string SeverityWarning = "warning";

    public const string SeverityCritical = "critical";

    /// <summary>Metric tag for latency-style p50 regressions (ms unit).</summary>
    public const string MetricP50LatencyMs = "p50_latency_ms";

    /// <summary>Metric tag for p50 regressions in any other unit.</summary>
    public const string MetricP50 = "p50";

    public const string MetricSuccessRate = "success_rate";

    /// <summary>
    /// Compare a completed run's per-case stats against the baseline run's and
    /// return every breached (case, metric). Cases absent from the baseline
    /// (new cases — nothing to compare) and cases failing the small-n guard
    /// produce nothing.
    /// </summary>
    public static IReadOnlyList<Regression> Detect(
        IReadOnlyList<CaseStats> current,
        IReadOnlyList<CaseStats> baseline)
    {
        var baselineByCase = new Dictionary<string, CaseStats>(StringComparer.Ordinal);
        foreach (var b in baseline)
        {
            baselineByCase.TryAdd(b.CaseId, b);
        }

        var regressions = new List<Regression>();
        foreach (var cur in current)
        {
            if (!baselineByCase.TryGetValue(cur.CaseId, out var basec))
            {
                continue;
            }

            CheckP50(cur, basec, regressions);
            CheckSuccessRate(cur, basec, regressions);
        }

        return regressions;
    }

    private static void CheckP50(CaseStats cur, CaseStats basec, List<Regression> outList)
    {
        // Small-n guard on the measured (included) samples of BOTH sides.
        if (cur.IncludedSampleCount < MinSamples || basec.IncludedSampleCount < MinSamples)
        {
            return;
        }

        // A non-positive baseline p50 has no meaningful relative delta.
        if (basec.P50 <= 0.0 || double.IsNaN(basec.P50) || double.IsNaN(cur.P50))
        {
            return;
        }

        var deltaPct = (cur.P50 - basec.P50) / basec.P50 * 100.0;

        // "Worse" follows the case's metric direction: latency up, throughput down.
        var worsenedPct = cur.HigherIsBetter ? -deltaPct : deltaPct;
        if (worsenedPct <= P50ThresholdPct)
        {
            return;
        }

        outList.Add(new Regression(
            cur.CaseId,
            cur.MetricUnit == "ms" ? MetricP50LatencyMs : MetricP50,
            cur.MetricUnit,
            basec.P50,
            cur.P50,
            deltaPct,
            worsenedPct > P50CriticalPct ? SeverityCritical : SeverityWarning));
    }

    private static void CheckSuccessRate(CaseStats cur, CaseStats basec, List<Regression> outList)
    {
        var curTotal = cur.SuccessCount + cur.FailureCount;
        var baseTotal = basec.SuccessCount + basec.FailureCount;

        // Small-n guard on total attempts of BOTH sides (1 failure in 5
        // attempts is 80% — noise, not signal).
        if (curTotal < MinSamples || baseTotal < MinSamples)
        {
            return;
        }

        var curRate = 100.0 * cur.SuccessCount / curTotal;
        if (curRate >= SuccessRateFloorPct)
        {
            return;
        }

        var baseRate = 100.0 * basec.SuccessCount / baseTotal;

        outList.Add(new Regression(
            cur.CaseId,
            MetricSuccessRate,
            "%",
            baseRate,
            curRate,
            curRate - baseRate, // percentage points
            curRate < SuccessRateCriticalPct ? SeverityCritical : SeverityWarning));
    }

    /// <summary>
    /// Parse a benchmark artifact's <c>summaries</c> JSON array into the
    /// fields the policy needs. Tolerant by design: a malformed document
    /// yields an empty list (detection then simply finds nothing) and unknown
    /// members are ignored — the artifact is a versioned tester-owned
    /// contract this side must not be brittle against.
    /// </summary>
    public static IReadOnlyList<CaseStats> ParseSummaries(string? summariesJson)
    {
        if (string.IsNullOrWhiteSpace(summariesJson))
        {
            return [];
        }

        try
        {
            using var doc = JsonDocument.Parse(summariesJson);
            if (doc.RootElement.ValueKind != JsonValueKind.Array)
            {
                return [];
            }

            var result = new List<CaseStats>();
            foreach (var el in doc.RootElement.EnumerateArray())
            {
                if (el.ValueKind != JsonValueKind.Object)
                {
                    continue;
                }

                var caseId = GetString(el, "case_id");
                if (caseId is null)
                {
                    continue;
                }

                result.Add(new CaseStats(
                    caseId,
                    GetString(el, "metric_unit") ?? "ms",
                    GetBool(el, "higher_is_better"),
                    GetDouble(el, "p50"),
                    GetLong(el, "success_count"),
                    GetLong(el, "failure_count"),
                    GetLong(el, "included_sample_count")));
            }

            return result;
        }
        catch (JsonException)
        {
            return [];
        }
    }

    private static string? GetString(JsonElement el, string name) =>
        el.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.String
            ? v.GetString()
            : null;

    private static bool GetBool(JsonElement el, string name) =>
        el.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.True;

    private static double GetDouble(JsonElement el, string name) =>
        el.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.Number
            ? v.GetDouble()
            : double.NaN;

    private static long GetLong(JsonElement el, string name) =>
        el.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.Number
            && v.TryGetInt64(out var l)
            ? l
            : 0;

    /// <summary>
    /// Publish a <see cref="BenchmarkRegression"/> event for a run's detected
    /// regressions. The <c>regressions</c> payload is serialized to JSON and
    /// forwarded verbatim (matching the Rust event's free-form
    /// <c>regressions</c> field). Returns the assigned event sequence number.
    /// </summary>
    public static long EmitRegressionEvent(
        EventBus bus,
        Guid configId,
        string configName,
        IReadOnlyList<Regression> regressions)
    {
        using var doc = JsonSerializer.SerializeToDocument(regressions);
        var evt = new BenchmarkRegression(
            configId,
            configName,
            regressions.Count,
            doc.RootElement.Clone());
        return bus.Publish(evt);
    }
}
