using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Read endpoint for detected benchmark regressions — the rows
/// <see cref="Provisioning.BenchmarkRegressionDetector"/> writes on run
/// completion (V047 <c>benchmark_regression</c>). Serves the dashboard's
/// Benchmark Regressions page; snake_case wire shape.
/// </summary>
public static class BenchmarkRegressionsEndpoints
{
    private const int DefaultLimit = 100;
    private const int MaxLimit = 500;

    public static IEndpointRouteBuilder MapBenchmarkRegressionsEndpoints(
        this IEndpointRouteBuilder app)
    {
        // GET /api/projects/{projectId}/benchmark-regressions?limit= — newest
        // first, joined to the config for its display name (member).
        app.MapGet("/api/projects/{projectId}/benchmark-regressions", async (
            string projectId,
            int? limit,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var take = Math.Clamp(limit ?? DefaultLimit, 1, MaxLimit);

            var rows = await db.BenchmarkRegressions
                .AsNoTracking()
                .Where(r => r.TestConfig.ProjectId == projectId)
                .OrderByDescending(r => r.DetectedAt)
                .ThenByDescending(r => r.RegressionId)
                .Take(take)
                .Select(r => new
                {
                    regression_id = r.RegressionId,
                    config_id = r.TestConfigId,
                    config_name = r.TestConfig.Name,
                    run_id = r.TestRunId,
                    baseline_run_id = r.BaselineRunId,
                    case_id = r.CaseId,
                    metric = r.Metric,
                    metric_unit = r.MetricUnit,
                    baseline_value = r.BaselineValue,
                    current_value = r.CurrentValue,
                    delta_percent = r.DeltaPercent,
                    severity = r.Severity,
                    detected_at = r.DetectedAt,
                })
                .ToListAsync(ct);

            return Results.Ok(rows);
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        return app;
    }
}
