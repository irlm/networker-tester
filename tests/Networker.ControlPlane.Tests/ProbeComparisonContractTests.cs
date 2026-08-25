using System.Text.Json;
using System.Text.Json.Nodes;
using Networker.ControlPlane.Endpoints;
using Networker.ControlPlane.Reports;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pins the wire shape of
/// <c>GET /api/projects/{id}/reports/probe-comparison</c> — the snake_case field
/// set the compare page consumes, the embedded <c>methodology</c> block, and the
/// nullable crowns. Also pins the query-parameter vocabulary (bucket widths and
/// window lengths), because the page builds its selectors from the same names.
/// </summary>
public sealed class ProbeComparisonContractTests
{
    private static readonly JsonSerializerOptions WebOptions = new(JsonSerializerDefaults.Web);
    private static readonly DateTime T0 = new(2026, 8, 20, 0, 0, 0, DateTimeKind.Utc);

    private static ProbeComparisonReport SampleReport() => new(
        GeneratedAt: T0,
        From: T0.AddDays(-7),
        To: T0,
        Window: "7d",
        Bucket: "1h",
        BucketSeconds: 3600,
        WindowBuckets: 168,
        MinSamples: 3,
        MinCoverageRatio: 0.30,
        Methodology: new ProbeComparisonMethodology(
            "bucket rule", "shared rule", "eligibility rule", "ranking rule", "crown rule", "mode rule"),
        Available:
        [
            new ProbeComparisonAvailable("https://a.example/", 840, 2, T0),
        ],
        // Hidden URLs that still have data — the picker offers them back rather
        // than pretending they never existed.
        Hidden: [],
        Modes:
        [
            new ProbeComparisonMode(
                Mode: "http2",
                Ranked: true,
                RankingVerdict: ProbeComparisonLogic.ReasonRanked,
                SharedBuckets: 60,
                CoverageRatio: 0.3571,
                Coverage:
                [
                    new ProbeComparisonCoverage("https://a.example/", 90, 450, true, null),
                    new ProbeComparisonCoverage("https://c.example/", 4, 20, false,
                        ProbeComparisonLogic.ExcludedUnderSampled),
                ],
                Scores:
                [
                    new ProbeComparisonScore(
                        "https://a.example/", 60, 300, 101.5, 180.0, 0.995, 1.7734,
                        12.0, 21.0, 33.0, 88.0, "timeout"),
                ],
                HeadToHead:
                [
                    new ProbeComparisonHeadToHead("https://a.example/", "https://b.example/", 60, 41, 18, 1),
                ],
                Crowns: new ProbeComparisonCrowns(
                    "https://a.example/", "https://a.example/", "https://b.example/",
                    "https://a.example/", null, null, "https://b.example/"),
                Series:
                [
                    new ProbeComparisonPoint(
                        "https://a.example/", T0, 5, 5, true, 101.5, 180.0, 12.0, 21.0, 33.0, 88.0, null),
                ]),
        ]);

    private static JsonNode Serialize() =>
        JsonNode.Parse(JsonSerializer.Serialize(SampleReport(), WebOptions))!;

    [Fact]
    public void Report_envelope_carries_the_window_and_the_methodology()
    {
        var json = Serialize();

        Assert.Equal("7d", (string?)json["window"]);
        Assert.Equal("1h", (string?)json["bucket"]);
        Assert.Equal(3600, (int?)json["bucket_seconds"]);
        Assert.Equal(168, (int?)json["window_buckets"]);
        Assert.Equal(3, (int?)json["min_samples"]);
        Assert.Equal(0.30, (double?)json["min_coverage_ratio"]);
        Assert.NotNull(json["generated_at"]);
        Assert.NotNull(json["from"]);
        Assert.NotNull(json["to"]);

        // The methodology travels WITH the numbers: a comparison exported to
        // PDF has to carry the rules it was computed under.
        var m = json["methodology"]!;
        foreach (var key in new[] { "buckets", "shared", "eligibility", "ranking", "crowns", "modes" })
        {
            Assert.False(string.IsNullOrWhiteSpace((string?)m[key]), $"methodology.{key} must be populated");
        }
    }

    [Fact]
    public void Available_lists_the_pickers_source_of_truth()
    {
        var a = Serialize()["available"]!.AsArray()[0]!;

        Assert.Equal("https://a.example/", (string?)a["url"]);
        Assert.Equal(840, (int?)a["sample_count"]);
        Assert.Equal(2, (int?)a["mode_count"]);
        Assert.NotNull(a["last_seen"]);
    }

    [Fact]
    public void A_mode_carries_its_own_ranking_verdict_and_coverage()
    {
        var mode = Serialize()["modes"]!.AsArray()[0]!;

        Assert.Equal("http2", (string?)mode["mode"]);
        Assert.True((bool?)mode["ranked"]);
        Assert.Equal("ranked", (string?)mode["ranking_verdict"]);
        Assert.Equal(60, (int?)mode["shared_buckets"]);
        Assert.Equal(0.3571, (double?)mode["coverage_ratio"]);
    }

