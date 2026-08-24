using System.Globalization;
using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Reports.Documents;

/// <summary>
/// Renders the URL comparison report (<see cref="ProbeComparisonReport"/>) as a
/// document: per mode, the crowns, the scoreboard, the head-to-head record and
/// the coverage that decides whether any of it may be believed. Pure — maps the
/// already-computed wire report and adds no new statistics.
///
/// <para>An exported comparison ALWAYS carries its methodology and its coverage.
/// A scoreboard that leaves the building without them is a claim nobody can
/// check, and a mode whose overlap was too thin to rank must say so in the
/// document exactly as it does on screen.</para>
/// </summary>
public static class ProbeComparisonReportDocument
{
    public static ReportDocument Build(ProbeComparisonReport report, string projectId)
    {
        var sections = new List<ReportSection>();

        if (report.Modes.Count == 0)
        {
            sections.Add(new ReportSection("Comparison", new ProseBlock(
                "No URL-probe data to compare in this window. Pick at least two URLs that "
                + "have been probed, or widen the window.")));
        }

        foreach (var mode in report.Modes)
        {
            var blocks = new List<ReportBlock>();

            // Coverage first, deliberately: it is the licence to read the rest.
            blocks.Add(new MetricsBlock(
                new Metric("Shared buckets", mode.SharedBuckets.ToString(CultureInfo.InvariantCulture)),
                new Metric("Window buckets", report.WindowBuckets.ToString(CultureInfo.InvariantCulture)),
                new Metric("Overlap", Ratio(mode.CoverageRatio),
                    mode.Ranked ? ReportTone.Good : ReportTone.Warn),
                new Metric("URLs compared",
                    mode.Coverage.Count(c => c.Eligible).ToString(CultureInfo.InvariantCulture))));

            if (!mode.Ranked)
            {
                blocks.Add(new CalloutBlock(NotRankedText(mode, report), ReportTone.Warn));
            }
            else
            {
                blocks.Add(new CalloutBlock(CrownText(mode.Crowns), ReportTone.Good));
            }

            if (mode.Scores.Count > 0)
            {
                blocks.Add(new TableBlock(
                    Headers: ["URL", "Buckets", "Samples", "p50", "p95", "Success", "Jitter", "DNS", "TCP", "TLS", "TTFB"],
                    Rows: [.. mode.Scores.Select(s => (IReadOnlyList<string>)new[]
                    {
                        s.Url,
                        s.SharedBuckets.ToString(CultureInfo.InvariantCulture),
                        s.Samples.ToString(CultureInfo.InvariantCulture),
                        Ms(s.MedianP50Ms),
                        Ms(s.MedianP95Ms),
                        Ratio(s.SuccessRate),
                        s.JitterRatio is { } j ? j.ToString("0.00", CultureInfo.InvariantCulture) + "x" : "—",
                        Ms(s.MedianDnsMs),
                        Ms(s.MedianTcpMs),
                        Ms(s.MedianTlsMs),
                        Ms(s.MedianTtfbMs),
                    })],
                    Aligns:
                    [
                        ColumnAlign.Left, ColumnAlign.Right, ColumnAlign.Right, ColumnAlign.Right,
                        ColumnAlign.Right, ColumnAlign.Right, ColumnAlign.Right, ColumnAlign.Right,
                        ColumnAlign.Right, ColumnAlign.Right, ColumnAlign.Right,
                    ]));
            }

            if (mode.HeadToHead.Count > 0)
            {
                blocks.Add(new TableBlock(
                    Headers: ["Matchup", "Raced buckets", "Wins", "Losses", "Ties", "Verdict"],
                    Rows: [.. mode.HeadToHead.Select(h => (IReadOnlyList<string>)new[]
                    {
                        $"{h.A} vs {h.B}",
                        h.Buckets.ToString(CultureInfo.InvariantCulture),
                        h.AWins.ToString(CultureInfo.InvariantCulture),
                        h.BWins.ToString(CultureInfo.InvariantCulture),
                        h.Ties.ToString(CultureInfo.InvariantCulture),
                        HeadToHeadVerdict(h),
                    })],
                    Aligns:
                    [
                        ColumnAlign.Left, ColumnAlign.Right, ColumnAlign.Right,
                        ColumnAlign.Right, ColumnAlign.Right, ColumnAlign.Left,
                    ]));
            }

            var excluded = mode.Coverage.Where(c => !c.Eligible).ToList();
            if (excluded.Count > 0)
            {
                blocks.Add(new CalloutBlock(
                    "Not compared — too little data in this window: "
                    + string.Join("; ", excluded.Select(c =>
                        $"{c.Url} ({c.QualifyingBuckets}/{report.WindowBuckets} buckets)")),
                    ReportTone.Neutral));
            }

            sections.Add(new ReportSection($"Mode: {mode.Mode}", blocks));
        }

        sections.Add(new ReportSection("How it's measured",
            new ProseBlock(report.Methodology.Buckets),
            new ProseBlock(report.Methodology.Shared),
            new ProseBlock(report.Methodology.Eligibility),
            new ProseBlock(report.Methodology.Ranking),
            new ProseBlock(report.Methodology.Crowns),
            new ProseBlock(report.Methodology.Modes)));

        return new ReportDocument(
            Title: "URL comparison",
            Subtitle: "Which URL is fastest, most reliable and most consistent — over the hours they were all measured",
            GeneratedAt: report.GeneratedAt,
            Meta:
            [
                new ReportMeta("Project", projectId),
                new ReportMeta("Window", report.Window),
                new ReportMeta("Bucket", report.Bucket),
                new ReportMeta("From", report.From.ToString("u", CultureInfo.InvariantCulture)),
                new ReportMeta("To", report.To.ToString("u", CultureInfo.InvariantCulture)),
            ],
            Sections: sections,
            FooterNote: $"Generated by {ReportBranding.ProductName}. Every figure is computed only over "
                + "time buckets in which every compared URL was measured.");
    }

