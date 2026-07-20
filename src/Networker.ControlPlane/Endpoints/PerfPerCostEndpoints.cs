using System.Text.Json.Serialization;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Reports;
using Networker.Data;
using Npgsql;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// GET /api/projects/{projectId}/reports/perf-per-cost — which provider gives
/// the best performance per dollar. Aggregates the probe results of COMPLETED
/// runs per (provider, vm_size, region) tester group and mode family, then
/// joins the static curated price table (<c>shared/cloud-costs.json</c>,
/// <see cref="CloudCostTable"/>) to compute the two value scores defined in
/// <see cref="PerfPerCostLogic"/>. Formulas are embedded in the response so
/// the numbers are self-documenting; full write-up in
/// <c>docs/reports-perf-per-cost.md</c>.
///
/// Perf data comes from the tester-owned V001 schema (RequestAttempt /
/// HttpResult — NOT in the EF model, raw Npgsql like
/// <see cref="Alerting.RunMetricProvider"/>): per successful attempt,
/// <c>HttpResult.TotalDurationMs</c> when present, else the attempt's
/// started→finished wall time; throughput from <c>HttpResult.ThroughputMbps</c>.
/// A missing tester schema (42P01) yields an empty report, not an error.
///
/// Honesty rules: tester groups whose SKU is absent from the price table are
/// NEVER dropped — they ship with <c>hourly_usd: null</c>, a note, and are
/// counted in <c>missing_cost_skus</c> (also logged). No pricing API is
/// called at runtime by design.
/// </summary>
public static class PerfPerCostEndpoints
{
    public static IEndpointRouteBuilder MapPerfPerCostEndpoints(this IEndpointRouteBuilder app)
    {
        // Project-scoped route → ProjectMember policy (member-read: any role
        // including viewer). Non-members get the same 403 the policy gives
        // every /api/projects/{projectId}/* route.
        app.MapGet("/api/projects/{projectId}/reports/perf-per-cost", async (
            string projectId,
            NetworkerDbContext db,
            NpgsqlDataSource dataSource,
            ILoggerFactory loggerFactory,
            CancellationToken ct) =>
        {
            var log = loggerFactory.CreateLogger("PerfPerCost");
            var costs = CloudCostTable.Instance;

            var aggregates = await LoadAggregatesAsync(dataSource, projectId, ct);

            // Shape: one group per (provider, vm_size, region), families nested.
            var groups = new List<PerfPerCostGroup>();
            var missing = new List<MissingCostSku>();

            foreach (var g in aggregates
                         .GroupBy(a => (a.Provider, a.VmSize, a.Region))
                         .OrderBy(g => g.Key.Provider, StringComparer.Ordinal)
                         .ThenBy(g => g.Key.VmSize, StringComparer.Ordinal)
                         .ThenBy(g => g.Key.Region, StringComparer.Ordinal))
            {
                var (provider, vmSize, region) = g.Key;
                var lookup = costs.Find(provider, vmSize, region);
                decimal? hourly = lookup?.Rate.HourlyUsd;

                string? costNote = null;
                if (lookup is null)
                {
                    costNote = "no price row for this SKU in shared/cloud-costs.json — value scores unavailable";
                    missing.Add(new MissingCostSku(provider, vmSize, region));
                }
                else if (!lookup.RegionMatched)
                {
                    costNote = $"priced from {lookup.Rate.Region} (no {region} row in the cost table)";
                }

                var families = g
                    .OrderBy(a => a.Family, StringComparer.Ordinal)
                    .Select(a =>
                    {
                        // thru attempts without a ThroughputMbps sample fall
                        // back to latency so the row is never silently empty —
                        // metric_label always names what was actually measured.
                        var isThroughput =
                            !PerfPerCostLogic.IsLatencyFamily(a.Family) && a.ThroughputSamples > 0;
                        return new PerfPerCostFamily(
                            Family: a.Family,
                            RunCount: a.RunCount,
                            SampleCount: isThroughput ? a.ThroughputSamples : a.LatencySamples,
                            MetricLabel: isThroughput ? "throughput_mbps" : "latency_ms",
                            Median: Round4(isThroughput ? a.MedianThroughputMbps : a.MedianLatencyMs),
                            P95Ms: isThroughput ? null : Round4(a.P95LatencyMs),
                            ValueMetric: isThroughput ? "mbps_per_dollar_hour" : "latency_cost_index",
                            ValueScore: isThroughput
                                ? PerfPerCostLogic.MbpsPerDollarHour(a.MedianThroughputMbps, hourly)
                                : PerfPerCostLogic.LatencyCostIndex(a.P95LatencyMs, hourly));
                    })
                    .ToList();

                groups.Add(new PerfPerCostGroup(
                    Provider: provider,
                    VmSize: vmSize,
                    Region: region,
                    HourlyUsd: hourly,
                    CostRegion: lookup?.Rate.Region,
                    CostSourceUrl: lookup?.Rate.SourceUrl,
                    CostAsOf: lookup?.Rate.AsOf,
                    CostNote: costNote,
                    Families: families));
            }

            if (missing.Count > 0)
            {
                log.LogWarning(
                    "perf-per-cost project={ProjectId}: {Count} tester group(s) missing from the "
                    + "cost table: {Skus} — rows shown without value scores",
                    projectId, missing.Count,
                    string.Join(", ", missing.Select(m => $"{m.Provider}/{m.VmSize}/{m.Region}")));
            }

            var providersWithData = groups.Select(x => x.Provider)
                .Distinct(StringComparer.OrdinalIgnoreCase).Count();

            // Total completed runs in the project (context: how much of the
            // fleet's history the aggregation could draw on).
            var completedRuns = await db.TestRuns.AsNoTracking()
                .CountAsync(r => r.ProjectId == projectId && r.Status == "completed", ct);

            return Results.Ok(new PerfPerCostReport(
                GeneratedAt: DateTime.UtcNow,
                CostTable: new CostTableInfo(
                    costs.AsOf,
                    costs.Disclaimer,
                    "shared/cloud-costs.json (static, hand-curated; no pricing API at runtime)"),
                Formulas: new FormulasInfo(
                    PerfPerCostLogic.Formulas.LatencyCostIndexText,
                    PerfPerCostLogic.Formulas.MbpsPerDollarHourText),
                CompletedRuns: completedRuns,
                ProvidersWithData: providersWithData,
                Groups: groups,
                MissingCostSkus: missing));
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        return app;
    }

    private static double? Round4(double? v) => v is double d ? Math.Round(d, 4) : null;

    private sealed record Aggregate(
        string Provider, string VmSize, string Region, string Family,
        int RunCount, int LatencySamples, int ThroughputSamples,
        double? MedianLatencyMs, double? P95LatencyMs, double? MedianThroughputMbps);

    /// <summary>
    /// One SQL pass: completed control-plane runs → their tester's
    /// (cloud, vm_size, region) → successful probe attempts, classified into
    /// mode families by the CASE generated from
    /// <see cref="PerfPerCostLogic.ProtocolFamily"/>. Latency per attempt is
    /// <c>COALESCE(HttpResult.TotalDurationMs, wall time)</c> — the same
    /// definition the alerting <c>RunMetricProvider</c> uses, so the report
    /// agrees with alert thresholds. Ordered-set aggregates ignore NULLs, so
    /// throughput percentiles only see attempts that measured throughput.
    /// </summary>
    private static async Task<List<Aggregate>> LoadAggregatesAsync(
        NpgsqlDataSource dataSource, string projectId, CancellationToken ct)
    {
        var familyCase = PerfPerCostLogic.FamilyCaseSql("a.Protocol");
        var sql = $"""
            SELECT t.cloud,
                   t.vm_size,
                   t.region,
                   {familyCase} AS family,
                   COUNT(DISTINCT r.id)::int AS run_count,
                   COUNT(*) FILTER (WHERE v.latency_ms IS NOT NULL)::int AS latency_samples,
                   COUNT(h.ThroughputMbps)::int AS throughput_samples,
                   PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY v.latency_ms) AS median_latency_ms,
                   PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY v.latency_ms) AS p95_latency_ms,
                   PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY h.ThroughputMbps) AS median_throughput_mbps
            FROM test_run r
            JOIN project_tester t ON t.tester_id = r.tester_id
            JOIN RequestAttempt a ON a.RunId = r.id AND a.Success
            LEFT JOIN HttpResult h ON h.AttemptId = a.AttemptId
            CROSS JOIN LATERAL (
                SELECT COALESCE(h.TotalDurationMs,
                                EXTRACT(EPOCH FROM (a.FinishedAt - a.StartedAt)) * 1000.0) AS latency_ms
            ) v
            WHERE r.project_id = $1 AND r.status = 'completed'
            GROUP BY 1, 2, 3, 4
            """;

        var rows = new List<Aggregate>();
        try
        {
            await using var cmd = dataSource.CreateCommand(sql);
            cmd.Parameters.AddWithValue(projectId);
            await using var reader = await cmd.ExecuteReaderAsync(ct);
            while (await reader.ReadAsync(ct))
            {
                rows.Add(new Aggregate(
                    Provider: reader.GetString(0),
                    VmSize: reader.GetString(1),
                    Region: reader.GetString(2),
                    Family: reader.GetString(3),
                    RunCount: reader.GetInt32(4),
                    LatencySamples: reader.GetInt32(5),
                    ThroughputSamples: reader.GetInt32(6),
                    MedianLatencyMs: reader.IsDBNull(7) ? null : reader.GetDouble(7),
                    P95LatencyMs: reader.IsDBNull(8) ? null : reader.GetDouble(8),
                    MedianThroughputMbps: reader.IsDBNull(9) ? null : reader.GetDouble(9)));
            }
        }
        catch (PostgresException ex) when (ex.SqlState == PostgresErrorCodes.UndefinedTable)
        {
            // Tester result schema not present — no probe has ever persisted
            // results to this database. Valid empty report, not an error.
        }

        return rows;
    }
}

// ── Wire shapes (snake_case, pinned by PerfPerCostContractTests) ─────────────

public sealed record PerfPerCostReport(
    [property: JsonPropertyName("generated_at")] DateTime GeneratedAt,
    [property: JsonPropertyName("cost_table")] CostTableInfo CostTable,
    [property: JsonPropertyName("formulas")] FormulasInfo Formulas,
    [property: JsonPropertyName("completed_runs")] int CompletedRuns,
    [property: JsonPropertyName("providers_with_data")] int ProvidersWithData,
    [property: JsonPropertyName("groups")] IReadOnlyList<PerfPerCostGroup> Groups,
    [property: JsonPropertyName("missing_cost_skus")] IReadOnlyList<MissingCostSku> MissingCostSkus);

public sealed record CostTableInfo(
    [property: JsonPropertyName("as_of")] string AsOf,
    [property: JsonPropertyName("disclaimer")] string Disclaimer,
    [property: JsonPropertyName("source")] string Source);

public sealed record FormulasInfo(
    [property: JsonPropertyName("latency_cost_index")] string LatencyCostIndex,
    [property: JsonPropertyName("mbps_per_dollar_hour")] string MbpsPerDollarHour);

public sealed record PerfPerCostGroup(
    [property: JsonPropertyName("provider")] string Provider,
    [property: JsonPropertyName("vm_size")] string VmSize,
    [property: JsonPropertyName("region")] string Region,
    [property: JsonPropertyName("hourly_usd")] decimal? HourlyUsd,
    [property: JsonPropertyName("cost_region")] string? CostRegion,
    [property: JsonPropertyName("cost_source_url")] string? CostSourceUrl,
    [property: JsonPropertyName("cost_as_of")] string? CostAsOf,
    [property: JsonPropertyName("cost_note")] string? CostNote,
    [property: JsonPropertyName("families")] IReadOnlyList<PerfPerCostFamily> Families);

public sealed record PerfPerCostFamily(
    [property: JsonPropertyName("family")] string Family,
    [property: JsonPropertyName("run_count")] int RunCount,
    [property: JsonPropertyName("sample_count")] int SampleCount,
    [property: JsonPropertyName("metric_label")] string MetricLabel,
    [property: JsonPropertyName("median")] double? Median,
    [property: JsonPropertyName("p95_ms")] double? P95Ms,
    [property: JsonPropertyName("value_metric")] string ValueMetric,
    [property: JsonPropertyName("value_score")] double? ValueScore);

public sealed record MissingCostSku(
    [property: JsonPropertyName("provider")] string Provider,
    [property: JsonPropertyName("vm_size")] string VmSize,
    [property: JsonPropertyName("region")] string Region);
