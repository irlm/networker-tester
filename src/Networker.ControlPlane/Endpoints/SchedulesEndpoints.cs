using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Dispatch;
using Networker.Data;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// REST v2 write + read endpoints for test schedules — the C# port of the Rust
/// <c>crates/networker-dashboard/src/api/schedules.rs</c> handlers
/// (create / list / get / patch / delete / trigger). JSON field names are
/// snake_case to match the Rust <c>networker_common::TestSchedule</c> wire shape.
///
/// A schedule links a <c>test_config</c> to a cron expression + timezone; a
/// separate scheduler service (out of M3 scope) evaluates the cron and fires
/// runs, stamping <c>last_fired_at</c> / <c>last_run_id</c> / <c>next_fire_at</c>.
/// Here we only own the CRUD: cron_expr / timezone are stored verbatim (no
/// server-side cron parsing — the Rust CRUD path likewise never computes
/// <c>next_fire_at</c>, it is left to the scheduler), and <c>next_fire_at</c> is
/// echoed back from whatever the scheduler last persisted.
///
/// Phase-2 M3 scope: CRUD only. The run-dispatch side of <c>trigger</c> is
/// deferred until the run dispatcher lands (see the TODO in the trigger handler).
/// </summary>
public static class SchedulesEndpoints
{
    public static IEndpointRouteBuilder MapSchedulesEndpoints(this IEndpointRouteBuilder app)
    {
        // POST /api/v2/projects/{projectId}/schedules — create.
        // Mirrors Rust create_handler + db::test_schedules::create (ProjectOperator).
        app.MapPost("/api/v2/projects/{projectId}/schedules", async (
            string projectId,
            CreateScheduleRequest body,
            HttpContext ctx,
            NetworkerDbContext db) =>
        {
            if (string.IsNullOrWhiteSpace(body.test_config_id) ||
                !Guid.TryParse(body.test_config_id, out var testConfigId))
            {
                return Results.BadRequest();
            }
            if (string.IsNullOrWhiteSpace(body.cron_expr))
            {
                return Results.BadRequest();
            }

            var user = ctx.GetAuthUser();

            var row = new Data.Entities.TestSchedule
            {
                Id = Guid.NewGuid(),
                TestConfigId = testConfigId,
                ProjectId = projectId,
                // cron_expr / timezone are persisted verbatim; cron validation and
                // next_fire_at computation are the scheduler's responsibility.
                CronExpr = body.cron_expr,
                Timezone = string.IsNullOrWhiteSpace(body.timezone) ? "UTC" : body.timezone,
                Enabled = body.enabled ?? true,
                CreatedBy = user?.UserId,
                CreatedAt = DateTime.UtcNow,
            };

            db.TestSchedules.Add(row);
            await db.SaveChangesAsync();

            return Results.Ok(ToDto(row));
        }).RequireAuthorization(AuthPolicies.ProjectOperator);

        // GET /api/v2/projects/{projectId}/schedules — list.
        // Mirrors Rust list_handler + db::test_schedules::list (ProjectMember).
        app.MapGet("/api/v2/projects/{projectId}/schedules", async (
            string projectId,
            NetworkerDbContext db) =>
        {
            var rows = await db.TestSchedules
                .AsNoTracking()
                .Where(s => s.ProjectId == projectId)
                .OrderByDescending(s => s.CreatedAt)
                .ToListAsync();

            return Results.Ok(rows.Select(ToDto));
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        // GET /api/v2/schedules/{id} — detail (incl. next_fire_at).
        // Mirrors Rust get via db::test_schedules::get. Flat route has no
        // {projectId}, so it can only require authentication (same limitation the
        // TestConfigs flat GET documents; row-level membership check is a follow-up).
        app.MapGet("/api/v2/schedules/{id:guid}", async (Guid id, NetworkerDbContext db) =>
        {
            var row = await db.TestSchedules
                .AsNoTracking()
                .FirstOrDefaultAsync(s => s.Id == id);

            return row is null ? Results.NotFound() : Results.Ok(ToDto(row));
        }).RequireAuthorization();

        // PATCH /api/v2/schedules/{id} — update cron_expr / timezone / enabled.
        // Mirrors Rust patch_handler + db::test_schedules::update. Every field is
        // optional; only supplied (non-null) fields are applied. next_fire_at is
        // deliberately NOT touched here (Rust passes next_fire_at: None) — the
        // scheduler recomputes it after a cron/timezone change.
        app.MapPatch("/api/v2/schedules/{id:guid}", async (
            Guid id,
            UpdateScheduleRequest body,
            HttpContext ctx,
            NetworkerDbContext db,
            ProjectAccessChecker access,
            CancellationToken ct) =>
        {
            var row = await db.TestSchedules.FirstOrDefaultAsync(s => s.Id == id, ct);
            if (row is null || !await access.HasRoleAsync(ctx, row.ProjectId, ProjectRole.Operator, ct))
            {
                return Results.NotFound();
            }

            if (body.cron_expr is not null)
            {
                row.CronExpr = body.cron_expr;
            }
            if (body.timezone is not null)
            {
                row.Timezone = body.timezone;
            }
            if (body.enabled is not null)
            {
                row.Enabled = body.enabled.Value;
            }

            await db.SaveChangesAsync();

            return Results.Ok(ToDto(row));
        }).RequireAuthorization();

        // DELETE /api/v2/schedules/{id} — 204 on success, 404 if absent.
        // Mirrors Rust delete_handler + db::test_schedules::delete.
        app.MapDelete("/api/v2/schedules/{id:guid}", async (
            Guid id,
            HttpContext ctx,
            NetworkerDbContext db,
            ProjectAccessChecker access,
            CancellationToken ct) =>
        {
            var row = await db.TestSchedules.FirstOrDefaultAsync(s => s.Id == id, ct);
            if (row is null || !await access.HasRoleAsync(ctx, row.ProjectId, ProjectRole.Operator, ct))
            {
                return Results.NotFound();
            }

            db.TestSchedules.Remove(row);
            await db.SaveChangesAsync(ct);

            return Results.NoContent();
        }).RequireAuthorization();

        // POST /api/v2/schedules/{id}/trigger — fire the schedule's config now.
        // Creates a queued test_run from the linked config via the dispatcher,
        // stamps last_fired_at/last_run_id, and returns 200 with the FULL
        // serialized test_run row, re-read after the dispatch attempt (the
        // frontend inserts this response straight into the runs list; status may
        // already be running/provisioning). Rust trigger_handler.
        app.MapPost("/api/v2/schedules/{id:guid}/trigger", async (
            Guid id,
            HttpContext ctx,
            NetworkerDbContext db,
            IRunDispatcher dispatcher,
            ProjectAccessChecker access,
            CancellationToken ct) =>
        {
            var schedule = await db.TestSchedules.FirstOrDefaultAsync(s => s.Id == id, ct);
            if (schedule is null || !await access.HasRoleAsync(ctx, schedule.ProjectId, ProjectRole.Operator, ct))
            {
                return Results.NotFound();
            }

            var caller = ctx.GetAuthUser();
            if (caller is null)
            {
                return Results.Unauthorized();
            }

            var runId = await dispatcher.LaunchAsync(
                schedule.TestConfigId, null, null, caller, ct);
            schedule.LastFiredAt = DateTime.UtcNow;
            schedule.LastRunId = runId;
            await db.SaveChangesAsync(ct);

            var run = await db.TestRuns
                .AsNoTracking()
                .FirstOrDefaultAsync(r => r.Id == runId, ct);

            return run is null
                ? Results.NotFound()
                : Results.Ok(TestRunResponse.ToDto(run));
        }).RequireAuthorization();

        return app;
    }

    // Shape a TestSchedule entity into the snake_case wire DTO matching the Rust
    // networker_common::TestSchedule.
    private static object ToDto(Data.Entities.TestSchedule s) => new
    {
        id = s.Id,
        test_config_id = s.TestConfigId,
        project_id = s.ProjectId,
        cron_expr = s.CronExpr,
        timezone = s.Timezone,
        enabled = s.Enabled,
        last_fired_at = s.LastFiredAt,
        last_run_id = s.LastRunId,
        next_fire_at = s.NextFireAt,
        created_by = s.CreatedBy,
        created_at = s.CreatedAt,
    };

    // ── request bodies (snake_case JSON, matching Rust Create/UpdateScheduleRequest) ──

    // test_config_id / cron_expr are required; timezone defaults to "UTC", enabled to true.
    public sealed record CreateScheduleRequest(
        string? test_config_id,
        string? cron_expr,
        string? timezone,
        bool? enabled);

    // All fields optional — only supplied (non-null) fields are applied.
    public sealed record UpdateScheduleRequest(
        string? cron_expr,
        string? timezone,
        bool? enabled);
}