    [Fact]
    public void An_excluded_url_keeps_its_real_numbers_and_says_why()
    {
        var excluded = Serialize()["modes"]![0]!["coverage"]!.AsArray()
            .First(c => (bool?)c!["eligible"] == false)!;

        Assert.Equal("https://c.example/", (string?)excluded["url"]);
        Assert.Equal(4, (int?)excluded["qualifying_buckets"]);
        Assert.Equal(20, (int?)excluded["total_samples"]);
        Assert.Equal("under_sampled", (string?)excluded["excluded_reason"]);
    }

    [Fact]
    public void A_score_carries_every_headline_and_phase_figure()
    {
        var s = Serialize()["modes"]![0]!["scores"]!.AsArray()[0]!;

        Assert.Equal("https://a.example/", (string?)s["url"]);
        Assert.Equal(60, (int?)s["shared_buckets"]);
        Assert.Equal(300, (int?)s["samples"]);
        Assert.Equal(101.5, (double?)s["median_p50_ms"]);
        Assert.Equal(180.0, (double?)s["median_p95_ms"]);
        Assert.Equal(0.995, (double?)s["success_rate"]);
        Assert.Equal(1.7734, (double?)s["jitter_ratio"]);
        Assert.Equal(12.0, (double?)s["median_dns_ms"]);
        Assert.Equal(21.0, (double?)s["median_tcp_ms"]);
        Assert.Equal(33.0, (double?)s["median_tls_ms"]);
        Assert.Equal(88.0, (double?)s["median_ttfb_ms"]);
        Assert.Equal("timeout", (string?)s["dominant_error_category"]);
    }

    [Fact]
    public void Head_to_head_is_the_claim_the_report_makes()
    {
        var h = Serialize()["modes"]![0]!["head_to_head"]!.AsArray()[0]!;

        Assert.Equal("https://a.example/", (string?)h["a"]);
        Assert.Equal("https://b.example/", (string?)h["b"]);
        Assert.Equal(60, (int?)h["buckets"]);
        Assert.Equal(41, (int?)h["a_wins"]);
        Assert.Equal(18, (int?)h["b_wins"]);
        Assert.Equal(1, (int?)h["ties"]);
    }

    [Fact]
    public void An_unwon_crown_serialises_as_null_not_as_a_missing_key()
    {
        var crowns = Serialize()["modes"]![0]!["crowns"]!;

        Assert.Equal("https://a.example/", (string?)crowns["fastest"]);
        Assert.Equal("https://a.example/", (string?)crowns["most_reliable"]);
        Assert.Equal("https://b.example/", (string?)crowns["most_consistent"]);
        Assert.Equal("https://a.example/", (string?)crowns["best_dns"]);
        Assert.Equal("https://b.example/", (string?)crowns["best_ttfb"]);

        // The page distinguishes "no winner" from "field absent"; both TCP and
        // TLS must be PRESENT and null.
        foreach (var key in new[] { "best_tcp", "best_tls" })
        {
            Assert.True(crowns.AsObject().ContainsKey(key), $"crowns.{key} must be present");
            Assert.Null((string?)crowns[key]);
        }
    }

    [Fact]
    public void A_series_point_says_whether_it_counted()
    {
        var p = Serialize()["modes"]![0]!["series"]!.AsArray()[0]!;

        Assert.Equal("https://a.example/", (string?)p["url"]);
        Assert.NotNull(p["bucket"]);
        Assert.Equal(5, (int?)p["sample_count"]);
        Assert.Equal(5, (int?)p["success_count"]);
        // The chart greys the points outside the shared set, so the flag rides
        // with every point rather than being recomputed client-side.
        Assert.True((bool?)p["shared"]);
    }

    // ── query vocabulary ────────────────────────────────────────────────────

    [Theory]
    [InlineData("15m", 900)]
    [InlineData("1h", 3600)]
    [InlineData("6h", 21600)]
    [InlineData("1d", 86400)]
    public void Bucket_widths_are_the_ones_the_page_offers(string label, int seconds)
    {
        Assert.Equal(seconds, ProbeComparisonEndpoints.BucketSeconds[label]);
    }

    [Theory]
    [InlineData("24h", 24)]
    [InlineData("7d", 168)]
    [InlineData("30d", 720)]
    public void Window_lengths_are_the_ones_the_page_offers(string label, int hours)
    {
        Assert.Equal(hours, ProbeComparisonEndpoints.WindowHours[label]);
    }

    [Fact]
    public void Url_parsing_trims_drops_blanks_and_dedupes_while_keeping_order()
    {
        Assert.Equal(
            ["https://b.example/", "https://a.example/"],
            ProbeComparisonEndpoints.ParseUrls(" https://b.example/ , ,https://a.example/,https://b.example/"));
        Assert.Empty(ProbeComparisonEndpoints.ParseUrls(null));
        Assert.Empty(ProbeComparisonEndpoints.ParseUrls("   "));
    }
}
