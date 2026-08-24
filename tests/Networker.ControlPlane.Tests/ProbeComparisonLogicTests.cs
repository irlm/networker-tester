using Networker.ControlPlane.Reports;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// The comparison methodology for the URL comparison report (#782 P3). This is
/// the whole product claim — "A is faster than B" — so the honesty rules are
/// pinned here with hand-picked numbers, DB-free: shared-bucket intersection,
/// under-sampled exclusion, the coverage floor that greys a ranking, and the
/// tie/null rules that decide when NO crown is awarded.
/// </summary>
public sealed class ProbeComparisonLogicTests
{
    private static readonly DateTime T0 = new(2026, 8, 20, 0, 0, 0, DateTimeKind.Utc);

    private static ProbeComparisonLogic.BucketPoint Point(
        string url, int hour, int samples = 5, int successes = 5,
        double? p50 = 100.0, double? p95 = 150.0,
        double? dns = null, double? tcp = null, double? tls = null, double? ttfb = null,
        string? error = null) =>
        new(url, T0.AddHours(hour), samples, successes, p50, p95, dns, tcp, tls, ttfb, error);

    // ── qualifying buckets ──────────────────────────────────────────────────

    [Fact]
    public void A_bucket_qualifies_only_at_or_above_min_samples()
    {
        List<ProbeComparisonLogic.BucketPoint> points =
        [
            Point("a", 0, samples: 5),
            Point("a", 1, samples: 3),  // exactly the floor — counts
            Point("a", 2, samples: 2),  // below — does not
        ];

        var qualifying = ProbeComparisonLogic.QualifyingBuckets(points, "a", minSamples: 3);

        Assert.Equal(2, qualifying.Count);
        Assert.Contains(T0.AddHours(0), qualifying);
        Assert.Contains(T0.AddHours(1), qualifying);
        Assert.DoesNotContain(T0.AddHours(2), qualifying);
    }

    // ── eligibility: an under-sampled URL is excluded, not ranked ────────────

    [Fact]
    public void A_url_below_the_coverage_floor_is_excluded_and_named()
    {
        // 10-bucket window: the floor is ceil(0.30 * 10) = 3 buckets.
        List<ProbeComparisonLogic.BucketPoint> points =
        [
            .. Enumerable.Range(0, 6).Select(h => Point("a", h)),
            .. Enumerable.Range(0, 3).Select(h => Point("b", h)),   // exactly the floor
            Point("c", 0),                                          // 1 bucket — excluded
        ];

        var coverage = ProbeComparisonLogic.Coverage(points, ["a", "b", "c"], windowBuckets: 10, minSamples: 3);

        Assert.True(coverage.Single(c => c.Url == "a").Eligible);
        Assert.True(coverage.Single(c => c.Url == "b").Eligible);

        var c3 = coverage.Single(c => c.Url == "c");
        Assert.False(c3.Eligible);
        Assert.Equal(ProbeComparisonLogic.ExcludedUnderSampled, c3.ExcludedReason);
        Assert.Equal(1, c3.QualifyingBuckets);
        Assert.Equal(5, c3.TotalSamples);
    }

    [Fact]
    public void An_excluded_url_does_not_shrink_the_shared_set_for_the_others()
    {
        // a and b overlap on 6 buckets; c has one lonely bucket at hour 9.
        // If c were allowed into the intersection the shared set would be EMPTY
        // and nothing could be compared — the whole point of excluding it.
        List<ProbeComparisonLogic.BucketPoint> points =
        [
            .. Enumerable.Range(0, 6).Select(h => Point("a", h)),
            .. Enumerable.Range(0, 6).Select(h => Point("b", h)),
            Point("c", 9),
        ];

        var coverage = ProbeComparisonLogic.Coverage(points, ["a", "b", "c"], windowBuckets: 10, minSamples: 3);
        var shared = ProbeComparisonLogic.SharedBuckets(points, coverage, minSamples: 3);

        Assert.Equal(6, shared.Count);
    }

    // ── shared buckets are an intersection, never a union ────────────────────

    [Fact]
    public void Shared_buckets_are_the_intersection_of_every_eligible_url()
    {
        // a: 0-5, b: 3-8 → shared 3,4,5
        List<ProbeComparisonLogic.BucketPoint> points =
        [
            .. Enumerable.Range(0, 6).Select(h => Point("a", h)),
            .. Enumerable.Range(3, 6).Select(h => Point("b", h)),
        ];

        var coverage = ProbeComparisonLogic.Coverage(points, ["a", "b"], windowBuckets: 10, minSamples: 3);
        var shared = ProbeComparisonLogic.SharedBuckets(points, coverage, minSamples: 3);

        Assert.Equal([T0.AddHours(3), T0.AddHours(4), T0.AddHours(5)], shared.OrderBy(b => b));
    }

