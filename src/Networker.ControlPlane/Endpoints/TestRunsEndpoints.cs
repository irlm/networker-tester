using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// REST v2 read endpoints for test runs — the C# port of the Rust
/// <c>crates/networker-dashboard/src/api/test_runs.rs</c> list / get / artifact
/// handlers. JSON field names are snake_case to match the Rust
/// <c>networker_common::TestRun</c> and <c>BenchmarkArtifact</c> wire shapes so
/// the existing frontend consumes either backend unchanged.
///
/// Phase-2 M1 scope: read-only. Mutating routes (cancel / compare / attempts)
/// are intentionally not ported here.
/// </summary>
public static class TestRunsEndpoints
{
    private const int DefaultLimit = 50;
    private const int MaxLimit = 200;

    public static IEndpointRouteBuilder MapTestRunsEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/v2/projects/{projectId}/test-runs — list with filters. Joins
        // TestRun→TestConfig for the config name and endpoint_kind. Mirrors the
        // Rust list_handler + db::test_runs::list, but folds in the config join
        // and the endpoint_kind / before-cursor filters the Rust DB layer had
        // left as TODOs.
        app.MapGet("/api/v2/projects/{projectId}/test-runs", async (
            string projectId,
            string? status,
            string? endpoint_kind,
            bool? has_artifact,
            Guid? comparison_group_id,
            int? limit,
            DateTime? before,
            NetworkerDbContext db) =>
        {
            var take = Math.Clamp(limit ?? DefaultLimit, 1, MaxLimit);

            var query = db.TestRuns
                .AsNoTracking()
                .Where(r => r.ProjectId == projectId);

            if (!string.IsNullOrEmpty(status))
            {
                query = query.Where(r => r.Status == status);
            }

            if (has_artifact is bool wantArtifact)
            {
                query = wantArtifact
                    ? query.Where(r => r.ArtifactId != null)
                    : query.Where(r => r.ArtifactId == null);
            }

            if (comparison_group_id is Guid cgid)
            {
                query = query.Where(r => r.ComparisonGroupId == cgid);
            }

            if (before is DateTime cursor)
            {
                // `before` is a keyset cursor over created_at DESC (exclusive).
                query = query.Where(r => r.CreatedAt < cursor);
            }

            // endpoint_kind lives on the config, so filter through the relation.
            if (!string.IsNullOrEmpty(endpoint_kind))
            {
                query = query.Where(r => r.TestConfig.EndpointKind == endpoint_kind);
            }

            var rows = await query
                .OrderByDescending(r => r.CreatedAt)
                .Take(take)
                .Select(r => new
                {
                    id = r.Id,
                    test_config_id = r.TestConfigId,
                    project_id = r.ProjectId,
                    status = r.Status,
                    started_at = r.StartedAt,
                    finished_at = r.FinishedAt,
                    success_count = r.SuccessCount,
                    failure_count = r.FailureCount,
                    error_message = r.ErrorMessage,
                    artifact_id = r.ArtifactId,
                    tester_id = r.TesterId,
                    worker_id = r.WorkerId,
                    last_heartbeat = r.LastHeartbeat,
                    created_at = r.CreatedAt,
                    comparison_group_id = r.ComparisonGroupId,
                    // Extra denormalized fields the Runs table needs; the join is
                    // why this endpoint is "fuller" than the base TestRun shape.
                    config_name = r.TestConfig.Name,
                    endpoint_kind = r.TestConfig.EndpointKind,
                })
                .ToListAsync();

            return Results.Ok(rows);
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        // GET /api/v2/test-runs/{id} — single run detail.
        // NOTE: flat route has no {projectId}, so the ProjectMember policy can't
        // resolve a project scope. Requires authentication only. FOLLOW-UP: add a
        // row-level project-membership check (load run.ProjectId, verify caller is
        // a member) once M0's project-scope helper is reusable off-path.
        app.MapGet("/api/v2/test-runs/{id:guid}", async (Guid id, NetworkerDbContext db) =>
        {
            var run = await db.TestRuns
                .AsNoTracking()
                .Where(r => r.Id == id)
                .Select(r => new
                {
                    id = r.Id,
                    test_config_id = r.TestConfigId,
                    project_id = r.ProjectId,
                    status = r.Status,
                    started_at = r.StartedAt,
                    finished_at = r.FinishedAt,
                    success_count = r.SuccessCount,
                    failure_count = r.FailureCount,
                    error_message = r.ErrorMessage,
                    artifact_id = r.ArtifactId,
                    tester_id = r.TesterId,
                    worker_id = r.WorkerId,
                    last_heartbeat = r.LastHeartbeat,
                    created_at = r.CreatedAt,
                    comparison_group_id = r.ComparisonGroupId,
                })
                .FirstOrDefaultAsync();

            return run is null ? Results.NotFound() : Results.Ok(run);
        }).RequireAuthorization();

        // GET /api/v2/test-runs/{id}/artifact — the BenchmarkArtifact for a run.
        // Mirrors Rust artifact_handler + db::benchmark_artifacts::get_for_run
        // (newest artifact for the run). The JSONB columns are stored as text in
        // the C# entity; we re-emit them as raw JSON (not escaped strings) so the
        // wire shape matches the Rust serde_json::Value fields.
        app.MapGet("/api/v2/test-runs/{id:guid}/artifact", async (Guid id, NetworkerDbContext db) =>
        {
            var art = await db.BenchmarkArtifacts
                .AsNoTracking()
                .Where(a => a.TestRunId == id)
                .OrderByDescending(a => a.CreatedAt)
                .FirstOrDefaultAsync();

            if (art is null)
            {
                return Results.NotFound();
            }

            return Results.Ok(new
            {
                id = art.Id,
                test_run_id = art.TestRunId,
                environment = RawJson(art.Environment),
                methodology = RawJson(art.Methodology),
                launches = RawJson(art.Launches),
                cases = RawJson(art.Cases),
                samples = RawJsonOrNull(art.Samples),
                summaries = RawJson(art.Summaries),
                data_quality = RawJson(art.DataQuality),
                created_at = art.CreatedAt,
            });
        }).RequireAuthorization();

        return app;
    }

    // Parse a JSONB-as-text column into a JsonElement so it serializes as raw
    // JSON. Falls back to the original text as a JSON string if it isn't valid
    // JSON (defensive; the DB constraint should guarantee valid JSON).
    private static object RawJson(string value)
    {
        try
        {
            using var doc = JsonDocument.Parse(value);
            return doc.RootElement.Clone();
        }
        catch (JsonException)
        {
            return value;
        }
    }

    private static object? RawJsonOrNull(string? value)
        => value is null ? null : RawJson(value);
}
