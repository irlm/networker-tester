using System.Globalization;
using Networker.ControlPlane.Endpoints;
using Networker.ControlPlane.Infra;

namespace Networker.ControlPlane.Reports;

/// <summary>One side of the run's infrastructure for reporting: identity +
/// catalog specs + effective price + the same-cloud alternative pool. The
/// typed counterpart of the /infra wire shape, shared by the report builder
/// (assessment below) and composable by the endpoint.</summary>
public sealed record ReportInfraSide(
    string Cloud,
    string? VmSize,
    string? Region,
    VmNetworkSpecs.VmSpec? Specs,
    double? HourlyUsd,
    IReadOnlyList<ReportAltSize> Alternatives);

/// <summary>A same-cloud alternative size (specs ⋈ price; null price = unknown).</summary>
public sealed record ReportAltSize(
    string VmSize,
    int EgressMbps,
    string Confidence,
    bool AcceleratedNetworking,
    double? HourlyUsd);

/// <summary>Everything the envelope section needs beyond the attempts:
/// both sides plus the runner's load/core facts from the run envelope.</summary>
public sealed record ReportRunInfra(
    ReportInfraSide? Runner,
    ReportInfraSide? Target,
    double? PeakLoad1m,
    int? CpuCores);

/// <summary>Per-direction verdict — the C# port of the dashboard's
/// lib/infra.ts assessment so exported reports and the run page cannot
/// disagree. Confidence: measured (mthroughput capacity) > documented >
/// estimated.</summary>
public sealed record DirectionAssessment(
    string Direction,
    double MeasuredMbps,
    long PayloadBytes,
    double? ExpectedMbps,
    string? Confidence,
    string? LimitingSide,
    double? Utilization,
    string Verdict);

/// <summary>A right-sizing suggestion (port of lib/advisor.ts): at most one
/// per side; a network-bound side gets the cheapest upsize clearing 1.5× its
/// ceiling, an idle side (<40% everywhere it carries) the cheapest downsize
/// still clearing measured×1.5. Prices are real or "price unknown" — never
/// invented.</summary>
public sealed record InfraSuggestion(string Kind, string Text);

public static class RunInfraAssessment
{
    private const double NetworkBoundUtilization = 0.8;
    private const double UpsizeMinFactor = 1.5;
    private const double DownsizeMaxUtilization = 0.4;
    private const double DownsizeHeadroomFactor = 1.5;

    private static readonly string[] DownloadProtocols = ["download", "webdownload"];
    private static readonly string[] UploadProtocols = ["upload", "webupload"];

    public static IReadOnlyList<DirectionAssessment> Assess(
        IReadOnlyList<AttemptView> attempts, ReportRunInfra? infra)
    {
        var result = new List<DirectionAssessment>(2);
        foreach (var direction in new[] { "download", "upload" })
        {
            if (AssessDirection(direction, attempts, infra) is { } a)
            {
                result.Add(a);
            }
        }
        return result;
    }

    private static DirectionAssessment? AssessDirection(
        string direction, IReadOnlyList<AttemptView> attempts, ReportRunInfra? infra)
    {
        var protos = direction == "download" ? DownloadProtocols : UploadProtocols;
        var steady = SteadyStateMbps(attempts, protos);
        if (steady is null)
        {
            return null;
        }

        // Sending side per direction: download ← target egress, upload ← runner
        // egress (clouds cap egress, not ingress). A measured multi-stream
        // capacity supersedes the catalog spec and is a PATH fact — not
        // attributed to one side.
        var limiting = direction == "download" ? infra?.Target : infra?.Runner;
        var specSide = direction == "download" ? "target" : "runner";
        var empirical = EmpiricalCapacityMbps(attempts, direction);
        var expected = empirical ?? limiting?.Specs?.EgressMbps;
        var confidence = empirical is not null ? "measured" : limiting?.Specs?.Confidence;
        var limitingSide = empirical is not null ? null : (expected is not null ? specSide : null);
        var utilization = expected is > 0 ? steady.Value.Mbps / expected : null;

        string verdict;
        if (utilization is >= NetworkBoundUtilization)
        {
            verdict = "network-bound";
        }
        else if (RunnerCpuSaturated(infra))
        {
            verdict = "cpu-bound";
        }
        else if (PathLossSignal(attempts, protos))
        {
            verdict = "path-bound";
        }
        else if (utilization is not null)
        {
            verdict = "headroom";
        }
        else
        {
            verdict = "unknown";
        }

        return new DirectionAssessment(
            direction, steady.Value.Mbps, steady.Value.PayloadBytes,
            expected, confidence, limitingSide, utilization, verdict);
    }