    /// <summary>Why a mode carries no ranking, in the reader's terms.</summary>
    internal static string NotRankedText(ProbeComparisonMode mode, ProbeComparisonReport report) =>
        mode.RankingVerdict switch
        {
            ProbeComparisonLogic.ReasonNoData =>
                "No probe data for these URLs in this window — nothing to compare.",
            ProbeComparisonLogic.ReasonTooFewUrls =>
                "Fewer than two URLs have enough data in this window to be compared. "
                + "The others are listed below with what they do have.",
            _ =>
                $"Insufficient overlap: the compared URLs were measured together in only "
                + $"{mode.SharedBuckets} of {report.WindowBuckets} buckets ({Ratio(mode.CoverageRatio)}). "
                + "The numbers below are shown for reference and are NOT ranked — co-schedule "
                + "these URLs so they are probed on the same tick, and the comparison becomes fair.",
        };

    /// <summary>The crowns as one sentence; silent about a crown nobody won.</summary>
    internal static string CrownText(ProbeComparisonCrowns c)
    {
        var parts = new List<string>();
        if (c.Fastest is not null) parts.Add($"Fastest: {c.Fastest}");
        if (c.MostReliable is not null) parts.Add($"Most reliable: {c.MostReliable}");
        if (c.MostConsistent is not null) parts.Add($"Most consistent: {c.MostConsistent}");
        if (c.BestDns is not null) parts.Add($"Best DNS: {c.BestDns}");
        if (c.BestTcp is not null) parts.Add($"Best TCP: {c.BestTcp}");
        if (c.BestTls is not null) parts.Add($"Best TLS: {c.BestTls}");
        if (c.BestTtfb is not null) parts.Add($"Best TTFB: {c.BestTtfb}");
        return parts.Count == 0
            ? "No category had a single winner — every measured metric was a tie."
            : string.Join(" · ", parts);
    }

    internal static string HeadToHeadVerdict(ProbeComparisonHeadToHead h)
    {
        if (h.Buckets == 0) return "never raced";
        if (h.AWins == h.BWins) return "even";
        var (winner, wins) = h.AWins > h.BWins ? (h.A, h.AWins) : (h.B, h.BWins);
        return $"{winner} faster in {wins}/{h.Buckets}";
    }

    private static string Ms(double? v) =>
        v is { } d ? d.ToString(d >= 100 ? "0" : "0.0", CultureInfo.InvariantCulture) + " ms" : "—";

    private static string Ratio(double? fraction) =>
        fraction is { } f ? (f * 100).ToString("0.#", CultureInfo.InvariantCulture) + "%" : "—";
}
