using System.Text.Json;
using System.Text.Json.Serialization;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;
using Npgsql;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// REST v2 read endpoints for test runs — the C# port of the Rust
/// <c>crates/networker-dashboard/src/api/test_runs.rs</c> list / get / artifact
/// handlers. JSON field names are snake_case to match the Rust
/// <c>networker_common::TestRun</c> and <c>BenchmarkArtifact</c> wire shapes so
/// the existing frontend consumes either backend unchanged.
///
/// Beyond the Rust shape, the list/detail responses add one computed field:
/// <c>result_status</c> — the shared completed-with-failures verdict
/// (<see cref="RunVerdict.ResultStatus"/>); <c>status</c> stays the raw stored
/// lifecycle value.
///
/// Mutating routes (cancel / compare) live elsewhere; <c>/attempts</c> is the
/// read route the run-detail page polls and is served here.
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

            // result_status is computed in memory (RunVerdict is not
            // EF-translatable) — `status` stays the raw stored value.
            var shaped = rows.Select(r => new
            {
                r.id,
                r.test_config_id,
                r.project_id,
                r.status,
                result_status = RunVerdict.ResultStatus(r.status, r.success_count, r.failure_count),
                r.started_at,
                r.finished_at,
                r.success_count,
                r.failure_count,
                r.error_message,
                r.artifact_id,
                r.tester_id,
                r.worker_id,
                r.last_heartbeat,
                r.created_at,
                r.comparison_group_id,
                r.config_name,
                r.endpoint_kind,
            });

            return Results.Ok(shaped);
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        // GET /api/v2/test-runs/{id} — single run detail.
        // Flat route (no {projectId}), so the ProjectMember policy can't resolve a
        // project scope. Instead: load the row, then row-level authz via
        // ProjectAccessChecker against run.ProjectId. No access → 404 (identical
        // to not-found, so the route is not an existence oracle for other
        // projects' run ids).
        app.MapGet("/api/v2/test-runs/{id:guid}", async (
            Guid id,
            HttpContext ctx,
            ProjectAccessChecker access,
            NetworkerDbContext db,
            CancellationToken ct) =>
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
                .FirstOrDefaultAsync(ct);

            if (run is null ||
                !await access.HasRoleAsync(ctx, run.project_id, ProjectRole.Viewer, ct))
            {
                return Results.NotFound();
            }

            return Results.Ok(new
            {
                run.id,
                run.test_config_id,
                run.project_id,
                run.status,
                result_status = RunVerdict.ResultStatus(
                    run.status, run.success_count, run.failure_count),
                run.started_at,
                run.finished_at,
                run.success_count,
                run.failure_count,
                run.error_message,
                run.artifact_id,
                run.tester_id,
                run.worker_id,
                run.last_heartbeat,
                run.created_at,
                run.comparison_group_id,
            });
        }).RequireAuthorization();

        // GET /api/v2/test-runs/{id}/attempts — per-attempt rows for a run.
        // The run-detail page polls this; it previously 404'd for EVERY run
        // because the route was never ported from the Rust dashboard (audit
        // F3). Semantics: 404 ONLY when the run does not exist or the caller
        // has no access (same non-oracle rule as the detail route); an
        // existing run always returns 200 with `{ "attempts": [...] }` — the
        // envelope the legacy Rust handler returned — empty when the tester
        // engine hasn't persisted probe rows for it (benchmark-style runs,
        // tester schema absent, or DB-less testers).
        //
        // Attempt rows live in the tester-owned V001 schema (RequestAttempt),
        // which is NOT part of the EF model — raw Npgsql, same pattern as
        // Alerting.RunMetricProvider / UrlTestsEndpoints.
        app.MapGet("/api/v2/test-runs/{id:guid}/attempts", async (
            Guid id,
            HttpContext ctx,
            ProjectAccessChecker access,
            NetworkerDbContext db,
            NpgsqlDataSource dataSource,
            CancellationToken ct) =>
        {
            var runProjectId = await db.TestRuns
                .AsNoTracking()
                .Where(r => r.Id == id)
                .Select(r => r.ProjectId)
                .FirstOrDefaultAsync(ct);

            if (runProjectId is null ||
                !await access.HasRoleAsync(ctx, runProjectId, ProjectRole.Viewer, ct))
            {
                return Results.NotFound();
            }

            var attempts = await LoadAttemptsAsync(dataSource, id, ct);
            return Results.Ok(new AttemptListResponse(attempts));
        }).RequireAuthorization();

        // GET /api/v2/test-runs/{id}/artifact — the BenchmarkArtifact for a run.
        // Mirrors Rust artifact_handler + db::benchmark_artifacts::get_for_run
        // (newest artifact for the run). The JSONB columns are stored as text in
        // the C# entity; we re-emit them as raw JSON (not escaped strings) so the
        // wire shape matches the Rust serde_json::Value fields.
        // Flat route: row-level authz via the parent run's ProjectId; no access
        // (or unknown run) → 404, same as a missing artifact.
        app.MapGet("/api/v2/test-runs/{id:guid}/artifact", async (
            Guid id,
            HttpContext ctx,
            ProjectAccessChecker access,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var runProjectId = await db.TestRuns
                .AsNoTracking()
                .Where(r => r.Id == id)
                .Select(r => r.ProjectId)
                .FirstOrDefaultAsync(ct);

            if (runProjectId is null ||
                !await access.HasRoleAsync(ctx, runProjectId, ProjectRole.Viewer, ct))
            {
                return Results.NotFound();
            }

            var art = await db.BenchmarkArtifacts
                .AsNoTracking()
                .Where(a => a.TestRunId == id)
                .OrderByDescending(a => a.CreatedAt)
                .FirstOrDefaultAsync(ct);

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

    /// <summary>
    /// Read the RequestAttempt rows the networker-tester engine persisted for
    /// a run (V001 tester-owned schema; unquoted identifiers fold to lowercase
    /// on both sides, matching how the tester creates the tables). A missing
    /// table (42P01 — no probe has ever written results to this database)
    /// yields an empty list, NOT an error: "no attempt data" is a valid state
    /// for an existing run. Capped defensively; ordered by sequence.
    /// </summary>
    private static async Task<List<AttemptView>> LoadAttemptsAsync(
        NpgsqlDataSource dataSource, Guid runId, CancellationToken ct)
    {
        const string sql = """
            SELECT AttemptId, Protocol, SequenceNum, StartedAt, FinishedAt,
                   Success, ErrorMessage, RetryCount
            FROM RequestAttempt
            WHERE RunId = $1
            ORDER BY SequenceNum, StartedAt
            LIMIT 10000
            """;

        var attempts = new List<AttemptView>();
        try
        {
            await using var cmd = dataSource.CreateCommand(sql);
            cmd.Parameters.AddWithValue(runId);
            await using var reader = await cmd.ExecuteReaderAsync(ct);
            while (await reader.ReadAsync(ct))
            {
                attempts.Add(new AttemptView(
                    AttemptId: reader.GetGuid(0),
                    Protocol: reader.GetString(1),
                    SequenceNum: reader.GetInt32(2),
                    StartedAt: reader.GetDateTime(3),
                    FinishedAt: reader.IsDBNull(4) ? null : reader.GetDateTime(4),
                    Success: reader.GetBoolean(5),
                    // Tester-written text can carry ANSI codes (the Rust side
                    // owns that write path) — scrub on emit so API consumers
                    // get clean data (audit F8).
                    ErrorMessage: reader.IsDBNull(6) ? null : AnsiText.Strip(reader.GetString(6)),
                    RetryCount: reader.GetInt32(7)));
            }
        }
        catch (PostgresException ex) when (ex.SqlState == PostgresErrorCodes.UndefinedTable)
        {
            // Tester result schema not present — an existing run with no
            // recorded attempts, i.e. an empty (200) list.
        }

        return attempts;
    }
}

/// <summary>
/// The pinned wire shape of <c>GET /api/v2/test-runs/{id}/attempts</c> — the
/// <c>{ "attempts": [...] }</c> envelope the legacy Rust handler returned and
/// the frontend client types. Pinned by <c>TestRunsContractTests</c>.
/// </summary>
public sealed record AttemptListResponse(
    [property: JsonPropertyName("attempts")] IReadOnlyList<AttemptView> Attempts);

/// <summary>
/// One attempt row — mirrors the tester's <c>RequestAttempt</c> table and the
/// frontend <c>Attempt</c> type (<c>dashboard/src/api/types.ts</c>).
/// </summary>
public sealed record AttemptView(
    [property: JsonPropertyName("attempt_id")] Guid AttemptId,
    [property: JsonPropertyName("protocol")] string Protocol,
    [property: JsonPropertyName("sequence_num")] int SequenceNum,
    [property: JsonPropertyName("started_at")] DateTime StartedAt,
    [property: JsonPropertyName("finished_at")] DateTime? FinishedAt,
    [property: JsonPropertyName("success")] bool Success,
    [property: JsonPropertyName("error_message")] string? ErrorMessage,
    [property: JsonPropertyName("retry_count")] int RetryCount);