    [Fact]
    public void Fewer_than_two_eligible_urls_share_nothing()
    {
        List<ProbeComparisonLogic.BucketPoint> points = [.. Enumerable.Range(0, 6).Select(h => Point("a", h))];

        var coverage = ProbeComparisonLogic.Coverage(points, ["a"], windowBuckets: 10, minSamples: 3);

        Assert.Empty(ProbeComparisonLogic.SharedBuckets(points, coverage, minSamples: 3));
    }

    // ── the ranking verdict: when the report refuses to rank ─────────────────

    [Fact]
    public void No_data_at_all_is_reported_as_no_data()
    {
        var coverage = ProbeComparisonLogic.Coverage([], ["a", "b"], windowBuckets: 10, minSamples: 3);

        Assert.Equal(ProbeComparisonLogic.ReasonNoData,
            ProbeComparisonLogic.RankingVerdict(coverage, sharedBuckets: 0, windowBuckets: 10));
    }

    [Fact]
    public void One_eligible_url_is_reported_as_too_few_urls()
    {
        List<ProbeComparisonLogic.BucketPoint> points =
        [
            .. Enumerable.Range(0, 6).Select(h => Point("a", h)),
            Point("b", 0),
        ];
        var coverage = ProbeComparisonLogic.Coverage(points, ["a", "b"], windowBuckets: 10, minSamples: 3);

        Assert.Equal(ProbeComparisonLogic.ReasonTooFewUrls,
            ProbeComparisonLogic.RankingVerdict(coverage, sharedBuckets: 0, windowBuckets: 10));
    }

    [Fact]
    public void Overlap_below_thirty_percent_of_the_window_is_not_ranked()
    {
        // 168-bucket window (7 days hourly), 42 shared → 25%: the issue's own
        // example of a ranking that must be greyed rather than asserted.
        List<ProbeComparisonLogic.BucketPoint> points =
        [
            .. Enumerable.Range(0, 60).Select(h => Point("a", h)),
            .. Enumerable.Range(0, 60).Select(h => Point("b", h)),
        ];
        var coverage = ProbeComparisonLogic.Coverage(points, ["a", "b"], windowBuckets: 168, minSamples: 3);

        Assert.Equal(ProbeComparisonLogic.ReasonInsufficientOverlap,
            ProbeComparisonLogic.RankingVerdict(coverage, sharedBuckets: 42, windowBuckets: 168));
    }

    [Fact]
    public void Overlap_at_exactly_thirty_percent_is_ranked()
    {
        List<ProbeComparisonLogic.BucketPoint> points =
        [
            .. Enumerable.Range(0, 60).Select(h => Point("a", h)),
            .. Enumerable.Range(0, 60).Select(h => Point("b", h)),
        ];
        var coverage = ProbeComparisonLogic.Coverage(points, ["a", "b"], windowBuckets: 100, minSamples: 3);

        Assert.Equal(ProbeComparisonLogic.ReasonRanked,
            ProbeComparisonLogic.RankingVerdict(coverage, sharedBuckets: 30, windowBuckets: 100));
    }

    [Fact]
    public void A_tiny_window_still_needs_the_absolute_floor_of_shared_buckets()
    {
        // 4 shared of a 4-bucket window is 100% coverage — and still only four
        // buckets, which is not a finding.
        List<ProbeComparisonLogic.BucketPoint> points =
        [
            .. Enumerable.Range(0, 4).Select(h => Point("a", h)),
            .. Enumerable.Range(0, 4).Select(h => Point("b", h)),
        ];
        var coverage = ProbeComparisonLogic.Coverage(points, ["a", "b"], windowBuckets: 4, minSamples: 3);

        Assert.Equal(ProbeComparisonLogic.ReasonInsufficientOverlap,
            ProbeComparisonLogic.RankingVerdict(coverage, sharedBuckets: 4, windowBuckets: 4));
    }

    [Theory]
    [InlineData(42, 168, 0.25)]
    [InlineData(30, 100, 0.3)]
    [InlineData(0, 0, 0.0)]     // empty window never divides by zero
    public void Coverage_ratio_is_shared_over_the_windows_buckets(int shared, int window, double expected)
    {
        Assert.Equal(expected, ProbeComparisonLogic.CoverageRatio(shared, window));
    }

