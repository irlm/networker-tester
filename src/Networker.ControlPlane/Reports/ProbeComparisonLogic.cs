namespace Networker.ControlPlane.Reports;

/// <summary>
/// Pure computation for the <b>URL comparison report</b> (issue #782 P3) — which
/// of several probed URLs is fastest, most reliable and most consistent, and
/// how much the answer can be trusted. Kept free of I/O so the methodology is
/// unit-testable without a database.
///
/// <para><b>The methodology, and why it is not "aggregate each URL over the
/// window".</b> A raw window-wide aggregate is biased by the probing schedule:
/// a URL probed only overnight "wins" by dodging peak hours, and nothing in the
/// number says so. Everything here is therefore computed over <b>shared time
/// buckets</b> — buckets in which <i>every</i> compared URL actually has
/// measurements:</para>
/// <list type="number">
///   <item>A bucket <b>qualifies</b> for a URL when it holds at least
///     <c>minSamples</c> samples of that URL (<see cref="DefaultMinSamples"/>).
///     One sample is an anecdote; burst sampling (P2) makes 5 per probe the
///     norm on this surface.</item>
///   <item>A URL is <b>eligible</b> when its own qualifying buckets cover at
///     least <see cref="MinCoverageRatio"/> of the window. A URL that is barely
///     probed is excluded and shown greyed with its real count — never ranked,
///     and never allowed to shrink everyone else's shared set.</item>
///   <item>The <b>shared</b> set is the intersection of the eligible URLs'
///     qualifying buckets. Every headline number is computed over it and
///     nothing else.</item>
///   <item>The scoreboard is <b>ranked</b> only when the shared set itself
///     covers at least <see cref="MinCoverageRatio"/> of the window. Below that
///     the report says "insufficient overlap" and greys the ranking rather than
///     ranking anyway.</item>
/// </list>
///
/// <para><b>Ranking is head-to-head</b>, not "whose average is lower": for each
/// pair, the number of shared buckets in which one URL's p50 beat the other's.
/// "In the 42 hours both were probed, A was faster in 31" survives an outlier
/// that a mean does not, and is the claim the report actually makes.</para>
///
/// <para>Nothing here interpolates. A bucket a URL did not measure is absent,
/// not zero; a phase the protocol never performs (no TLS on plain HTTP, no DNS
/// on an IP literal) is null, and a null never becomes a winner.</para>
/// </summary>
public static class ProbeComparisonLogic
{
    /// <summary>Samples a bucket needs before it counts as measured for a URL.</summary>
    public const int DefaultMinSamples = 3;

    /// <summary>
    /// Coverage floor, as a fraction of the window's buckets — used twice: a URL
    /// below it is excluded from the comparison, and a shared set below it is
    /// reported but not ranked. ~30% per issue #782.
    /// </summary>
    public const double MinCoverageRatio = 0.30;

    /// <summary>Fewest shared buckets that may carry a ranking at all, however
    /// short the window. Two buckets can agree by chance; this is the floor
    /// under which "A won 2 of 2" is not a finding.</summary>
    public const int MinSharedBuckets = 5;

    public const string ReasonRanked = "ranked";
    public const string ReasonInsufficientOverlap = "insufficient_overlap";
    public const string ReasonTooFewUrls = "too_few_urls";
    public const string ReasonNoData = "no_data";

    public const string ExcludedUnderSampled = "under_sampled";

    /// <summary>One URL's measurements in one bucket, as the SQL layer supplies
    /// them. Percentiles are null when the URL had no SUCCESSFUL sample in the
    /// bucket (a bucket of pure failures still counts for reliability).</summary>
    public sealed record BucketPoint(
        string Url,
        DateTime Bucket,
        int SampleCount,
        int SuccessCount,
        double? P50TotalMs,
        double? P95TotalMs,
        double? P50DnsMs,
        double? P50TcpMs,
        double? P50TlsMs,
        double? P50TtfbMs,
        string? DominantErrorCategory);

