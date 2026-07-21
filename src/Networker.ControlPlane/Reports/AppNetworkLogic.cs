namespace Networker.ControlPlane.Reports;

/// <summary>
/// Pure computation for the <b>Application Network Performance</b> report — the
/// split between time spent in the customer's application (server) and time
/// spent on the network, for <c>sdkprobe</c> runs against a LagHound SDK
/// endpoint. Kept free of I/O so the verdict and the numbers are unit-testable.
///
/// <para>The report never invents data: for each successful sdkprobe attempt the
/// SQL layer supplies two measured quantities —</para>
/// <list type="bullet">
///   <item><b>wall</b> — the attempt's end-to-end latency (RequestAttempt
///     started→finished), the same wall-time PerfPerCost uses.</item>
///   <item><b>server_ms</b> — <c>ServerTimingResult.TotalServerMs</c>, the
///     LagHound SDK's <c>Server-Timing: total;dur</c> (the application's own
///     processing time), joined per attempt on AttemptId.</item>
/// </list>
/// <para>From those, <c>network_ms = max(0, wall − server_ms)</c>. A row where
/// <c>server_ms &gt; wall</c> is a <b>split anomaly</b> (clock skew, or the SDK
/// timing a longer span than the client observed) — it is counted, and its
/// network_ms floors at 0 so it can never go negative.</para>
///
/// <para>The verdict answers the product question — "is my slowness the app or
/// the network?" — from the group's median server and median network:</para>
/// <list type="bullet">
///   <item><b>server_bound</b> — median server_ms ≥ 60% of median wall.</item>
///   <item><b>network_bound</b> — median network_ms ≥ 60% of median wall.</item>
///   <item><b>balanced</b> — neither dominates.</item>
/// </list>
/// </summary>
public static class AppNetworkLogic
{
    public const string VerdictServerBound = "server_bound";
    public const string VerdictNetworkBound = "network_bound";
    public const string VerdictBalanced = "balanced";
    public const string VerdictNoData = "no_data";

    /// <summary>The dominance threshold: a side "bounds" the latency when its
    /// median is at least this fraction of the median wall time.</summary>
    public const double DominanceRatio = 0.60;

    /// <summary>
    /// Per-attempt network time: <c>max(0, wall − server)</c>. Floors at 0 so a
    /// split anomaly (server &gt; wall) never yields a negative network time.
    /// </summary>
    public static double NetworkMs(double wallMs, double serverMs) =>
        Math.Max(0.0, wallMs - serverMs);

    /// <summary>True when the SDK's reported server time exceeds the observed
    /// wall time — a measurement anomaly (clock skew / span mismatch).</summary>
    public static bool IsSplitAnomaly(double wallMs, double serverMs) => serverMs > wallMs;

    /// <summary>
    /// The verdict for a group from its median server / network / wall times.
    /// Returns <see cref="VerdictNoData"/> when there is nothing to judge
    /// (no samples, so wall is null or ≤ 0).
    /// </summary>
    public static string Verdict(double? medianServerMs, double? medianNetworkMs, double? medianWallMs)
    {
        if (medianWallMs is not double wall || wall <= 0
            || medianServerMs is not double server
            || medianNetworkMs is not double network)
        {
            return VerdictNoData;
        }

        // Server dominance is checked first: when the application eats the wall
        // clock, that is the headline even if network is also non-trivial.
        if (server >= DominanceRatio * wall)
        {
            return VerdictServerBound;
        }
        if (network >= DominanceRatio * wall)
        {
            return VerdictNetworkBound;
        }
        return VerdictBalanced;
    }

    /// <summary>
    /// The share of median wall time attributable to the server (0..1), or null
    /// when there is no wall time to divide by. Rounded to 4 dp.
    /// </summary>
    public static double? ServerRatio(double? medianServerMs, double? medianWallMs) =>
        medianWallMs is double wall && wall > 0 && medianServerMs is double server && server >= 0
            ? Math.Round(Math.Min(1.0, server / wall), 4)
            : null;

    /// <summary>
    /// A human-readable one-liner for the verdict, e.g. the server-bound message
    /// the product spec calls for. Numbers are rounded to whole ms for reading.
    /// </summary>
    public static string MainIssue(
        string verdict, double? medianServerMs, double? medianNetworkMs, double? medianWallMs)
    {
        var server = medianServerMs is double s ? Math.Round(s) : 0;
        var network = medianNetworkMs is double n ? Math.Round(n) : 0;
        var total = medianWallMs is double w ? Math.Round(w) : 0;

        return verdict switch
        {
            VerdictServerBound =>
                $"Server processing dominates: ~{server:0}ms of ~{total:0}ms — investigate your application, not the network.",
            VerdictNetworkBound =>
                $"Network transit dominates: ~{network:0}ms of ~{total:0}ms — investigate connectivity/routing, not your application.",
            VerdictBalanced =>
                $"Balanced: ~{server:0}ms server vs ~{network:0}ms network of ~{total:0}ms — no single dominant cost.",
            _ =>
                "No sdkprobe samples for this selection — run an SDK-endpoint probe to populate the split.",
        };
    }

    /// <summary>Round a nullable value to 4 dp (null stays null).</summary>
    public static double? Round4(double? v) => v is double d ? Math.Round(d, 4) : null;
}
