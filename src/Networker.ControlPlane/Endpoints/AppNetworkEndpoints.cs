using System.Text.Json.Serialization;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Reports;
using Npgsql;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// GET /api/projects/{projectId}/reports/app-network — the <b>Application
/// Network Performance</b> report. For an SDK endpoint (a <c>sdkprobe</c> test
/// config) it answers the one question customers care about: <i>"is my slowness
/// the application or the network?"</i>
///
/// <para>It reads the tester-owned V001 probe schema directly (raw Npgsql, like
/// <see cref="PerfPerCostEndpoints"/> / the alerting <c>RunMetricProvider</c>).
/// Per successful sdkprobe attempt it takes the wall latency
/// (RequestAttempt started→finished) and the SDK's server time
/// (<c>ServerTimingResult.TotalServerMs</c>, joined on AttemptId — the
/// <c>Server-Timing: total;dur</c> the LagHound SDK emits). The network/server
/// split, the verdict, and the human summary are computed by the pure
/// <see cref="AppNetworkLogic"/> (unit-tested without a DB); the formulas are
/// embedded in the response and written up in <c>docs/reports-app-network.md</c>.</para>
///
/// <para>A missing tester schema (42P01) yields an empty, valid report — never
/// an error. RBAC is member-read (any project role, including viewer); a
/// non-member gets the same 403 every <c>/api/projects/{projectId}/*</c> route
/// gives. An optional <c>?config_id=</c> narrows to one SDK endpoint.</para>
/// </summary>
public static class AppNetworkEndpoints
{
    public static IEndpointRouteBuilder MapAppNetworkEndpoints(this IEndpointRouteBuilder app)
    {
        app.MapGet("/api/projects/{projectId}/reports/app-network", async (
            string projectId,
            Guid? config_id,
            NpgsqlDataSource dataSource,
            ILoggerFactory loggerFactory,
            CancellationToken ct) =>
        {
            var log = loggerFactory.CreateLogger("AppNetwork");

            var aggregates = await LoadAggregatesAsync(dataSource, projectId, config_id, ct);

            var groups = new List<AppNetworkGroup>();
            long totalAttempts = 0;
            long totalAnomalies = 0;
            var runIds = new HashSet<Guid>();

            foreach (var a in aggregates
                         .OrderBy(x => x.ConfigName, StringComparer.Ordinal)
                         .ThenBy(x => x.ConfigId))
            {
                var verdict = AppNetworkLogic.Verdict(a.MedianServerMs, a.MedianNetworkMs, a.MedianWallMs);
                groups.Add(new AppNetworkGroup(
                    ConfigId: a.ConfigId,
                    ConfigName: a.ConfigName,
                    RunCount: a.RunCount,
                    AttemptCount: a.AttemptCount,
                    SplitAnomalyCount: a.SplitAnomalyCount,
                    MedianServerMs: AppNetworkLogic.Round4(a.MedianServerMs),
                    P95ServerMs: AppNetworkLogic.Round4(a.P95ServerMs),
                    MedianNetworkMs: AppNetworkLogic.Round4(a.MedianNetworkMs),
                    P95NetworkMs: AppNetworkLogic.Round4(a.P95NetworkMs),
                    MedianWallMs: AppNetworkLogic.Round4(a.MedianWallMs),
                    ServerRatio: AppNetworkLogic.ServerRatio(a.MedianServerMs, a.MedianWallMs),
                    Verdict: verdict,
                    MainIssue: AppNetworkLogic.MainIssue(
                        verdict, a.MedianServerMs, a.MedianNetworkMs, a.MedianWallMs)));

                totalAttempts += a.AttemptCount;
                totalAnomalies += a.SplitAnomalyCount;
            }

            // Overall verdict from the pooled medians across every selected group.
            // (Group-count-weighted medians aren't recomputable from group
            // summaries, so the overall verdict is derived from ALL attempts in
            // a second pass — cheap, one extra ordered-set aggregate.)
            var overallStats = await LoadOverallAsync(dataSource, projectId, config_id, ct);
            var overallVerdict = AppNetworkLogic.Verdict(
                overallStats.MedianServerMs, overallStats.MedianNetworkMs, overallStats.MedianWallMs);

            if (totalAnomalies > 0)
            {
                log.LogInformation(
                    "app-network project={ProjectId} config={ConfigId}: {Count} split anomaly attempt(s) "
                    + "(server time > wall time) — clock skew or SDK span mismatch",
                    projectId, config_id, totalAnomalies);
            }

            return Results.Ok(new AppNetworkReport(
                GeneratedAt: DateTime.UtcNow,
                Formulas: new AppNetworkFormulas(
                    ServerMs: "server_ms = ServerTimingResult.TotalServerMs (the SDK's Server-Timing total;dur, per attempt joined on AttemptId)",
                    NetworkMs: "network_ms = max(0, wall_ms - server_ms) where wall_ms = RequestAttempt finished - started",
                    Split: $"a side is dominant (verdict) when its median >= {AppNetworkLogic.DominanceRatio:0.##} of median wall; else balanced",
                    SplitAnomaly: "split_anomaly = server_ms > wall_ms (counted; network_ms floors at 0)"),
                Mode: SdkEndpointsEndpoints.SdkProbeMode,
                AttemptCount: totalAttempts,
                SplitAnomalyCount: totalAnomalies,
                OverallVerdict: overallVerdict,
                OverallMainIssue: AppNetworkLogic.MainIssue(
                    overallVerdict, overallStats.MedianServerMs, overallStats.MedianNetworkMs, overallStats.MedianWallMs),
                OverallMedianServerMs: AppNetworkLogic.Round4(overallStats.MedianServerMs),
                OverallMedianNetworkMs: AppNetworkLogic.Round4(overallStats.MedianNetworkMs),
                OverallMedianWallMs: AppNetworkLogic.Round4(overallStats.MedianWallMs),
                OverallServerRatio: AppNetworkLogic.ServerRatio(overallStats.MedianServerMs, overallStats.MedianWallMs),
                Groups: groups));
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        return app;
    }

    // ── SQL ──────────────────────────────────────────────────────────────────

    private sealed record Aggregate(
        Guid ConfigId, string ConfigName, int RunCount, int AttemptCount, int SplitAnomalyCount,
        double? MedianServerMs, double? P95ServerMs,
        double? MedianNetworkMs, double? P95NetworkMs, double? MedianWallMs);

    private sealed record OverallStats(
        double? MedianServerMs, double? MedianNetworkMs, double? MedianWallMs);

    /// <summary>Shared FROM/WHERE: completed sdkprobe attempts of this project
    /// (optionally one config), with wall/server/network per attempt.</summary>
    private const string BaseCte = """
        WITH attempt AS (
            SELECT r.test_config_id AS config_id,
                   r.id             AS run_id,
                   EXTRACT(EPOCH FROM (a.FinishedAt - a.StartedAt)) * 1000.0 AS wall_ms,
                   st.TotalServerMs AS server_ms
            FROM test_run r
            JOIN RequestAttempt a ON a.RunId = r.id AND a.Success
            JOIN ServerTimingResult st ON st.AttemptId = a.AttemptId
            WHERE r.project_id = $1
              AND r.status = 'completed'
              AND LOWER(a.Protocol) = 'sdkprobe'
              AND st.TotalServerMs IS NOT NULL
              AND a.FinishedAt IS NOT NULL
              {CONFIG_FILTER}
        ),
        split AS (
            SELECT config_id, run_id,
                   wall_ms,
                   server_ms,
                   GREATEST(0.0, wall_ms - server_ms) AS network_ms,
                   (server_ms > wall_ms)              AS anomaly
            FROM attempt
        )
        """;

    private static async Task<List<Aggregate>> LoadAggregatesAsync(
        NpgsqlDataSource dataSource, string projectId, Guid? configId, CancellationToken ct)
    {
        var configFilter = configId is null ? "" : "AND r.test_config_id = $2";
        var sql = BaseCte.Replace("{CONFIG_FILTER}", configFilter) + """

            SELECT s.config_id,
                   c.name AS config_name,
                   COUNT(DISTINCT s.run_id)::int              AS run_count,
                   COUNT(*)::int                              AS attempt_count,
                   COUNT(*) FILTER (WHERE s.anomaly)::int     AS anomaly_count,
                   PERCENTILE_CONT(0.5)  WITHIN GROUP (ORDER BY s.server_ms)  AS median_server_ms,
                   PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY s.server_ms)  AS p95_server_ms,
                   PERCENTILE_CONT(0.5)  WITHIN GROUP (ORDER BY s.network_ms) AS median_network_ms,
                   PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY s.network_ms) AS p95_network_ms,
                   PERCENTILE_CONT(0.5)  WITHIN GROUP (ORDER BY s.wall_ms)    AS median_wall_ms
            FROM split s
            -- Belt-and-suspenders: the config name is only ever surfaced for a
            -- config that ALSO belongs to $1's project. test_run.project_id is
            -- written from the config's project at launch, so this holds today —
            -- but enforcing it in SQL (not by a launch-time invariant) means a
            -- stray/mis-written test_run can never leak another project's config
            -- name here (project-isolation audit §3 / P2).
            JOIN test_config c ON c.id = s.config_id AND c.project_id = $1
            GROUP BY s.config_id, c.name
            """;

        var rows = new List<Aggregate>();
        try
        {
            await using var cmd = dataSource.CreateCommand(sql);
            cmd.Parameters.AddWithValue(projectId);
            if (configId is { } cid)
            {
                cmd.Parameters.AddWithValue(cid);
            }
            await using var reader = await cmd.ExecuteReaderAsync(ct);
            while (await reader.ReadAsync(ct))
            {
                rows.Add(new Aggregate(
                    ConfigId: reader.GetGuid(0),
                    ConfigName: reader.GetString(1),
                    RunCount: reader.GetInt32(2),
                    AttemptCount: reader.GetInt32(3),
                    SplitAnomalyCount: reader.GetInt32(4),
                    MedianServerMs: Nullable(reader, 5),
                    P95ServerMs: Nullable(reader, 6),
                    MedianNetworkMs: Nullable(reader, 7),
                    P95NetworkMs: Nullable(reader, 8),
                    MedianWallMs: Nullable(reader, 9)));
            }
        }
        catch (PostgresException ex) when (ex.SqlState == PostgresErrorCodes.UndefinedTable)
        {
            // Tester probe schema absent (no sdkprobe has ever persisted here) —
            // a valid empty report, not an error.
        }

        return rows;
    }

    private static async Task<OverallStats> LoadOverallAsync(
        NpgsqlDataSource dataSource, string projectId, Guid? configId, CancellationToken ct)
    {
        var configFilter = configId is null ? "" : "AND r.test_config_id = $2";
        var sql = BaseCte.Replace("{CONFIG_FILTER}", configFilter) + """

            SELECT PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY s.server_ms)  AS median_server_ms,
                   PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY s.network_ms) AS median_network_ms,
                   PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY s.wall_ms)    AS median_wall_ms
            FROM split s
            """;

        try
        {
            await using var cmd = dataSource.CreateCommand(sql);
            cmd.Parameters.AddWithValue(projectId);
            if (configId is { } cid)
            {
                cmd.Parameters.AddWithValue(cid);
            }
            await using var reader = await cmd.ExecuteReaderAsync(ct);
            if (await reader.ReadAsync(ct))
            {
                return new OverallStats(Nullable(reader, 0), Nullable(reader, 1), Nullable(reader, 2));
            }
        }
        catch (PostgresException ex) when (ex.SqlState == PostgresErrorCodes.UndefinedTable)
        {
            // schema absent → no overall stats.
        }

        return new OverallStats(null, null, null);
    }

    private static double? Nullable(NpgsqlDataReader reader, int ordinal) =>
        reader.IsDBNull(ordinal) ? null : reader.GetDouble(ordinal);
}

// ── Wire shapes (snake_case, pinned by AppNetworkContractTests) ──────────────

public sealed record AppNetworkReport(
    [property: JsonPropertyName("generated_at")] DateTime GeneratedAt,
    [property: JsonPropertyName("formulas")] AppNetworkFormulas Formulas,
    [property: JsonPropertyName("mode")] string Mode,
    [property: JsonPropertyName("attempt_count")] long AttemptCount,
    [property: JsonPropertyName("split_anomaly_count")] long SplitAnomalyCount,
    [property: JsonPropertyName("overall_verdict")] string OverallVerdict,
    [property: JsonPropertyName("overall_main_issue")] string OverallMainIssue,
    [property: JsonPropertyName("overall_median_server_ms")] double? OverallMedianServerMs,
    [property: JsonPropertyName("overall_median_network_ms")] double? OverallMedianNetworkMs,
    [property: JsonPropertyName("overall_median_wall_ms")] double? OverallMedianWallMs,
    [property: JsonPropertyName("overall_server_ratio")] double? OverallServerRatio,
    [property: JsonPropertyName("groups")] IReadOnlyList<AppNetworkGroup> Groups);

public sealed record AppNetworkFormulas(
    [property: JsonPropertyName("server_ms")] string ServerMs,
    [property: JsonPropertyName("network_ms")] string NetworkMs,
    [property: JsonPropertyName("split")] string Split,
    [property: JsonPropertyName("split_anomaly")] string SplitAnomaly);

public sealed record AppNetworkGroup(
    [property: JsonPropertyName("config_id")] Guid ConfigId,
    [property: JsonPropertyName("config_name")] string ConfigName,
    [property: JsonPropertyName("run_count")] int RunCount,
    [property: JsonPropertyName("attempt_count")] int AttemptCount,
    [property: JsonPropertyName("split_anomaly_count")] int SplitAnomalyCount,
    [property: JsonPropertyName("median_server_ms")] double? MedianServerMs,
    [property: JsonPropertyName("p95_server_ms")] double? P95ServerMs,
    [property: JsonPropertyName("median_network_ms")] double? MedianNetworkMs,
    [property: JsonPropertyName("p95_network_ms")] double? P95NetworkMs,
    [property: JsonPropertyName("median_wall_ms")] double? MedianWallMs,
    [property: JsonPropertyName("server_ratio")] double? ServerRatio,
    [property: JsonPropertyName("verdict")] string Verdict,
    [property: JsonPropertyName("main_issue")] string MainIssue);