    /// <summary>How much of the window one URL actually measured.</summary>
    public sealed record UrlCoverage(
        string Url,
        int QualifyingBuckets,
        int TotalSamples,
        bool Eligible,
        string? ExcludedReason);

    /// <summary>A URL's headline numbers, computed over the SHARED buckets only.</summary>
    public sealed record UrlScore(
        string Url,
        int SharedBuckets,
        int Samples,
        double? MedianP50Ms,
        double? MedianP95Ms,
        double? SuccessRate,
        double? JitterRatio,
        double? MedianDnsMs,
        double? MedianTcpMs,
        double? MedianTlsMs,
        double? MedianTtfbMs,
        string? DominantErrorCategory);

    /// <summary>Pairwise result over the shared buckets: how often each side's
    /// p50 was the lower one.</summary>
    public sealed record HeadToHead(string A, string B, int Buckets, int AWins, int BWins, int Ties);

    /// <summary>Round to 4dp, preserving null. Report numbers only.</summary>
    public static double? Round4(double? v) => v is null ? null : Math.Round(v.Value, 4);

    /// <summary>
    /// Buckets in which <paramref name="url"/> has at least
    /// <paramref name="minSamples"/> samples.
    /// </summary>
    public static HashSet<DateTime> QualifyingBuckets(
        IEnumerable<BucketPoint> points, string url, int minSamples) =>
        points.Where(p => p.Url == url && p.SampleCount >= minSamples)
              .Select(p => p.Bucket)
              .ToHashSet();

    /// <summary>
    /// Per-URL coverage, and which URLs are eligible to be compared at all.
    /// A URL is excluded when its own qualifying buckets cover less than
    /// <see cref="MinCoverageRatio"/> of the window — it is shown with its real
    /// numbers and never ranked, so one barely-probed URL cannot shrink the
    /// shared set for everybody else.
    /// </summary>
    public static List<UrlCoverage> Coverage(
        IReadOnlyList<BucketPoint> points, IReadOnlyList<string> urls, int windowBuckets, int minSamples)
    {
        var result = new List<UrlCoverage>();
        foreach (var url in urls)
        {
            var qualifying = QualifyingBuckets(points, url, minSamples);
            var samples = points.Where(p => p.Url == url).Sum(p => p.SampleCount);
            var eligible = windowBuckets > 0
                           && qualifying.Count >= Math.Ceiling(MinCoverageRatio * windowBuckets);
            result.Add(new UrlCoverage(
                Url: url,
                QualifyingBuckets: qualifying.Count,
                TotalSamples: samples,
                Eligible: eligible,
                ExcludedReason: eligible ? null : ExcludedUnderSampled));
        }

        return result;
    }

    /// <summary>
    /// The buckets every eligible URL measured — the only buckets any headline
    /// number is computed over. Empty when fewer than two URLs are eligible.
    /// </summary>
    public static HashSet<DateTime> SharedBuckets(
        IReadOnlyList<BucketPoint> points, IReadOnlyList<UrlCoverage> coverage, int minSamples)
    {
        var eligible = coverage.Where(c => c.Eligible).Select(c => c.Url).ToList();
        if (eligible.Count < 2) return [];

        HashSet<DateTime>? shared = null;
        foreach (var url in eligible)
        {
            var buckets = QualifyingBuckets(points, url, minSamples);
            if (shared is null) shared = buckets;
            else shared.IntersectWith(buckets);
            if (shared.Count == 0) break;
        }

        return shared ?? [];
    }

