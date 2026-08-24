using System.Text.Json.Serialization;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Reports;
using Networker.ControlPlane.Reports.Documents;
using Npgsql;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// <c>GET /api/projects/{projectId}/reports/probe-comparison</c> — the <b>URL
/// comparison report</b> (issue #782 P3). Of the URLs this project probes:
/// which is fastest, which is most reliable, which is most consistent — and
/// how far the answer can be trusted.
///
/// <para>It reads the tester-owned V001 probe schema directly (raw Npgsql, like
/// <see cref="AppNetworkEndpoints"/> / <see cref="PerfPerCostEndpoints"/>),
/// restricted to <c>url_probe</c> configs so benchmark, canary and
/// deployment runs can never drift into a URL comparison. Attempts are bucketed
/// by start time and aggregated per URL per bucket; the comparison itself —
/// shared-bucket intersection, head-to-head, crowns, coverage honesty — is
/// computed by the pure <see cref="ProbeComparisonLogic"/> (unit-tested without
/// a DB), and the methodology is embedded in the response as
/// <c>methodology</c>.</para>
///
/// <para><b>Modes are never mixed.</b> An http1 probe and an http3 probe of the
/// same URL are not a fair race, so the report is computed once PER MODE and
/// the response carries one entry per mode with its own scoreboard. The caller
/// picks which to read; nothing is silently pooled.</para>
///
/// <para>With no <c>urls</c> the response is a discovery call: <c>available</c>
/// lists every URL with probe data in the window and its sample count, and the
/// per-mode entries are empty. The page populates its picker from that, so the
/// picker and the report agree on URL identity by construction.</para>
///
/// <para>A missing tester schema (42P01) yields an empty, valid report — never
/// an error. RBAC is member-read, like every other report route.</para>
/// </summary>
public static class ProbeComparisonEndpoints
{
    /// <summary>Bucket widths the report offers, in seconds.</summary>
    internal static readonly Dictionary<string, int> BucketSeconds = new(StringComparer.OrdinalIgnoreCase)
    {
        ["15m"] = 900,
        ["1h"] = 3600,
        ["6h"] = 21600,
        ["1d"] = 86400,
    };

    /// <summary>Window lengths the report offers, in hours.</summary>
    internal static readonly Dictionary<string, int> WindowHours = new(StringComparer.OrdinalIgnoreCase)
    {
        ["24h"] = 24,
        ["7d"] = 168,
        ["30d"] = 720,
    };

    public const string DefaultBucket = "1h";
    public const string DefaultWindow = "7d";

    /// <summary>Hard cap on URLs in one comparison: the pairwise head-to-head
    /// grid is O(n²) and a scoreboard nobody can read is not a report.</summary>
    public const int MaxUrls = 8;