    // ── median: PERCENTILE_CONT semantics, nulls dropped not zeroed ──────────

    [Fact]
    public void Median_interpolates_like_percentile_cont_and_drops_nulls()
    {
        Assert.Equal(2.0, ProbeComparisonLogic.Median([1.0, 2.0, 3.0]));
        Assert.Equal(2.5, ProbeComparisonLogic.Median([1.0, 2.0, 3.0, 4.0]));
        // A missing measurement must not be read as a fast zero.
        Assert.Equal(3.0, ProbeComparisonLogic.Median([null, 3.0, null]));
        Assert.Null(ProbeComparisonLogic.Median([null, null]));
        Assert.Null(ProbeComparisonLogic.Median([]));
    }

    [Theory]
    [InlineData(100.0, 150.0, 1.5)]
    [InlineData(100.0, 100.0, 1.0)]   // perfectly consistent
    [InlineData(null, 150.0, null)]
    [InlineData(100.0, null, null)]
    [InlineData(0.0, 150.0, null)]    // nothing to be relative to
    public void Jitter_ratio_is_p95_over_p50(double? p50, double? p95, double? expected)
    {
        Assert.Equal(expected, ProbeComparisonLogic.JitterRatio(p50, p95));
    }

    // ── scoring over shared buckets only ─────────────────────────────────────

    [Fact]
    public void A_score_ignores_buckets_outside_the_shared_set()
    {
        // The 2000 ms bucket at hour 9 is not shared. Including it would move
        // the median; the whole methodology is that it must not.
        List<ProbeComparisonLogic.BucketPoint> points =
        [
            Point("a", 0, p50: 100.0),
            Point("a", 1, p50: 100.0),
            Point("a", 9, p50: 2000.0),
        ];
        var shared = new HashSet<DateTime> { T0.AddHours(0), T0.AddHours(1) };

        var score = ProbeComparisonLogic.Score(points, "a", shared);

        Assert.Equal(2, score.SharedBuckets);
        Assert.Equal(100.0, score.MedianP50Ms);
        Assert.Equal(10, score.Samples);
    }

    [Fact]
    public void Every_bucket_weighs_the_same_however_often_it_was_probed()
    {
        // Hour 0 holds 50 samples, hour 1 holds 5. A sample-weighted mean would
        // be dragged to ~110; the per-bucket median is 300 either way.
        List<ProbeComparisonLogic.BucketPoint> points =
        [
            Point("a", 0, samples: 50, successes: 50, p50: 100.0),
            Point("a", 1, samples: 5, successes: 5, p50: 500.0),
        ];
        var shared = new HashSet<DateTime> { T0.AddHours(0), T0.AddHours(1) };

        Assert.Equal(300.0, ProbeComparisonLogic.Score(points, "a", shared).MedianP50Ms);
    }

    [Fact]
    public void Success_rate_counts_samples_and_the_dominant_error_wins_on_bucket_count()
    {
        List<ProbeComparisonLogic.BucketPoint> points =
        [
            Point("a", 0, samples: 5, successes: 4, error: "timeout"),
            Point("a", 1, samples: 5, successes: 5, error: null),
            Point("a", 2, samples: 5, successes: 3, error: "timeout"),
            Point("a", 3, samples: 5, successes: 4, error: "dns"),
        ];
        var shared = new HashSet<DateTime>
            { T0.AddHours(0), T0.AddHours(1), T0.AddHours(2), T0.AddHours(3) };

        var score = ProbeComparisonLogic.Score(points, "a", shared);

        Assert.Equal(0.8, score.SuccessRate);           // 16 of 20
        Assert.Equal("timeout", score.DominantErrorCategory);
    }

    // ── head-to-head ────────────────────────────────────────────────────────

    [Fact]
    public void Head_to_head_counts_the_buckets_each_side_won()
    {
        List<ProbeComparisonLogic.BucketPoint> points =
        [
            Point("a", 0, p50: 100.0), Point("b", 0, p50: 200.0),  // a
            Point("a", 1, p50: 300.0), Point("b", 1, p50: 200.0),  // b
            Point("a", 2, p50: 100.0), Point("b", 2, p50: 100.0),  // tie
        ];
        var shared = new HashSet<DateTime> { T0.AddHours(0), T0.AddHours(1), T0.AddHours(2) };

        var h = ProbeComparisonLogic.Compare(points, "a", "b", shared);

        Assert.Equal(3, h.Buckets);
        Assert.Equal(1, h.AWins);
        Assert.Equal(1, h.BWins);
        Assert.Equal(1, h.Ties);
    }

