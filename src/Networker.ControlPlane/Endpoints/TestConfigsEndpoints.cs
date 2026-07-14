using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// REST v2 read endpoints for test configs — the C# port of the Rust
/// <c>crates/networker-dashboard/src/api/test_configs.rs</c> list / get
/// handlers. JSON field names are snake_case to match the Rust
/// <c>networker_common::TestConfig</c> wire shape.
///
/// The polymorphic <c>endpoint</c> / <c>workload</c> / <c>methodology</c> fields
/// are stored as JSONB (text in the EF entity). Rust deserializes the
/// <c>endpoint_ref</c> column into the <c>endpoint</c> field; we re-emit the raw
/// JSON under the same field names so the shapes line up. Note the derived
/// <c>endpoint_kind</c> column is intentionally NOT serialized — the Rust
/// TestConfig doesn't expose it (it's recoverable from <c>endpoint.kind</c>).
///
/// Phase-2 M1 scope: read-only. Create / update / delete / launch are not ported.
/// </summary>
public static class TestConfigsEndpoints
{
    private const int ListLimit = 200;

    public static IEndpointRouteBuilder MapTestConfigsEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/v2/projects/{projectId}/test-configs — list (newest first,
        // capped at 200). Mirrors Rust list_handler + db::test_configs::list.
        app.MapGet("/api/v2/projects/{projectId}/test-configs", async (
            string projectId,
            NetworkerDbContext db) =>
        {
            var rows = await db.TestConfigs
                .AsNoTracking()
                .Where(c => c.ProjectId == projectId)
                .OrderByDescending(c => c.CreatedAt)
                .Take(ListLimit)
                .ToListAsync();

            return Results.Ok(rows.Select(ToDto));
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        // GET /api/v2/test-configs/{id} — single config detail (incl.
        // baseline_run_id). Mirrors Rust get_handler + db::test_configs::get.
        // Flat route (no {projectId}), so the ProjectMember policy can't resolve a
        // project scope. Instead: load the row, then row-level authz via
        // ProjectAccessChecker against cfg.ProjectId. No access → 404 (identical
        // to not-found, so the route is not an existence oracle).
        app.MapGet("/api/v2/test-configs/{id:guid}", async (
            Guid id,
            HttpContext ctx,
            ProjectAccessChecker access,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var cfg = await db.TestConfigs
                .AsNoTracking()
                .FirstOrDefaultAsync(c => c.Id == id, ct);

            if (cfg is null ||
                !await access.HasRoleAsync(ctx, cfg.ProjectId, ProjectRole.Viewer, ct))
            {
                return Results.NotFound();
            }

            return Results.Ok(ToDto(cfg));
        }).RequireAuthorization();

        return app;
    }

    // Shape a TestConfig entity into the snake_case wire DTO matching the Rust
    // networker_common::TestConfig. The JSONB columns are re-emitted as raw JSON.
    private static object ToDto(Data.Entities.TestConfig c) => new
    {
        id = c.Id,
        project_id = c.ProjectId,
        name = c.Name,
        description = c.Description,
        endpoint = RawJson(c.EndpointRef),
        workload = RawJson(c.Workload),
        methodology = RawJsonOrNull(c.Methodology),
        baseline_run_id = c.BaselineRunId,
        max_duration_secs = c.MaxDurationSecs,
        created_by = c.CreatedBy,
        created_at = c.CreatedAt,
        updated_at = c.UpdatedAt,
    };

    // Parse a JSONB-as-text column into a JsonElement so it serializes as raw
    // JSON. Falls back to the original text if it isn't valid JSON (defensive).
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
