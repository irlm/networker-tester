using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Dispatch;
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

        // POST /api/v2/comparison-groups/{id}/launch — materialize + dispatch one
        // run per cell. For each cell: create a TestConfig (cell endpoint +
        // group base_workload + methodology), then IRunDispatcher.LaunchAsync
        // tagging the run with this group id — pending endpoints are picked up by
        // the ProvisioningOrchestrator (provision → readiness-gate → dispatch),
        // network/proxy endpoints dispatch immediately. Operator required.
        //
        // Was an unimplemented M3 stub (returned 202, created ZERO runs — the UI
        // then redirected to an empty results page; E2E follow-up 2026-07-29).
        // The dispatcher + orchestrator it waited on now exist.
        app.MapPost("/api/v2/comparison-groups/{id:guid}/launch", async (
            Guid id,
            HttpContext ctx,
            ProjectAccessChecker access,
            IRunDispatcher dispatcher,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var user = ctx.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }

            var group = await db.ComparisonGroups.AsTracking()
                .FirstOrDefaultAsync(g => g.Id == id, ct);
            if (group is null ||
                !await access.HasRoleAsync(ctx, group.ProjectId, ProjectRole.Operator, ct))
            {
                return Results.NotFound(); // 404 == no-access, don't leak existence
            }

            List<CellSpec> cells;
            try
            {
                cells = ParseCells(group.Cells);
            }
            catch (JsonException)
            {
                return ApiError.Status(StatusCodes.Status422UnprocessableEntity,
                    "comparison group has malformed cells");
            }
            if (cells.Count == 0)
            {
                return ApiError.Status(StatusCodes.Status422UnprocessableEntity,
                    "comparison group has no cells to launch");
            }

            var now = DateTime.UtcNow;
            var launched = new List<Guid>(cells.Count);
            var failures = new List<string>();
            // UNIQUE(project_id, name): the index keeps cells within one launch
            // distinct, but a RE-launch of the same group regenerates the same
            // (group, index) pairs — without a per-launch nonce every cell of a
            // retry fails on the unique constraint before it can provision.
            var launchNonce = Guid.NewGuid().ToString("N")[..4];

            for (var i = 0; i < cells.Count; i++)
            {
                var cell = cells[i];
                try
                {
                    var cfg = new Data.Entities.TestConfig
                    {
                        Id = Guid.NewGuid(),
                        ProjectId = group.ProjectId,
                        Name = CellConfigName(cell.Label, id, i, launchNonce),
                        EndpointKind = cell.EndpointKind,
                        EndpointRef = cell.EndpointRaw,
                        Workload = group.BaseWorkload,
                        Methodology = group.Methodology,
                        MaxDurationSecs = CellMaxDurationSecs(group.BaseWorkload),
                        CreatedBy = user.UserId,
                        CreatedAt = now,
                        UpdatedAt = now,
                    };
                    db.TestConfigs.Add(cfg);
                    await db.SaveChangesAsync(ct);

                    // Tag the run with THIS group so the runs list
                    // (?comparison_group_id=) and the group detail collect it.
                    var runId = await dispatcher.LaunchAsync(cfg.Id, id, cell.RunnerId, user, ct);
                    launched.Add(runId);
                }
                catch (Exception ex)
                {
                    // One bad cell must not abort the matrix — record + continue.
                    failures.Add($"{cell.Label}: {ex.Message}");
                    ctx.RequestServices.GetService<ILoggerFactory>()?
                        .CreateLogger("ComparisonGroups.launch")
                        .LogWarning(ex, "Comparison group {GroupId} cell '{Cell}' failed to launch", id, cell.Label);
                }
            }

            group.Status = launched.Count > 0 ? "running" : "failed";
            await db.SaveChangesAsync(ct);

            return Results.Accepted($"/api/v2/comparison-groups/{id}", new
            {
                launched = launched.Count,
                total = cells.Count,
                failed = failures.Count,
                errors = failures.Count > 0 ? failures : null,
            });
        }).RequireAuthorization();

        return app;
    }

    /// <summary>A comparison-group cell resolved for launch.</summary>
    internal sealed record CellSpec(string Label, string EndpointRaw, string EndpointKind, Guid? RunnerId);

    /// <summary>Per-cell test-config name: group id + cell index keep one
    /// launch's cells distinct; the per-launch nonce keeps a re-launch of the
    /// same group from tripping UNIQUE(project_id, name).</summary>
    internal static string CellConfigName(string label, Guid groupId, int index, string launchNonce)
        => $"{label} · cg-{groupId.ToString()[..8]}·{index}·{launchNonce}";

    /// <summary>Parse the group's <c>cells</c> JSON into launch specs. Each cell
    /// carries a <c>label</c>, a polymorphic <c>endpoint</c> (kind pending /
    /// network / proxy), and an optional <c>runner_id</c>.</summary>
    internal static List<CellSpec> ParseCells(string cellsJson)
    {
        using var doc = JsonDocument.Parse(cellsJson);
        var list = new List<CellSpec>();
        if (doc.RootElement.ValueKind != JsonValueKind.Array)
        {
            return list;
        }
        foreach (var cell in doc.RootElement.EnumerateArray())
        {
            if (!cell.TryGetProperty("endpoint", out var ep) || ep.ValueKind != JsonValueKind.Object)
            {
                continue; // a cell with no endpoint can't become a run
            }
            var label = cell.TryGetProperty("label", out var l) && l.ValueKind == JsonValueKind.String
                ? l.GetString() ?? "cell" : "cell";
            var kind = ep.TryGetProperty("kind", out var k) && k.ValueKind == JsonValueKind.String
                ? k.GetString() ?? "pending" : "pending";
            Guid? runnerId = cell.TryGetProperty("runner_id", out var r)
                && r.ValueKind == JsonValueKind.String && Guid.TryParse(r.GetString(), out var rid)
                ? rid : null;
            list.Add(new CellSpec(label, ep.GetRawText(), kind, runnerId));
        }
        return list;
    }

    private const int DefaultMaxDurationSecs = 900;

    /// <summary>Ceiling for a workload-derived cell deadline — guards against
    /// a malformed workload producing an unbounded run. 8h (was 6h): the full
    /// matrix workload legitimately estimates past 6h under the corrected
    /// per-unit budget.</summary>
    private const int MaxCellDurationSecs = 8 * 3600;

    /// <summary>
    /// Derive a cell's <c>max_duration_secs</c> from the group's base workload.
    /// The old fixed 900s deadline was IMPOSSIBLE for real matrix workloads —
    /// runs=100 × 26 modes needs hours, so every cell that reached the runner
    /// was killed at ~16 minutes (2026-08-03). Budget: ~4s per (run × mode)
    /// attempt (mixes ms-scale dns/tcp with multi-second pageload/throughput)
    /// plus a 10-minute fixed buffer for startup/report/upload, floored at the
    /// old default and capped at <see cref="MaxCellDurationSecs"/>. An
    /// unparseable workload falls back to the old default.
    /// </summary>
    internal static int CellMaxDurationSecs(string? baseWorkloadJson)
    {
        try
        {
            if (string.IsNullOrEmpty(baseWorkloadJson))
            {
                return DefaultMaxDurationSecs;
            }
            using var doc = JsonDocument.Parse(baseWorkloadJson);
            if (doc.RootElement.ValueKind != JsonValueKind.Object)
            {
                return DefaultMaxDurationSecs;
            }
            var runs = doc.RootElement.TryGetProperty("runs", out var r)
                       && r.ValueKind == JsonValueKind.Number && r.TryGetInt32(out var rv) && rv > 0
                ? rv
                : 10;
            var modes = doc.RootElement.TryGetProperty("modes", out var m)
                        && m.ValueKind == JsonValueKind.Array
                ? m.GetArrayLength()
                : 1;
            // 8s per (run × mode), not 4s: measured live (2026-08-04, 4 cells
            // sharing one runner), the real attempt count runs ~1.7× runs×modes
            // (payload sizes multiply throughput modes) and the slower proxies
            // (haproxy/traefik) needed >4.2s per unit — both hit the old
            // deadline at 78-85% complete while nginx/caddy squeaked through.
            var estimate = (long)runs * Math.Max(modes, 1) * 8 + 600;
            return (int)Math.Clamp(estimate, DefaultMaxDurationSecs, MaxCellDurationSecs);
        }
        catch (JsonException)
        {
            return DefaultMaxDurationSecs;
        }
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