    [Fact]
    public void A_bucket_only_one_side_measured_successfully_is_not_a_race()
    {
        // b had no successful sample in hour 1 — that is not a win for a.
        List<ProbeComparisonLogic.BucketPoint> points =
        [
            Point("a", 0, p50: 100.0), Point("b", 0, p50: 200.0),
            Point("a", 1, p50: 100.0), Point("b", 1, samples: 5, successes: 0, p50: null),
        ];
        var shared = new HashSet<DateTime> { T0.AddHours(0), T0.AddHours(1) };

        var h = ProbeComparisonLogic.Compare(points, "a", "b", shared);

        Assert.Equal(1, h.Buckets);
        Assert.Equal(1, h.AWins);
        Assert.Equal(0, h.BWins);
        Assert.Equal(0, h.Ties);
    }

    [Fact]
    public void All_pairs_covers_every_unordered_pair_once()
    {
        List<ProbeComparisonLogic.BucketPoint> points =
        [
            Point("a", 0, p50: 100.0), Point("b", 0, p50: 200.0), Point("c", 0, p50: 300.0),
        ];
        var shared = new HashSet<DateTime> { T0 };

        var pairs = ProbeComparisonLogic.AllPairs(points, ["a", "b", "c"], shared);

        Assert.Equal(3, pairs.Count);
        Assert.Contains(pairs, p => p is { A: "a", B: "b" });
        Assert.Contains(pairs, p => p is { A: "a", B: "c" });
        Assert.Contains(pairs, p => p is { A: "b", B: "c" });
    }

    // ── crowns: ties and nulls award nothing ────────────────────────────────

    private static ProbeComparisonLogic.UrlScore ScoreOf(
        string url, double? p50 = null, double? success = null, double? jitter = null, double? tls = null) =>
        new(url, 10, 50, p50, null, success, jitter, null, null, tls, null, null);

    [Fact]
    public void The_lowest_metric_takes_the_crown()
    {
        Assert.Equal("b", ProbeComparisonLogic.Lowest(
            [ScoreOf("a", p50: 200.0), ScoreOf("b", p50: 100.0)], s => s.MedianP50Ms));
    }

    [Fact]
    public void A_tie_awards_no_crown()
    {
        Assert.Null(ProbeComparisonLogic.Lowest(
            [ScoreOf("a", p50: 100.0), ScoreOf("b", p50: 100.0)], s => s.MedianP50Ms));
    }

    [Fact]
    public void A_phase_nobody_measured_awards_no_crown()
    {
        // Plain HTTP performs no TLS handshake: "no TLS" is not "the best TLS".
        Assert.Null(ProbeComparisonLogic.Lowest(
            [ScoreOf("a"), ScoreOf("b")], s => s.MedianTlsMs));
    }

    [Fact]
    public void Only_urls_that_measured_the_phase_compete_for_its_crown()
    {
        Assert.Equal("b", ProbeComparisonLogic.Lowest(
            [ScoreOf("a", tls: null), ScoreOf("b", tls: 40.0)], s => s.MedianTlsMs));
    }

    [Fact]
    public void Reliability_takes_the_highest_success_rate()
    {
        Assert.Equal("a", ProbeComparisonLogic.Highest(
            [ScoreOf("a", success: 0.99), ScoreOf("b", success: 0.90)], s => s.SuccessRate));
        Assert.Null(ProbeComparisonLogic.Highest(
            [ScoreOf("a", success: 0.99), ScoreOf("b", success: 0.99)], s => s.SuccessRate));
    }

    [Fact]
    public void Consistency_takes_the_lowest_jitter_not_the_lowest_latency()
    {
        // b is slower but far steadier — the crowns are deliberately different
        // questions, and this is the case that proves they are.
        var scores = new List<ProbeComparisonLogic.UrlScore>
        {
            ScoreOf("a", p50: 100.0, jitter: 4.0),
            ScoreOf("b", p50: 300.0, jitter: 1.1),
        };

        Assert.Equal("a", ProbeComparisonLogic.Lowest(scores, s => s.MedianP50Ms));
        Assert.Equal("b", ProbeComparisonLogic.Lowest(scores, s => s.JitterRatio));
    }
}