    /// <summary>Largest-payload p50 throughput, MB/s → Mbps. Success only;
    /// small payloads measure slow-start burst, so only the largest counts.</summary>
    private static (double Mbps, long PayloadBytes)? SteadyStateMbps(
        IReadOnlyList<AttemptView> attempts, string[] protocols)
    {
        var groups = attempts
            .Where(a => a.Success
                        && protocols.Contains(a.Protocol, StringComparer.OrdinalIgnoreCase)
                        && a.Http?.ThroughputMbps is not null
                        && a.Http?.PayloadBytes is not null)
            .GroupBy(a => a.Http!.PayloadBytes!.Value)
            .OrderByDescending(g => g.Key)
            .FirstOrDefault();
        if (groups is null)
        {
            return null;
        }
        var p50 = Percentile(groups.Select(a => a.Http!.ThroughputMbps!.Value).ToList(), 0.50);
        return p50 is null ? null : (p50.Value * 8, groups.Key);
    }

    /// <summary>Newest successful mthroughput attempt's capacity for the
    /// direction, MB/s → Mbps (V005 persistence).</summary>
    internal static double? EmpiricalCapacityMbps(
        IReadOnlyList<AttemptView> attempts, string direction)
    {
        for (var i = attempts.Count - 1; i >= 0; i--)
        {
            var a = attempts[i];
            if (!a.Success
                || !string.Equals(a.Protocol, "mthroughput", StringComparison.OrdinalIgnoreCase)
                || a.Mthroughput is null)
            {
                continue;
            }
            var cap = direction == "download"
                ? a.Mthroughput.CapacityDownMbps
                : a.Mthroughput.CapacityUpMbps;
            if (cap is not null)
            {
                return cap * 8;
            }
        }
        return null;
    }

    private static bool RunnerCpuSaturated(ReportRunInfra? infra) =>
        infra is { CpuCores: > 0, PeakLoad1m: { } load } && load >= infra.CpuCores;

    private static bool PathLossSignal(IReadOnlyList<AttemptView> attempts, string[] protocols)
    {
        var retrans = attempts.Any(a =>
            protocols.Contains(a.Protocol, StringComparer.OrdinalIgnoreCase)
            && a.Tcp?.TotalRetrans is > 0);
        var udpLoss = attempts.Any(a =>
            string.Equals(a.Protocol, "udp", StringComparison.OrdinalIgnoreCase)
            && a.Udp?.LossPercent is > 1);
        return retrans || udpLoss;
    }

    // ── advisor (port of lib/advisor.ts) ────────────────────────────────────

    public static IReadOnlyList<InfraSuggestion> Advise(
        IReadOnlyList<DirectionAssessment> assessments, ReportRunInfra? infra)
    {
        if (infra is null)
        {
            return Array.Empty<InfraSuggestion>();
        }

        var result = new List<InfraSuggestion>(2);
        foreach (var (side, name) in new[] { (infra.Target, "target"), (infra.Runner, "runner") })
        {
            if (side is null)
            {
                continue;
            }
            var bound = assessments.FirstOrDefault(a =>
                a.Verdict == "network-bound" && a.LimitingSide == name);
            var s = bound is not null
                ? Upsize(bound, side, name)
                : Downsize(assessments, side, name);
            if (s is not null)
            {
                result.Add(s);
            }
        }
        return result;
    }