    public static IEndpointRouteBuilder MapProbeComparisonEndpoints(this IEndpointRouteBuilder app)
    {
        app.MapGet("/api/projects/{projectId}/reports/probe-comparison", async (
            string projectId,
            string? urls,
            string? window,
            string? bucket,
            int? min_samples,
            string? format,
            NpgsqlDataSource dataSource,
            ReportExporterResolver exporters,
            CancellationToken ct) =>
        {
            if (!ReportFormats.TryParse(format, out var reportFormat))
            {
                return ReportExport.BadFormat(format, exporters);
            }

            if (!BucketSeconds.TryGetValue(bucket ?? DefaultBucket, out var bucketSeconds))
            {
                return Results.BadRequest(new
                {
                    error = $"unknown bucket '{bucket}'",
                    allowed = BucketSeconds.Keys.ToArray(),
                });
            }

            if (!WindowHours.TryGetValue(window ?? DefaultWindow, out var windowHours))
            {
                return Results.BadRequest(new
                {
                    error = $"unknown window '{window}'",
                    allowed = WindowHours.Keys.ToArray(),
                });
            }

            var selected = ParseUrls(urls);
            if (selected.Count > MaxUrls)
            {
                return Results.BadRequest(new
                {
                    error = $"compare at most {MaxUrls} URLs at once ({selected.Count} requested)",
                });
            }

            var minSamples = Math.Clamp(min_samples ?? ProbeComparisonLogic.DefaultMinSamples, 1, 50);

            var to = DateTime.UtcNow;
            var from = to.AddHours(-windowHours);

            var report = await BuildReportAsync(
                dataSource, projectId, selected, from, to,
                window ?? DefaultWindow, bucket ?? DefaultBucket, bucketSeconds, minSamples, ct);

            if (reportFormat == ReportFormat.Json)
            {
                return Results.Ok(report);
            }

            return ReportExport.Deliver(exporters, reportFormat,
                ProbeComparisonReportDocument.Build(report, projectId),
                fileBase: $"url-comparison-{ReportExport.SafeFileBase(projectId)}", requested: format);
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        return app;
    }

    /// <summary>Comma-separated URL list, trimmed, blank-free and de-duplicated
    /// while keeping the caller's order (the page's column order).</summary>
    internal static List<string> ParseUrls(string? urls)
    {
        if (string.IsNullOrWhiteSpace(urls)) return [];
        var seen = new HashSet<string>(StringComparer.Ordinal);
        return urls.Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
                   .Where(u => seen.Add(u))
                   .ToList();
    }

    internal static async Task<ProbeComparisonReport> BuildReportAsync(
        NpgsqlDataSource dataSource,
        string projectId,
        IReadOnlyList<string> selected,
        DateTime from,
        DateTime to,
        string windowLabel,
        string bucketLabel,
        int bucketSeconds,
        int minSamples,
        CancellationToken ct)
    {
        var available = await LoadAvailableAsync(dataSource, projectId, from, to, ct);

        // The number of buckets the window COULD hold — the denominator every
        // coverage figure is honest about. Not "buckets we saw data in", which
        // would make a URL probed twice look like 100% coverage.
        var windowBuckets = (int)Math.Round((to - from).TotalSeconds / bucketSeconds);

        var modes = new List<ProbeComparisonMode>();
        if (selected.Count >= 2)
        {
            var points = await LoadPointsAsync(
                dataSource, projectId, selected, from, to, bucketSeconds, ct);

            foreach (var modeGroup in points.GroupBy(p => p.Mode).OrderBy(g => g.Key, StringComparer.Ordinal))
            {
                modes.Add(BuildMode(modeGroup.Key, [.. modeGroup.Select(p => p.Point)], selected, windowBuckets, minSamples));
            }
        }

        return new ProbeComparisonReport(
            GeneratedAt: DateTime.UtcNow,
            From: from,
            To: to,
            Window: windowLabel,
            Bucket: bucketLabel,
            BucketSeconds: bucketSeconds,
            WindowBuckets: windowBuckets,
            MinSamples: minSamples,
            MinCoverageRatio: ProbeComparisonLogic.MinCoverageRatio,
            Methodology: new ProbeComparisonMethodology(
                Buckets: $"attempts are grouped into fixed {bucketLabel} buckets by start time; a bucket counts for a URL once it holds >= {minSamples} samples of it",
                Shared: "every headline number is computed only over buckets in which EVERY eligible URL has measurements — a window-wide aggregate would reward a URL for being probed at quiet hours",
                Eligibility: $"a URL whose own measured buckets cover < {ProbeComparisonLogic.MinCoverageRatio:P0} of the window is excluded and shown greyed, so one barely-probed URL cannot shrink the shared set for the others",
                Ranking: $"URLs are ranked head-to-head: how many shared buckets each one's p50 won. The scoreboard is greyed unless the shared set covers >= {ProbeComparisonLogic.MinCoverageRatio:P0} of the window and holds >= {ProbeComparisonLogic.MinSharedBuckets} buckets",
                Crowns: "fastest = lowest median of per-bucket p50s; most reliable = highest success rate; most consistent = lowest p95/p50; phase crowns are the lowest median DNS / TCP / TLS / TTFB. A tie awards no crown, and a phase nothing measured never wins one",
                Modes: "modes are compared separately — an http1 probe and an http3 probe of the same URL are not the same race"),
            Available: available,
            Modes: modes);
    }

    /// <summary>Assemble one mode's scoreboard from its bucket points.</summary>
    internal static ProbeComparisonMode BuildMode(
        string mode,
        IReadOnlyList<ProbeComparisonLogic.BucketPoint> points,
        IReadOnlyList<string> selected,
        int windowBuckets,
        int minSamples)
    {
        var coverage = ProbeComparisonLogic.Coverage(points, selected, windowBuckets, minSamples);
        var shared = ProbeComparisonLogic.SharedBuckets(points, coverage, minSamples);
        var verdict = ProbeComparisonLogic.RankingVerdict(coverage, shared.Count, windowBuckets);
        var eligible = coverage.Where(c => c.Eligible).Select(c => c.Url).ToList();

        var scores = eligible.Select(u => ProbeComparisonLogic.Score(points, u, shared)).ToList();
        var ranked = verdict == ProbeComparisonLogic.ReasonRanked;

        return new ProbeComparisonMode(
            Mode: mode,
            Ranked: ranked,
            RankingVerdict: verdict,
            SharedBuckets: shared.Count,
            CoverageRatio: ProbeComparisonLogic.CoverageRatio(shared.Count, windowBuckets),
            Coverage: [.. coverage.Select(c => new ProbeComparisonCoverage(
                c.Url, c.QualifyingBuckets, c.TotalSamples, c.Eligible, c.ExcludedReason))],
            Scores: [.. scores.Select(s => new ProbeComparisonScore(
                Url: s.Url,
                SharedBuckets: s.SharedBuckets,
                Samples: s.Samples,
                MedianP50Ms: s.MedianP50Ms,
                MedianP95Ms: s.MedianP95Ms,
                SuccessRate: s.SuccessRate,
                JitterRatio: s.JitterRatio,
                MedianDnsMs: s.MedianDnsMs,
                MedianTcpMs: s.MedianTcpMs,
                MedianTlsMs: s.MedianTlsMs,
                MedianTtfbMs: s.MedianTtfbMs,
                DominantErrorCategory: s.DominantErrorCategory))],
            HeadToHead: [.. ProbeComparisonLogic.AllPairs(points, eligible, shared)
                .Select(h => new ProbeComparisonHeadToHead(h.A, h.B, h.Buckets, h.AWins, h.BWins, h.Ties))],
            // Crowns are only awarded on a ranking the report stands behind.
            Crowns: ranked
                ? new ProbeComparisonCrowns(
                    Fastest: ProbeComparisonLogic.Lowest(scores, s => s.MedianP50Ms),
                    MostReliable: ProbeComparisonLogic.Highest(scores, s => s.SuccessRate),
                    MostConsistent: ProbeComparisonLogic.Lowest(scores, s => s.JitterRatio),
                    BestDns: ProbeComparisonLogic.Lowest(scores, s => s.MedianDnsMs),
                    BestTcp: ProbeComparisonLogic.Lowest(scores, s => s.MedianTcpMs),
                    BestTls: ProbeComparisonLogic.Lowest(scores, s => s.MedianTlsMs),
                    BestTtfb: ProbeComparisonLogic.Lowest(scores, s => s.MedianTtfbMs))
                : new ProbeComparisonCrowns(null, null, null, null, null, null, null),
            Series: [.. points
                .OrderBy(p => p.Bucket)
                .ThenBy(p => p.Url, StringComparer.Ordinal)
                .Select(p => new ProbeComparisonPoint(
                    Url: p.Url,
                    Bucket: p.Bucket,
                    SampleCount: p.SampleCount,
                    SuccessCount: p.SuccessCount,
                    Shared: shared.Contains(p.Bucket),
                    P50TotalMs: p.P50TotalMs,
                    P95TotalMs: p.P95TotalMs,
                    P50DnsMs: p.P50DnsMs,
                    P50TcpMs: p.P50TcpMs,
                    P50TlsMs: p.P50TlsMs,
                    P50TtfbMs: p.P50TtfbMs,
                    DominantErrorCategory: p.DominantErrorCategory))]);
    }

    // ── SQL ──────────────────────────────────────────────────────────────────

    /// <summary>
    /// Attempts of this project's URL-probe runs, in the window.
    ///
    /// <para><c>TargetUrl</c> is per-attempt from V006, but NULL on rows written
    /// by a tester older than v0.28.231 and on single-target runs of that era —
    /// so it falls back to the tester TestRun's own TargetUrl, which every run
    /// has always carried. Without the fallback the whole pre-#820 history of
    /// single-URL probes would be invisible to the comparison, which is most of
    /// the data anyone has.</para>
    ///
    /// <para>Bucketing is <c>floor(epoch / width)</c> rather than TimescaleDB's
    /// <c>time_bucket</c>: identical results for the fixed epoch-aligned widths
    /// this report offers, no extension dependency (so it also works against a
    /// plain-PostgreSQL test or lab database), and it expresses 15m and 6h
    /// widths that <c>date_trunc</c> cannot.</para>
    /// </summary>
    private const string PointsSql = """
        WITH sample AS (
            SELECT COALESCE(a.TargetUrl, tr.TargetUrl)              AS url,
                   LOWER(a.Protocol)                                AS mode,
                   TO_TIMESTAMP(FLOOR(EXTRACT(EPOCH FROM a.StartedAt) / $4) * $4) AS bucket,
                   a.Success                                        AS success,
                   CASE WHEN a.Success THEN COALESCE(
                            h.TotalDurationMs,
                            EXTRACT(EPOCH FROM (a.FinishedAt - a.StartedAt)) * 1000.0)
                   END                                              AS total_ms,
                   CASE WHEN a.Success THEN d.DurationMs         END AS dns_ms,
                   CASE WHEN a.Success THEN t.ConnectDurationMs  END AS tcp_ms,
                   CASE WHEN a.Success THEN s.HandshakeDurationMs END AS tls_ms,
                   CASE WHEN a.Success THEN h.TtfbMs             END AS ttfb_ms
            FROM test_run r
            -- Project isolation enforced in SQL, not by a launch-time invariant:
            -- a stray test_run row can never pull another project's data in.
            JOIN test_config c ON c.id = r.test_config_id
                              AND c.project_id = $1
                              AND c.test_kind = 'url_probe'
            JOIN RequestAttempt a ON a.RunId = r.id
            LEFT JOIN TestRun tr    ON tr.RunId = r.id
            LEFT JOIN HttpResult h  ON h.AttemptId = a.AttemptId
            LEFT JOIN DnsResult d   ON d.AttemptId = a.AttemptId
            LEFT JOIN TcpResult t   ON t.AttemptId = a.AttemptId
            LEFT JOIN TlsResult s   ON s.AttemptId = a.AttemptId
            WHERE r.project_id = $1
              AND a.StartedAt >= $2
              AND a.StartedAt <  $3
        ),
        picked AS (
            SELECT * FROM sample WHERE url = ANY($5)
        )
        SELECT url,
               mode,
               bucket,
               COUNT(*)::int                                                        AS sample_count,
               COUNT(*) FILTER (WHERE success)::int                                 AS success_count,
               PERCENTILE_CONT(0.5)  WITHIN GROUP (ORDER BY total_ms) FILTER (WHERE total_ms IS NOT NULL) AS p50_total_ms,
               PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY total_ms) FILTER (WHERE total_ms IS NOT NULL) AS p95_total_ms,
               PERCENTILE_CONT(0.5)  WITHIN GROUP (ORDER BY dns_ms)   FILTER (WHERE dns_ms   IS NOT NULL) AS p50_dns_ms,
               PERCENTILE_CONT(0.5)  WITHIN GROUP (ORDER BY tcp_ms)   FILTER (WHERE tcp_ms   IS NOT NULL) AS p50_tcp_ms,
               PERCENTILE_CONT(0.5)  WITHIN GROUP (ORDER BY tls_ms)   FILTER (WHERE tls_ms   IS NOT NULL) AS p50_tls_ms,
               PERCENTILE_CONT(0.5)  WITHIN GROUP (ORDER BY ttfb_ms)  FILTER (WHERE ttfb_ms  IS NOT NULL) AS p50_ttfb_ms
        FROM picked
        GROUP BY url, mode, bucket
        ORDER BY mode, bucket, url
        """;

    /// <summary>
    /// The dominant error category per URL/mode/bucket, joined onto the points
    /// separately: ErrorRecord is one-to-many per attempt, so folding it into
    /// the aggregate above would multiply the sample counts.
    /// </summary>
    private const string ErrorsSql = """
        WITH failed AS (
            SELECT COALESCE(a.TargetUrl, tr.TargetUrl)              AS url,
                   LOWER(a.Protocol)                                AS mode,
                   TO_TIMESTAMP(FLOOR(EXTRACT(EPOCH FROM a.StartedAt) / $4) * $4) AS bucket,
                   e.ErrorCategory                                  AS category
            FROM test_run r
            JOIN test_config c ON c.id = r.test_config_id
                              AND c.project_id = $1
                              AND c.test_kind = 'url_probe'
            JOIN RequestAttempt a ON a.RunId = r.id AND NOT a.Success
            JOIN ErrorRecord e    ON e.AttemptId = a.AttemptId
            LEFT JOIN TestRun tr  ON tr.RunId = r.id
            WHERE r.project_id = $1
              AND a.StartedAt >= $2
              AND a.StartedAt <  $3
        ),
        counted AS (
            SELECT url, mode, bucket, category, COUNT(*) AS n
            FROM failed
            WHERE url = ANY($5)
            GROUP BY url, mode, bucket, category
        )
        SELECT DISTINCT ON (url, mode, bucket) url, mode, bucket, category
        FROM counted
        ORDER BY url, mode, bucket, n DESC, category
        """;

    /// <summary>Every URL with probe data in the window, newest-first by volume
    /// — the picker's source of truth, so the picker and the report can never
    /// disagree about what a URL is called.</summary>
    private const string AvailableSql = """
        SELECT COALESCE(a.TargetUrl, tr.TargetUrl)  AS url,
               COUNT(*)::int                        AS sample_count,
               COUNT(DISTINCT LOWER(a.Protocol))::int AS mode_count,
               MAX(a.StartedAt)                     AS last_seen
        FROM test_run r
        JOIN test_config c ON c.id = r.test_config_id
                          AND c.project_id = $1
                          AND c.test_kind = 'url_probe'
        JOIN RequestAttempt a ON a.RunId = r.id
        LEFT JOIN TestRun tr  ON tr.RunId = r.id
        WHERE r.project_id = $1
          AND a.StartedAt >= $2
          AND a.StartedAt <  $3
          AND COALESCE(a.TargetUrl, tr.TargetUrl) IS NOT NULL
        GROUP BY COALESCE(a.TargetUrl, tr.TargetUrl)
        ORDER BY sample_count DESC, url
        """;

    private sealed record ModePoint(string Mode, ProbeComparisonLogic.BucketPoint Point);

    private static async Task<List<ProbeComparisonAvailable>> LoadAvailableAsync(
        NpgsqlDataSource dataSource, string projectId, DateTime from, DateTime to, CancellationToken ct)
    {
        var rows = new List<ProbeComparisonAvailable>();
        try
        {
            await using var cmd = dataSource.CreateCommand(AvailableSql);
            cmd.Parameters.AddWithValue(projectId);
            cmd.Parameters.AddWithValue(from);
            cmd.Parameters.AddWithValue(to);
            await using var reader = await cmd.ExecuteReaderAsync(ct);
            while (await reader.ReadAsync(ct))
            {
                rows.Add(new ProbeComparisonAvailable(
                    Url: reader.GetString(0),
                    SampleCount: reader.GetInt32(1),
                    ModeCount: reader.GetInt32(2),
                    LastSeen: reader.GetDateTime(3)));
            }
        }
        catch (PostgresException ex) when (ex.SqlState == PostgresErrorCodes.UndefinedTable)
        {
            // Tester result schema not present in this database — nothing probed yet.
        }

        return rows;
    }

    private static async Task<List<ModePoint>> LoadPointsAsync(
        NpgsqlDataSource dataSource,
        string projectId,
        IReadOnlyList<string> urls,
        DateTime from,
        DateTime to,
        int bucketSeconds,
        CancellationToken ct)
    {
        var raw = new List<(string Mode, string Url, DateTime Bucket, int Samples, int Successes,
                            double? P50, double? P95, double? Dns, double? Tcp, double? Tls, double? Ttfb)>();
        var errors = new Dictionary<(string Mode, string Url, DateTime Bucket), string>();

        try
        {
            await using (var cmd = dataSource.CreateCommand(PointsSql))
            {
                cmd.Parameters.AddWithValue(projectId);
                cmd.Parameters.AddWithValue(from);
                cmd.Parameters.AddWithValue(to);
                cmd.Parameters.AddWithValue((double)bucketSeconds);
                cmd.Parameters.AddWithValue(urls.ToArray());
                await using var reader = await cmd.ExecuteReaderAsync(ct);
                while (await reader.ReadAsync(ct))
                {
                    raw.Add((
                        reader.GetString(1),
                        reader.GetString(0),
                        reader.GetDateTime(2),
                        reader.GetInt32(3),
                        reader.GetInt32(4),
                        Nullable(reader, 5), Nullable(reader, 6), Nullable(reader, 7),
                        Nullable(reader, 8), Nullable(reader, 9), Nullable(reader, 10)));
                }
            }

            await using (var cmd = dataSource.CreateCommand(ErrorsSql))
            {
                cmd.Parameters.AddWithValue(projectId);
                cmd.Parameters.AddWithValue(from);
                cmd.Parameters.AddWithValue(to);
                cmd.Parameters.AddWithValue((double)bucketSeconds);
                cmd.Parameters.AddWithValue(urls.ToArray());
                await using var reader = await cmd.ExecuteReaderAsync(ct);
                while (await reader.ReadAsync(ct))
                {
                    errors[(reader.GetString(1), reader.GetString(0), reader.GetDateTime(2))] =
                        reader.GetString(3);
                }
            }
        }
        catch (PostgresException ex) when (ex.SqlState == PostgresErrorCodes.UndefinedTable)
        {
            return [];
        }

        return [.. raw.Select(r => new ModePoint(r.Mode, new ProbeComparisonLogic.BucketPoint(
            Url: r.Url,
            Bucket: r.Bucket,
            SampleCount: r.Samples,
            SuccessCount: r.Successes,
            P50TotalMs: ProbeComparisonLogic.Round4(r.P50),
            P95TotalMs: ProbeComparisonLogic.Round4(r.P95),
            P50DnsMs: ProbeComparisonLogic.Round4(r.Dns),
            P50TcpMs: ProbeComparisonLogic.Round4(r.Tcp),
            P50TlsMs: ProbeComparisonLogic.Round4(r.Tls),
            P50TtfbMs: ProbeComparisonLogic.Round4(r.Ttfb),
            DominantErrorCategory: errors.GetValueOrDefault((r.Mode, r.Url, r.Bucket)))))];
    }

    private static double? Nullable(NpgsqlDataReader reader, int ordinal) =>
        reader.IsDBNull(ordinal) ? null : reader.GetDouble(ordinal);
}

// ── Wire shapes (snake_case, pinned by ProbeComparisonContractTests) ─────────

public sealed record ProbeComparisonReport(
    [property: JsonPropertyName("generated_at")] DateTime GeneratedAt,
    [property: JsonPropertyName("from")] DateTime From,
    [property: JsonPropertyName("to")] DateTime To,
    [property: JsonPropertyName("window")] string Window,
    [property: JsonPropertyName("bucket")] string Bucket,
    [property: JsonPropertyName("bucket_seconds")] int BucketSeconds,
    [property: JsonPropertyName("window_buckets")] int WindowBuckets,
    [property: JsonPropertyName("min_samples")] int MinSamples,
    [property: JsonPropertyName("min_coverage_ratio")] double MinCoverageRatio,
    [property: JsonPropertyName("methodology")] ProbeComparisonMethodology Methodology,
    [property: JsonPropertyName("available")] IReadOnlyList<ProbeComparisonAvailable> Available,
    [property: JsonPropertyName("modes")] IReadOnlyList<ProbeComparisonMode> Modes);

public sealed record ProbeComparisonMethodology(
    [property: JsonPropertyName("buckets")] string Buckets,
    [property: JsonPropertyName("shared")] string Shared,
    [property: JsonPropertyName("eligibility")] string Eligibility,
    [property: JsonPropertyName("ranking")] string Ranking,
    [property: JsonPropertyName("crowns")] string Crowns,
    [property: JsonPropertyName("modes")] string Modes);

public sealed record ProbeComparisonAvailable(
    [property: JsonPropertyName("url")] string Url,
    [property: JsonPropertyName("sample_count")] int SampleCount,
    [property: JsonPropertyName("mode_count")] int ModeCount,
    [property: JsonPropertyName("last_seen")] DateTime LastSeen);

public sealed record ProbeComparisonMode(
    [property: JsonPropertyName("mode")] string Mode,
    [property: JsonPropertyName("ranked")] bool Ranked,
    [property: JsonPropertyName("ranking_verdict")] string RankingVerdict,
    [property: JsonPropertyName("shared_buckets")] int SharedBuckets,
    [property: JsonPropertyName("coverage_ratio")] double CoverageRatio,
    [property: JsonPropertyName("coverage")] IReadOnlyList<ProbeComparisonCoverage> Coverage,
    [property: JsonPropertyName("scores")] IReadOnlyList<ProbeComparisonScore> Scores,
    [property: JsonPropertyName("head_to_head")] IReadOnlyList<ProbeComparisonHeadToHead> HeadToHead,
    [property: JsonPropertyName("crowns")] ProbeComparisonCrowns Crowns,
    [property: JsonPropertyName("series")] IReadOnlyList<ProbeComparisonPoint> Series);

public sealed record ProbeComparisonCoverage(
    [property: JsonPropertyName("url")] string Url,
    [property: JsonPropertyName("qualifying_buckets")] int QualifyingBuckets,
    [property: JsonPropertyName("total_samples")] int TotalSamples,
    [property: JsonPropertyName("eligible")] bool Eligible,
    [property: JsonPropertyName("excluded_reason")] string? ExcludedReason);

public sealed record ProbeComparisonScore(
    [property: JsonPropertyName("url")] string Url,
    [property: JsonPropertyName("shared_buckets")] int SharedBuckets,
    [property: JsonPropertyName("samples")] int Samples,
    [property: JsonPropertyName("median_p50_ms")] double? MedianP50Ms,
    [property: JsonPropertyName("median_p95_ms")] double? MedianP95Ms,
    [property: JsonPropertyName("success_rate")] double? SuccessRate,
    [property: JsonPropertyName("jitter_ratio")] double? JitterRatio,
    [property: JsonPropertyName("median_dns_ms")] double? MedianDnsMs,
    [property: JsonPropertyName("median_tcp_ms")] double? MedianTcpMs,
    [property: JsonPropertyName("median_tls_ms")] double? MedianTlsMs,
    [property: JsonPropertyName("median_ttfb_ms")] double? MedianTtfbMs,
    [property: JsonPropertyName("dominant_error_category")] string? DominantErrorCategory);

public sealed record ProbeComparisonHeadToHead(
    [property: JsonPropertyName("a")] string A,
    [property: JsonPropertyName("b")] string B,
    [property: JsonPropertyName("buckets")] int Buckets,
    [property: JsonPropertyName("a_wins")] int AWins,
    [property: JsonPropertyName("b_wins")] int BWins,
    [property: JsonPropertyName("ties")] int Ties);

public sealed record ProbeComparisonCrowns(
    [property: JsonPropertyName("fastest")] string? Fastest,
    [property: JsonPropertyName("most_reliable")] string? MostReliable,
    [property: JsonPropertyName("most_consistent")] string? MostConsistent,
    [property: JsonPropertyName("best_dns")] string? BestDns,
    [property: JsonPropertyName("best_tcp")] string? BestTcp,
    [property: JsonPropertyName("best_tls")] string? BestTls,
    [property: JsonPropertyName("best_ttfb")] string? BestTtfb);

public sealed record ProbeComparisonPoint(
    [property: JsonPropertyName("url")] string Url,
    [property: JsonPropertyName("bucket")] DateTime Bucket,
    [property: JsonPropertyName("sample_count")] int SampleCount,
    [property: JsonPropertyName("success_count")] int SuccessCount,
    [property: JsonPropertyName("shared")] bool Shared,
    [property: JsonPropertyName("p50_total_ms")] double? P50TotalMs,
    [property: JsonPropertyName("p95_total_ms")] double? P95TotalMs,
    [property: JsonPropertyName("p50_dns_ms")] double? P50DnsMs,
    [property: JsonPropertyName("p50_tcp_ms")] double? P50TcpMs,
    [property: JsonPropertyName("p50_tls_ms")] double? P50TlsMs,
    [property: JsonPropertyName("p50_ttfb_ms")] double? P50TtfbMs,
    [property: JsonPropertyName("dominant_error_category")] string? DominantErrorCategory);
