using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Http;
using Microsoft.AspNetCore.Routing;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Networker.ControlPlane.Auth;
using Networker.Data;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Phase-2 M1: read-only parity for the PROJECTS + project DASHBOARD endpoints.
///
/// Ports three GET handlers from the Rust dashboard
/// (<c>crates/networker-dashboard/src/api/projects.rs</c> and
/// <c>.../api/dashboard.rs</c>) onto the C# control plane. Hand-written SQL +
/// manual row mapping becomes EF Core LINQ against <see cref="NetworkerDbContext"/>.
///
/// JSON field names are kept snake_case to match the exact shapes the React
/// frontend already consumes (same as the existing endpoints in Program.cs, which
/// spell anonymous-object members in snake_case directly).
///
/// Not registered in Program.cs yet (parallel-work constraint); it compiles
/// standalone and is wired up in a later step.
/// </summary>
public static class ProjectsEndpoints
{
    public static void MapProjectsEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/projects — projects visible to the caller.
        //
        // Mirrors Rust list_projects → db::projects::list_user_projects:
        // platform admins see every non-deleted project (role COALESCE'd to
        // "admin" when they aren't an explicit member); everyone else sees only
        // projects they're a member of. Response is wrapped as { "projects": [...] }.
        app.MapGet("/api/projects", async (HttpContext ctx, NetworkerDbContext db) =>
        {
            var user = ctx.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }

            List<ProjectListItem> projects;
            if (user.IsPlatformAdmin)
            {
                // LEFT JOIN project_member to surface the real role if present,
                // else "admin" (implicit platform-admin role).
                projects = await db.Projects
                    .AsNoTracking()
                    .Where(p => p.DeletedAt == null)
                    .OrderBy(p => p.CreatedAt)
                    .Select(p => new ProjectListItem(
                        p.ProjectId,
                        p.Name,
                        p.Slug,
                        p.Description,
                        db.ProjectMembers
                            .Where(m => m.ProjectId == p.ProjectId && m.UserId == user.UserId)
                            .Select(m => m.Role)
                            .FirstOrDefault() ?? "admin",
                        p.CreatedAt))
                    .ToListAsync();
            }
            else
            {
                projects = await db.Projects
                    .AsNoTracking()
                    .Where(p => p.DeletedAt == null)
                    .Join(
                        db.ProjectMembers.Where(m => m.UserId == user.UserId),
                        p => p.ProjectId,
                        m => m.ProjectId,
                        (p, m) => new ProjectListItem(
                            p.ProjectId,
                            p.Name,
                            p.Slug,
                            p.Description,
                            m.Role,
                            p.CreatedAt))
                    .OrderBy(p => p.created_at)
                    .ToListAsync();
            }

            return Results.Ok(new { projects });
        }).RequireAuthorization(AuthPolicies.GlobalViewer);

        // GET /api/projects/{projectId} — project detail + the caller's role.
        //
        // Mirrors Rust get_project_detail → db::projects::get_project. The
        // ProjectRoleHandler (via the ProjectMember policy) has already resolved
        // and stashed the caller's effective role in HttpContext.Items.
        app.MapGet("/api/projects/{projectId}", async (
            string projectId,
            HttpContext ctx,
            NetworkerDbContext db) =>
        {
            var project = await db.Projects
                .AsNoTracking()
                .Where(p => p.ProjectId == projectId)
                .Select(p => new
                {
                    p.ProjectId,
                    p.Name,
                    p.Slug,
                    p.Description,
                    p.CreatedBy,
                    p.CreatedAt,
                    p.UpdatedAt,
                    p.Settings,
                })
                .FirstOrDefaultAsync();

            if (project is null)
            {
                return Results.NotFound();
            }

            // Effective role the authorization handler resolved for this request.
            var role = ctx.Items.TryGetValue(ProjectRoleHandler.ProjectRoleItemKey, out var raw)
                       && raw is ProjectRole pr
                ? pr.ToWire()
                : "viewer";

            // settings is jsonb in Postgres, mapped as string on the entity. Emit
            // it as raw JSON (not a quoted string) to match the Rust serde_json::Value.
            var settings = string.IsNullOrWhiteSpace(project.Settings)
                ? "{}"
                : project.Settings;

            return Results.Ok(new ProjectDetail(
                project.ProjectId,
                project.Name,
                project.Slug,
                project.Description,
                project.CreatedBy,
                project.CreatedAt,
                project.UpdatedAt,
                System.Text.Json.JsonSerializer.Deserialize<System.Text.Json.JsonElement>(settings),
                role));
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        // GET /api/projects/{projectId}/dashboard/summary — aggregate stats.
        //
        // Mirrors Rust summary_scoped: agents online, in-flight runs, runs in the
        // last 24h, and queued runs — all scoped to the project. Field names match
        // the Rust DashboardSummary struct exactly.
        app.MapGet("/api/projects/{projectId}/dashboard/summary", async (
            string projectId,
            NetworkerDbContext db) =>
        {
            var agentsOnline = await db.Agents
                .CountAsync(a => a.Status == "online" && a.ProjectId == projectId);

            var jobsRunning = await db.TestRuns
                .CountAsync(r => (r.Status == "running" || r.Status == "queued")
                                 && r.ProjectId == projectId);

            var cutoff = DateTime.UtcNow.AddHours(-24);
            var runs24h = await db.TestRuns
                .CountAsync(r => r.CreatedAt > cutoff && r.ProjectId == projectId);

            var jobsPending = await db.TestRuns
                .CountAsync(r => r.Status == "queued" && r.ProjectId == projectId);

            return Results.Ok(new DashboardSummary(
                agentsOnline,
                jobsRunning,
                runs24h,
                jobsPending));
        }).RequireAuthorization(AuthPolicies.ProjectMember);
    }
}

/// <summary>
/// One row of GET /api/projects. snake_case members match the Rust
/// <c>ProjectWithRole</c> serde shape: project_id, name, slug, description, role,
/// created_at.
/// </summary>
public sealed record ProjectListItem(
    string project_id,
    string name,
    string slug,
    string? description,
    string role,
    DateTime created_at);

/// <summary>
/// GET /api/projects/{projectId} body. Matches the Rust get_project_detail JSON:
/// project_id, name, slug, description, created_by, created_at, updated_at,
/// settings, role.
/// </summary>
public sealed record ProjectDetail(
    string project_id,
    string name,
    string slug,
    string? description,
    Guid? created_by,
    DateTime created_at,
    DateTime updated_at,
    System.Text.Json.JsonElement settings,
    string role);

/// <summary>
/// GET /api/projects/{projectId}/dashboard/summary body. Matches the Rust
/// <c>DashboardSummary</c>: agents_online, jobs_running, runs_24h, jobs_pending.
/// </summary>
public sealed record DashboardSummary(
    int agents_online,
    int jobs_running,
    int runs_24h,
    int jobs_pending);