    private static InfraSuggestion? Upsize(DirectionAssessment a, ReportInfraSide side, string name)
    {
        if (side.VmSize is null || side.Specs is null || side.Alternatives.Count == 0)
        {
            return null;
        }
        var alt = CheapestAbove(side.Alternatives, side.Specs.EgressMbps * UpsizeMinFactor);
        if (alt is null)
        {
            return null;
        }
        var delta = side.HourlyUsd is { } cur && alt.HourlyUsd is { } next ? next - cur : (double?)null;
        var accel = alt.AcceleratedNetworking && !side.Specs.AcceleratedNetworking
            ? " + accelerated networking" : "";
        return new InfraSuggestion("upsize",
            $"{a.Direction} is at the {name}'s egress cap — {name} {alt.VmSize} ({FmtDelta(delta)}) "
            + $"lifts the ceiling to {(alt.Confidence == "estimated" ? "~" : "")}{FmtMbps(alt.EgressMbps)} "
            + $"({(alt.Confidence == "documented" ? "doc" : "est")}){accel}.");
    }

    private static InfraSuggestion? Downsize(
        IReadOnlyList<DirectionAssessment> assessments, ReportInfraSide side, string name)
    {
        if (side.VmSize is null || side.Specs is null || side.HourlyUsd is null
            || side.Alternatives.Count == 0)
        {
            return null;
        }
        var carried = assessments
            .Where(a => name == "target" ? a.Direction == "download" : a.Direction == "upload")
            .ToList();
        if (carried.Count == 0
            || !carried.All(a => a.Utilization is < DownsizeMaxUtilization))
        {
            return null;
        }
        var needed = carried.Max(a => a.MeasuredMbps) * DownsizeHeadroomFactor;
        var alt = CheapestAbove(side.Alternatives, needed);
        if (alt?.HourlyUsd is not { } next || next >= side.HourlyUsd)
        {
            return null;
        }
        var pct = (int)Math.Round((side.HourlyUsd.Value - next) / side.HourlyUsd.Value * 100);
        return new InfraSuggestion("downsize",
            $"{name} has ample network headroom — {alt.VmSize} ({FmtDelta(next - side.HourlyUsd)}, −{pct}%) "
            + $"still clears the measured rate with {DownsizeHeadroomFactor.ToString("0.#", CultureInfo.InvariantCulture)}× margin.");
    }

    private static ReportAltSize? CheapestAbove(IReadOnlyList<ReportAltSize> alts, double minEgressMbps) =>
        alts.Where(a => a.EgressMbps >= minEgressMbps)
            .OrderBy(a => a.HourlyUsd ?? double.MaxValue)
            .ThenBy(a => a.EgressMbps)
            .FirstOrDefault();

    // ── formatting ──────────────────────────────────────────────────────────

    public static string FmtMbps(double mbps) => mbps >= 1000
        ? (mbps / 1000).ToString("0.#", CultureInfo.InvariantCulture) + " Gbps"
        : Math.Round(mbps).ToString("0", CultureInfo.InvariantCulture) + " Mbps";

    private static string FmtDelta(double? delta)
    {
        if (delta is not { } d)
        {
            return "price unknown";
        }
        var abs = Math.Abs(d).ToString("0.###", CultureInfo.InvariantCulture);
        return d >= 0 ? $"+${abs}/h" : $"−${abs}/h";
    }

    private static double? Percentile(IReadOnlyList<double> values, double q)
    {
        if (values.Count == 0)
        {
            return null;
        }
        var sorted = values.OrderBy(x => x).ToList();
        if (sorted.Count == 1)
        {
            return sorted[0];
        }
        var pos = q * (sorted.Count - 1);
        var lo = (int)Math.Floor(pos);
        var hi = (int)Math.Ceiling(pos);
        return sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo);
    }
}