    /// <summary>
    /// Whether the shared set can carry a ranking, and why not when it cannot.
    /// Returns <see cref="ReasonRanked"/> only when at least two URLs are
    /// eligible AND the shared set clears both the absolute floor
    /// (<see cref="MinSharedBuckets"/>) and <see cref="MinCoverageRatio"/> of
    /// the window.
    /// </summary>
    public static string RankingVerdict(
        IReadOnlyList<UrlCoverage> coverage, int sharedBuckets, int windowBuckets)
    {
        if (coverage.Count == 0 || coverage.All(c => c.TotalSamples == 0)) return ReasonNoData;
        if (coverage.Count(c => c.Eligible) < 2) return ReasonTooFewUrls;
        if (sharedBuckets < MinSharedBuckets) return ReasonInsufficientOverlap;
        if (windowBuckets <= 0) return ReasonInsufficientOverlap;
        return sharedBuckets >= MinCoverageRatio * windowBuckets
            ? ReasonRanked
            : ReasonInsufficientOverlap;
    }

    /// <summary>Coverage as a fraction of the window's buckets; 0 when the
    /// window holds no buckets.</summary>
    public static double CoverageRatio(int sharedBuckets, int windowBuckets) =>
        windowBuckets <= 0 ? 0.0 : Math.Round((double)sharedBuckets / windowBuckets, 4);

    /// <summary>
    /// Median of a sample, by the same linear-interpolation definition
    /// PostgreSQL's <c>PERCENTILE_CONT(0.5)</c> uses, so a median computed here
    /// over per-bucket values and one computed in SQL over raw samples mean the
    /// same thing. Nulls are dropped, never treated as zero; null when nothing
    /// is left.
    /// </summary>
    public static double? Median(IEnumerable<double?> values)
    {
        var present = values.Where(v => v.HasValue).Select(v => v!.Value).OrderBy(v => v).ToList();
        if (present.Count == 0) return null;
        var pos = 0.5 * (present.Count - 1);
        var lo = (int)Math.Floor(pos);
        var hi = (int)Math.Ceiling(pos);
        return lo == hi ? present[lo] : present[lo] + ((present[hi] - present[lo]) * (pos - lo));
    }

    /// <summary>
    /// Spread relative to the typical value: <c>p95 / p50</c>. 1.0 is a URL that
    /// always responds in the same time; the higher it climbs the less the
    /// median predicts any single request. Null when either side is missing or
    /// the median is zero (nothing to be relative to).
    /// </summary>
    public static double? JitterRatio(double? p50, double? p95) =>
        p50 is null or 0 || p95 is null ? null : Math.Round(p95.Value / p50.Value, 4);

    /// <summary>
    /// One URL's headline numbers over the shared buckets. Every figure is a
    /// median of that URL's PER-BUCKET values, so each bucket weighs the same
    /// however many times the URL happened to be probed inside it — a URL
    /// probed twice an hour must not outvote one probed once.
    /// </summary>
    public static UrlScore Score(
        IReadOnlyList<BucketPoint> points, string url, IReadOnlySet<DateTime> shared)
    {
        var mine = points.Where(p => p.Url == url && shared.Contains(p.Bucket)).ToList();

        var samples = mine.Sum(p => p.SampleCount);
        var successes = mine.Sum(p => p.SuccessCount);
        var p50 = Median(mine.Select(p => p.P50TotalMs));
        var p95 = Median(mine.Select(p => p.P95TotalMs));

        // Dominant error category across the shared buckets: the one named by
        // the most buckets. Ties break on the name so the answer is stable.
        var dominant = mine
            .Where(p => p.DominantErrorCategory is not null)
            .GroupBy(p => p.DominantErrorCategory!)
            .OrderByDescending(g => g.Count())
            .ThenBy(g => g.Key, StringComparer.Ordinal)
            .Select(g => g.Key)
            .FirstOrDefault();

        return new UrlScore(
            Url: url,
            SharedBuckets: mine.Count,
            Samples: samples,
            MedianP50Ms: Round4(p50),
            MedianP95Ms: Round4(p95),
            SuccessRate: samples == 0 ? null : Math.Round((double)successes / samples, 4),
            JitterRatio: JitterRatio(p50, p95),
            MedianDnsMs: Round4(Median(mine.Select(p => p.P50DnsMs))),
            MedianTcpMs: Round4(Median(mine.Select(p => p.P50TcpMs))),
            MedianTlsMs: Round4(Median(mine.Select(p => p.P50TlsMs))),
            MedianTtfbMs: Round4(Median(mine.Select(p => p.P50TtfbMs))),
            DominantErrorCategory: dominant);
    }

