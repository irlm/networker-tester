using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// REST v2 write + read endpoints for comparison groups — the C# port of the Rust
/// <c>crates/networker-dashboard/src/api/comparison_groups.rs</c> handlers
/// (create / list / get / launch). JSON field names are snake_case to match the
/// Rust <c>networker_common::ComparisonGroup</c> wire shape.
///
/// A comparison group batches N test runs that share a common <c>base_workload</c>
/// but vary endpoint / runner across <c>cells</c>. Each cell is meant to expand
/// into a TestConfig + queued TestRun pair (see the launch TODO). The polymorphic
/// <c>base_workload</c> / <c>methodology</c> / <c>cells</c> fields are stored as
/// JSONB (text in the EF entity) and re-emitted as raw JSON, exactly like
/// <see cref="TestConfigsEndpoints"/>.
///
/// Phase-2 M3 scope: CRUD only. The per-cell TestConfig+TestRun fan-out and the
/// dispatch of queued runs both need the run dispatcher (built in parallel), so
/// create persists just the group row and launch is an endpoint shell returning
/// 202 (see the TODOs).
/// </summary>
public static class ComparisonGroupsEndpoints
{
    private const int ListLimit = 200;

    public static IEndpointRouteBuilder MapComparisonGroupsEndpoints(this IEndpointRouteBuilder app)
    {
        // POST /api/v2/projects/{projectId}/comparison-groups — create.
        // Mirrors Rust create_handler (ProjectOperator). Rust also fans each cell
        // out into a TestConfig + queued TestRun; that fan-out needs the run
        // dispatcher, so M3 persists only the group row (status = "pending") and
        // returns it with an empty runs[] (see launch TODO for the fan-out).
        app.MapPost("/api/v2/projects/{projectId}/comparison-groups", async (
            string projectId,
            CreateComparisonGroupRequest body,
            HttpContext ctx,
            NetworkerDbContext db) =>
        {
            if (string.IsNullOrWhiteSpace(body.name))
            {
                return Results.BadRequest();
            }
            if (body.base_workload is null || body.base_workload.Value.ValueKind == JsonValueKind.Null)
            {
                return Results.BadRequest();
            }
            // Rust rejects an empty cell matrix (StatusCode::BAD_REQUEST).
            if (body.cells is null || body.cells.Value.ValueKind != JsonValueKind.Array ||
                body.cells.Value.GetArrayLength() == 0)
            {
                return Results.BadRequest();
            }

            var user = ctx.GetAuthUser();

            var row = new Data.Entities.ComparisonGroup
            {
                Id = Guid.NewGuid(),
                ProjectId = projectId,
                Name = body.name,
                // base_workload / methodology / cells are polymorphic JSON; store verbatim.
                BaseWorkload = body.base_workload.Value.GetRawText(),
                Methodology = body.methodology is null || body.methodology.Value.ValueKind == JsonValueKind.Null
                    ? null
                    : body.methodology.Value.GetRawText(),
                Cells = body.cells.Value.GetRawText(),
                Status = "pending",
                CreatedBy = user?.UserId,
                CreatedAt = DateTime.UtcNow,
            };

            db.ComparisonGroups.Add(row);
            await db.SaveChangesAsync();

            // TODO(M3): for each cell, create a TestConfig sharing base_workload/
            // methodology + a queued TestRun (comparison_group_id = row.Id) via
            // IRunDispatcher, then return them in runs[]. Deferred until the run
            // dispatcher lands; for now runs[] is empty.
            return Results.Ok(ToDetailDto(row, runs: []));
        }).RequireAuthorization(AuthPolicies.ProjectOperator);

        // GET /api/v2/projects/{projectId}/comparison-groups — list.
        // Mirrors Rust list_handler + db::comparison_groups::list (ProjectMember).
        app.MapGet("/api/v2/projects/{projectId}/comparison-groups", async (
            string projectId,
            NetworkerDbContext db) =>
        {
            var rows = await db.ComparisonGroups
                .AsNoTracking()
                .Where(g => g.ProjectId == projectId)
                .OrderByDescending(g => g.CreatedAt)
                .Take(ListLimit)
                .ToListAsync();

            return Results.Ok(rows.Select(ToGroupDto));
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        // GET /api/v2/comparison-groups/{id} — detail (incl. run_ids via runs[]).
        // Mirrors Rust get_handler + db::comparison_groups::get / get_runs. Flat
        // route (no {projectId}): row-level authz via ProjectAccessChecker against
        // group.ProjectId (Viewer). No access → 404, identical to not-found.
        app.MapGet("/api/v2/comparison-groups/{id:guid}", async (
            Guid id,
            HttpContext ctx,
            ProjectAccessChecker access,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var group = await db.ComparisonGroups
                .AsNoTracking()
                .FirstOrDefaultAsync(g => g.Id == id, ct);
            if (group is null ||
                !await access.HasRoleAsync(ctx, group.ProjectId, ProjectRole.Viewer, ct))
            {
                return Results.NotFound();
            }

            // run_ids: the TestRuns linked to this group.
            var runs = await db.TestRuns
                .AsNoTracking()
                .Where(r => r.ComparisonGroupId == id)
                .OrderBy(r => r.CreatedAt)
                .ToListAsync(ct);

            return Results.Ok(ToDetailDto(group, runs.Select(ToRunDto).ToArray()));
        }).RequireAuthorization();

        // POST /api/v2/comparison-groups/{id}/launch — dispatch queued runs.
        // In Rust this marks the group "running" and dispatch_or_provisions each
        // queued run. That needs the run dispatcher (built in parallel), so M3
        // ships only the endpoint shell: validate the group exists, then 202.
        // Flat route: row-level authz via ProjectAccessChecker against
        // group.ProjectId — launching is a mutation, so Operator (not Viewer) is
        // required. No access → 404, identical to not-found.
        app.MapPost("/api/v2/comparison-groups/{id:guid}/launch", async (
            Guid id,
            HttpContext ctx,
            ProjectAccessChecker access,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var groupProjectId = await db.ComparisonGroups
                .AsNoTracking()
                .Where(g => g.Id == id)
                .Select(g => g.ProjectId)
                .FirstOrDefaultAsync(ct);
            if (groupProjectId is null ||
                !await access.HasRoleAsync(ctx, groupProjectId, ProjectRole.Operator, ct))
            {
                return Results.NotFound();
            }

            // TODO(M3): dispatch each config variant via IRunDispatcher — mark the
            // group "running" and dispatch (or provision) every still-queued run in
            // the group. Deferred until the run dispatcher lands; for now we accept
            // the request without launching.
            return Results.Accepted();
        }).RequireAuthorization();

        return app;
    }

    // Shape a ComparisonGroup entity into the snake_case wire DTO matching the Rust
    // networker_common::ComparisonGroup. base_workload / methodology / cells are
    // re-emitted as raw JSON.
    private static object ToGroupDto(Data.Entities.ComparisonGroup g) => new
    {
        id = g.Id,
        project_id = g.ProjectId,
        name = g.Name,
        base_workload = RawJson(g.BaseWorkload),
        methodology = RawJsonOrNull(g.Methodology),
        cells = RawJson(g.Cells),
        status = g.Status,
        created_by = g.CreatedBy,
        created_at = g.CreatedAt,
    };

    // Detail = the flattened group fields + a runs[] array (Rust ComparisonGroupDetail
    // uses #[serde(flatten)] on the group, so the run list sits alongside the group
    // fields at the top level).
    private static object ToDetailDto(Data.Entities.ComparisonGroup g, object[] runs) => new
    {
        id = g.Id,
        project_id = g.ProjectId,
        name = g.Name,
        base_workload = RawJson(g.BaseWorkload),
        methodology = RawJsonOrNull(g.Methodology),
        cells = RawJson(g.Cells),
        status = g.Status,
        created_by = g.CreatedBy,
        created_at = g.CreatedAt,
        runs,
    };

    // Shape a TestRun into the snake_case wire DTO matching networker_common::TestRun.
    private static object ToRunDto(Data.Entities.TestRun r) => new
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
    };

    // Parse a JSONB-as-text column into a JsonElement so it serializes as raw JSON.
    // Falls back to the original text if it isn't valid JSON (defensive) — matches
    // TestConfigsEndpoints.RawJson.
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

    // ── request body (snake_case JSON, matching Rust CreateComparisonGroupRequest) ──
    //
    // base_workload / methodology / cells are polymorphic — accepted as raw
    // JsonElement and stored verbatim, mirroring the JSONB round-trip elsewhere.
    public sealed record CreateComparisonGroupRequest(
        string? name,
        JsonElement? base_workload,
        JsonElement? methodology,
        JsonElement? cells);
}
