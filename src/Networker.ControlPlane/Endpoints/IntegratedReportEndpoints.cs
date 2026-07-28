using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Reports.Documents;
using Networker.Data;
using Npgsql;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// GET /api/projects/{projectId}/reports/integrated?format=html|md|docx|pdf —
/// the project-level <b>Integrated Test Report</b>: every test result in one
/// branded, exportable document — executive summary (KPIs, verdict,
/// runs-by-status), results per test, per-protocol latency distributions, the
/// condensed application-vs-network and perf-per-cost analyses, and a
/// recent-runs table.
///
/// <para>Document-only (default <c>html</c>; <c>json</c> is rejected — the
/// machine-readable shapes are the standalone report/list routes). The two
/// analysis sections are produced by the SAME <c>BuildReportAsync</c> helpers
/// the standalone routes use, so the integrated view cannot disagree with
/// them. Probe aggregates read the tester-owned V001 schema via raw Npgsql
/// (42P01 → empty, never an error), the same pattern as
/// <see cref="PerfPerCostEndpoints"/>. RBAC: member-read.</para>
/// </summary>
public static class IntegratedReportEndpoints
{
    public static IEndpointRouteBuilder MapIntegratedReportEndpoints(this IEndpointRouteBuilder app)
    {
        app.MapGet("/api/projects/{projectId}/reports/integrated", async (
            string projectId,
            string? format,
            NetworkerDbContext db,
            NpgsqlDataSource dataSource,
            ReportExporterResolver exporters,
            ILoggerFactory loggerFactory,
            CancellationToken ct) =>
        {
            ReportFormat fmt;
            if (string.IsNullOrWhiteSpace(format))
            {
                fmt = ReportFormat.Html;
            }
            else if (!ReportFormats.TryParse(format, out fmt) || fmt == ReportFormat.Json)
            {
                return ReportExport.BadFormat(format, exporters);
            }

            var log = loggerFactory.CreateLogger("IntegratedReport");

            var projectName = await db.Projects.AsNoTracking()
                .Where(p => p.ProjectId == projectId && p.DeletedAt == null)
                .Select(p => p.Name)
                .FirstOrDefaultAsync(ct);
            if (projectName is null)
            {
                return Results.NotFound();
            }

            var input = await LoadInputAsync(db, dataSource, projectId, projectName, log, ct);

            return ReportExport.Deliver(exporters, fmt, IntegratedReportDocument.Build(input),
                fileBase: $"integrated-report-{ReportExport.SafeFileBase(projectId)}", requested: format);
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        return app;
    }

    private static async Task<IntegratedReportInput> LoadInputAsync(
        NetworkerDbContext db, NpgsqlDataSource dataSource, string projectId, string projectName,
        ILogger log, CancellationToken ct)
    {
        // ── Control-plane side (EF) ──────────────────────────────────────────
        var statuses = await db.TestRuns.AsNoTracking()
            .Where(r => r.ProjectId == projectId)
            .GroupBy(r => r.Status)
            .Select(g => new RunStatusCount(g.Key, g.Count()))
            .ToListAsync(ct);

        var firstRunAt = await db.TestRuns.AsNoTracking()
            .Where(r => r.ProjectId == projectId)
            .MinAsync(r => (DateTime?)r.CreatedAt, ct);
        var lastRunAt = await db.TestRuns.AsNoTracking()
            .Where(r => r.ProjectId == projectId)
            .MaxAsync(r => (DateTime?)r.CreatedAt, ct);

        var configs = await db.TestConfigs.AsNoTracking()
            .Where(c => c.ProjectId == projectId)
            .Select(c => new { c.Id, c.Name, c.Workload })
            .ToListAsync(ct);

        var runsByConfig = await db.TestRuns.AsNoTracking()
            .Where(r => r.ProjectId == projectId)
            .GroupBy(r => r.TestConfigId)
            .Select(g => new { ConfigId = g.Key, Count = g.Count(), Last = g.Max(r => (DateTime?)r.CreatedAt) })
            .ToDictionaryAsync(x => x.ConfigId, ct);

        var recentRuns = await db.TestRuns.AsNoTracking()
            .Where(r => r.ProjectId == projectId)
            .OrderByDescending(r => r.CreatedAt)
            .Take(IntegratedReportDocument.MaxRecentRuns)
            .Select(r => new RecentRunRow(
                r.Id, r.TestConfig.Name, r.Status, r.StartedAt, r.FinishedAt,
                r.SuccessCount, r.FailureCount))
            .ToListAsync(ct);

        // ── Probe side (tester-owned V001 schema, raw Npgsql) ────────────────
        var attemptsByConfig = await LoadConfigAttemptStatsAsync(dataSource, projectId, ct);
        var protocols = await LoadProtocolStatsAsync(dataSource, projectId, ct);

        var configResults = configs
            .Select(c =>
            {
                runsByConfig.TryGetValue(c.Id, out var runs);
                attemptsByConfig.TryGetValue(c.Id, out var att);
                return new ConfigResult(
                    ConfigId: c.Id,
                    Name: c.Name,
                    Workload: c.Workload,
                    RunCount: runs?.Count ?? 0,
                    LastRunAt: runs?.Last,
                    AttemptCount: att?.Attempts ?? 0,
                    OkCount: att?.Ok ?? 0,
                    P50Ms: att?.P50,
                    P95Ms: att?.P95);
            })
            // Configs that never ran carry no results — keep the report about
            // test RESULTS (the config catalog lives in the dashboard).
            .Where(c => c.RunCount > 0)
            .ToList();

        // ── Analysis sections: the exact same computations the standalone
        //    routes serve. ─────────────────────────────────────────────────────
        var appNetwork = await AppNetworkEndpoints.BuildReportAsync(dataSource, projectId, null, log, ct);
        var perfPerCost = await PerfPerCostEndpoints.BuildReportAsync(db, dataSource, projectId, log, ct);

        return new IntegratedReportInput(
            ProjectId: projectId,
            ProjectName: projectName,
            RunStatuses: statuses,
            FirstRunAt: firstRunAt,
            LastRunAt: lastRunAt,
            Configs: configResults,
            Protocols: protocols,
            AppNetwork: appNetwork,
            PerfPerCost: perfPerCost,
            RecentRuns: recentRuns);
    }

    private sealed record ConfigAttemptStats(int Attempts, int Ok, double? P50, double? P95);

    /// <summary>Shared FROM/WHERE: every probe attempt of the project's
    /// completed runs, with the same latency definition the perf-per-cost
    /// report and alerting use (HttpResult total, else wall time).</summary>
    private const string AttemptBase = """
        FROM test_run r
        JOIN RequestAttempt a ON a.RunId = r.id
        LEFT JOIN HttpResult h ON h.AttemptId = a.AttemptId
        CROSS JOIN LATERAL (
            SELECT COALESCE(h.TotalDurationMs,
                            EXTRACT(EPOCH FROM (a.FinishedAt - a.StartedAt)) * 1000.0) AS latency_ms
        ) v
        WHERE r.project_id = $1 AND r.status = 'completed'
        """;

    private static async Task<Dictionary<Guid, ConfigAttemptStats>> LoadConfigAttemptStatsAsync(
        NpgsqlDataSource dataSource, string projectId, CancellationToken ct)
    {
        var sql = """
            SELECT r.test_config_id,
                   COUNT(*)::int                                AS attempts,
                   COUNT(*) FILTER (WHERE a.Success)::int       AS ok,
                   PERCENTILE_CONT(0.5)  WITHIN GROUP (ORDER BY v.latency_ms) FILTER (WHERE a.Success) AS p50,
                   PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY v.latency_ms) FILTER (WHERE a.Success) AS p95
            """ + "\n" + AttemptBase + "\nGROUP BY 1";

        var rows = new Dictionary<Guid, ConfigAttemptStats>();
        try
        {
            await using var cmd = dataSource.CreateCommand(sql);
            cmd.Parameters.AddWithValue(projectId);
            await using var reader = await cmd.ExecuteReaderAsync(ct);
            while (await reader.ReadAsync(ct))
            {
                rows[reader.GetGuid(0)] = new ConfigAttemptStats(
                    Attempts: reader.GetInt32(1),
                    Ok: reader.GetInt32(2),
                    P50: Nullable(reader, 3),
                    P95: Nullable(reader, 4));
            }
        }
        catch (PostgresException ex) when (ex.SqlState == PostgresErrorCodes.UndefinedTable)
        {
            // Tester probe schema absent — valid empty aggregates, not an error.
        }
        return rows;
    }

    private static async Task<List<ProtocolResult>> LoadProtocolStatsAsync(
        NpgsqlDataSource dataSource, string projectId, CancellationToken ct)
    {
        var sql = """
            SELECT LOWER(a.Protocol),
                   COUNT(*)::int                          AS attempts,
                   COUNT(*) FILTER (WHERE a.Success)::int AS ok,
                   MIN(v.latency_ms) FILTER (WHERE a.Success)                                          AS min_ms,
                   PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY v.latency_ms) FILTER (WHERE a.Success) AS p25,
                   PERCENTILE_CONT(0.5)  WITHIN GROUP (ORDER BY v.latency_ms) FILTER (WHERE a.Success) AS p50,
                   PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY v.latency_ms) FILTER (WHERE a.Success) AS p75,
                   PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY v.latency_ms) FILTER (WHERE a.Success) AS p95,
                   MAX(v.latency_ms) FILTER (WHERE a.Success)                                          AS max_ms
            """ + "\n" + AttemptBase + "\nGROUP BY 1";

        var rows = new List<ProtocolResult>();
        try
        {
            await using var cmd = dataSource.CreateCommand(sql);
            cmd.Parameters.AddWithValue(projectId);
            await using var reader = await cmd.ExecuteReaderAsync(ct);
            while (await reader.ReadAsync(ct))
            {
                rows.Add(new ProtocolResult(
                    Protocol: reader.GetString(0),
                    AttemptCount: reader.GetInt32(1),
                    OkCount: reader.GetInt32(2),
                    MinMs: Nullable(reader, 3),
                    P25Ms: Nullable(reader, 4),
                    P50Ms: Nullable(reader, 5),
                    P75Ms: Nullable(reader, 6),
                    P95Ms: Nullable(reader, 7),
                    MaxMs: Nullable(reader, 8)));
            }
        }
        catch (PostgresException ex) when (ex.SqlState == PostgresErrorCodes.UndefinedTable)
        {
            // Tester probe schema absent — valid empty aggregates.
        }
        return rows;
    }

    private static double? Nullable(NpgsqlDataReader reader, int ordinal) =>
        reader.IsDBNull(ordinal) ? null : reader.GetDouble(ordinal);
}