    /// <summary>
    /// Head-to-head over the shared buckets: in how many of them each side's
    /// p50 was the lower one. A bucket where either side has no successful
    /// sample is not a race and is counted in neither column nor as a tie.
    /// </summary>
    public static HeadToHead Compare(
        IReadOnlyList<BucketPoint> points, string a, string b, IReadOnlySet<DateTime> shared)
    {
        var left = points.Where(p => p.Url == a && shared.Contains(p.Bucket))
                         .ToDictionary(p => p.Bucket, p => p.P50TotalMs);
        var right = points.Where(p => p.Url == b && shared.Contains(p.Bucket))
                          .ToDictionary(p => p.Bucket, p => p.P50TotalMs);

        int aWins = 0, bWins = 0, ties = 0, raced = 0;
        foreach (var bucket in shared)
        {
            if (!left.TryGetValue(bucket, out var lv) || lv is null) continue;
            if (!right.TryGetValue(bucket, out var rv) || rv is null) continue;
            raced++;
            if (lv.Value < rv.Value) aWins++;
            else if (rv.Value < lv.Value) bWins++;
            else ties++;
        }

        return new HeadToHead(a, b, raced, aWins, bWins, ties);
    }

    /// <summary>
    /// Every unordered pair of eligible URLs, in a stable order.
    /// </summary>
    public static List<HeadToHead> AllPairs(
        IReadOnlyList<BucketPoint> points, IReadOnlyList<string> eligibleUrls, IReadOnlySet<DateTime> shared)
    {
        var pairs = new List<HeadToHead>();
        for (var i = 0; i < eligibleUrls.Count; i++)
        {
            for (var j = i + 1; j < eligibleUrls.Count; j++)
            {
                pairs.Add(Compare(points, eligibleUrls[i], eligibleUrls[j], shared));
            }
        }

        return pairs;
    }

    /// <summary>
    /// The URL with the lowest value of <paramref name="metric"/>, or null when
    /// no candidate has one. A null metric never wins: "no TLS measured" is not
    /// "the fastest TLS". Null when the best value is shared by more than one
    /// URL — a crown implies a winner, and a tie has none.
    /// </summary>
    public static string? Lowest(IEnumerable<UrlScore> scores, Func<UrlScore, double?> metric)
    {
        var ranked = scores.Where(s => metric(s).HasValue)
                           .OrderBy(s => metric(s)!.Value)
                           .ThenBy(s => s.Url, StringComparer.Ordinal)
                           .ToList();
        if (ranked.Count == 0) return null;
        if (ranked.Count > 1 && metric(ranked[0])!.Value == metric(ranked[1])!.Value) return null;
        return ranked[0].Url;
    }

    /// <summary>
    /// The URL with the highest value of <paramref name="metric"/> — the
    /// reliability crown. Same null and tie rules as <see cref="Lowest"/>.
    /// </summary>
    public static string? Highest(IEnumerable<UrlScore> scores, Func<UrlScore, double?> metric)
    {
        var ranked = scores.Where(s => metric(s).HasValue)
                           .OrderByDescending(s => metric(s)!.Value)
                           .ThenBy(s => s.Url, StringComparer.Ordinal)
                           .ToList();
        if (ranked.Count == 0) return null;
        if (ranked.Count > 1 && metric(ranked[0])!.Value == metric(ranked[1])!.Value) return null;
        return ranked[0].Url;
    }
}
