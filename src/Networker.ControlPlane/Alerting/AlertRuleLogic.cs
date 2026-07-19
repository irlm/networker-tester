using System.Globalization;

namespace Networker.ControlPlane.Alerting;

/// <summary>
/// The pure, side-effect-free core of alert evaluation: metric/comparator
/// vocabularies, breach + window decisions, and firing/resolved state
/// transitions. Everything here is deterministic and unit-tested without a
/// database — <see cref="AlertEvaluator"/> supplies the I/O around it.
/// </summary>
public static class AlertRuleLogic
{
    /// <summary>Metrics a rule can watch (matches the V041 CHECK constraint).</summary>
    public static readonly IReadOnlySet<string> Metrics = new HashSet<string>(StringComparer.Ordinal)
    {
        MetricP95Ms, MetricMeanMs, MetricErrorRate, MetricSuccessRate,
    };

    public const string MetricP95Ms = "p95_ms";
    public const string MetricMeanMs = "mean_ms";
    public const string MetricErrorRate = "error_rate";
    public const string MetricSuccessRate = "success_rate";

    /// <summary>Comparators (matches the V041 CHECK constraint).</summary>
    public static readonly IReadOnlySet<string> Comparators = new HashSet<string>(StringComparer.Ordinal)
    {
        ComparatorGt, ComparatorLt,
    };

    public const string ComparatorGt = "gt";
    public const string ComparatorLt = "lt";

    /// <summary>Event / rule states.</summary>
    public const string StateFiring = "firing";
    public const string StateResolved = "resolved";

    /// <summary>window_runs bounds (matches the V041 CHECK constraint).</summary>
    public const int MinWindowRuns = 1;
    public const int MaxWindowRuns = 50;

    /// <summary>
    /// Strict comparison — a value exactly at the threshold does NOT breach
    /// (gt means &gt;, lt means &lt;). Unknown comparators never breach.
    /// </summary>
    public static bool Breaches(string comparator, double value, double threshold) => comparator switch
    {
        ComparatorGt => value > threshold,
        ComparatorLt => value < threshold,
        _ => false,
    };

    /// <summary>
    /// Window decision over the last terminal runs' metric values, NEWEST
    /// FIRST (index 0 = the run that just finished). Breached only when there
    /// are at least <paramref name="windowRuns"/> values AND the newest
    /// <paramref name="windowRuns"/> of them are all non-null and all breach.
    /// A null (metric not measurable for that run) breaks the streak — the
    /// rule never fires on missing data.
    /// </summary>
    public static bool WindowBreached(
        IReadOnlyList<double?> newestFirstValues,
        int windowRuns,
        string comparator,
        double threshold)
    {
        if (windowRuns < MinWindowRuns || newestFirstValues.Count < windowRuns)
        {
            return false;
        }

        for (var i = 0; i < windowRuns; i++)
        {
            if (newestFirstValues[i] is not { } value || !Breaches(comparator, value, threshold))
            {
                return false;
            }
        }

        return true;
    }

    /// <summary>
    /// State transition with dedup: returns the state of the event to record
    /// (<see cref="StateFiring"/> / <see cref="StateResolved"/>) or null when
    /// nothing changes (already firing and still breaching, or quiet and still
    /// quiet). <paramref name="currentlyFiring"/> is the latest recorded event
    /// state for this (rule, config) pair.
    /// </summary>
    public static string? NextTransition(bool currentlyFiring, bool windowBreached) =>
        (currentlyFiring, windowBreached) switch
        {
            (false, true) => StateFiring,
            (true, false) => StateResolved,
            _ => null,
        };

    /// <summary>
    /// success_rate ∈ [0,1] from run counts; null when the run recorded no
    /// attempts at all (no data ≠ 0% ≠ 100%).
    /// </summary>
    public static double? SuccessRate(int successCount, int failureCount)
    {
        var total = successCount + failureCount;
        return total <= 0 ? null : (double)successCount / total;
    }

    /// <summary>error_rate ∈ [0,1] from run counts; null when no attempts.</summary>
    public static double? ErrorRate(int successCount, int failureCount)
    {
        var total = successCount + failureCount;
        return total <= 0 ? null : (double)failureCount / total;
    }

    /// <summary>Human-readable event message (stored on alert_event, sent in payloads).</summary>
    public static string BuildMessage(
        string state, string metric, string comparator, double threshold, double value, int windowRuns)
    {
        var op = comparator == ComparatorLt ? "<" : ">";
        var v = value.ToString("0.###", CultureInfo.InvariantCulture);
        var t = threshold.ToString("0.###", CultureInfo.InvariantCulture);
        return state == StateResolved
            ? $"{metric} {v} back within threshold {t} ({op})"
            : $"{metric} {v} {op} {t} for {windowRuns} consecutive run(s)";
    }
}
