namespace Networker.ControlPlane.Auth;

/// <summary>
/// Row-level project authorization for FLAT routes (routes without a
/// <c>{projectId}</c> segment, e.g. <c>GET /api/v2/test-runs/{id}</c>), where the
/// policy-based <see cref="ProjectRoleHandler"/> cannot resolve a project scope
/// from the route. Registered SCOPED by <see cref="AuthExtensions.AddNetworkerAuth"/>;
/// it is also the shared engine <see cref="ProjectRoleHandler"/> delegates to, so
/// path-scoped and flat routes enforce the exact same rules.
///
/// <para><b>Usage in a flat endpoint</b> — load the row first (you need its
/// <c>ProjectId</c>), then gate on it, returning 404 (NOT 403) when access is
/// denied so the response does not reveal that the resource exists:</para>
/// <code>
/// app.MapGet("/api/v2/things/{id:guid}", async (
///     Guid id, HttpContext ctx, ProjectAccessChecker access,
///     NetworkerDbContext db, CancellationToken ct) =>
/// {
///     var row = await db.Things.AsNoTracking().FirstOrDefaultAsync(t => t.Id == id, ct);
///     if (row is null) return Results.NotFound();
///     if (!await access.HasRoleAsync(ctx, row.ProjectId, ProjectRole.Viewer, ct))
///         return Results.NotFound(); // no-access == not-found (no existence oracle)
///     return Results.Ok(ToDto(row));
/// }).RequireAuthorization();
/// </code>
///
/// <para>Semantics (identical to the Rust <c>require_project</c> +
/// <c>require_project_role</c> pair and the <see cref="ProjectRoleHandler"/>):
/// unauthenticated → no role; missing project → no role; soft-deleted project →
/// no role unless platform admin; platform admin → implicit
/// <see cref="ProjectRole.Admin"/>; otherwise the caller's
/// <c>project_member.role</c>.</para>
/// </summary>
public sealed class ProjectAccessChecker(AuthRepository repo)
{
    /// <summary>
    /// Resolve the caller's effective role in <paramref name="projectId"/>, or
    /// null when they have no access (unauthenticated, project missing,
    /// soft-deleted project for non-admins, or not a member).
    /// </summary>
    public async Task<ProjectRole?> GetRoleForProjectAsync(
        HttpContext ctx, string projectId, CancellationToken ct)
    {
        var user = ctx.GetAuthUser();
        if (user is null || string.IsNullOrEmpty(projectId))
        {
            return null;
        }

        var (exists, deleted) = await repo.GetProjectStateAsync(projectId, ct);

        // Only hit project_member when the pure rules can't already decide.
        ProjectRole? memberRole = null;
        if (exists && !deleted && !user.IsPlatformAdmin)
        {
            memberRole = await repo.GetMemberRoleAsync(projectId, user.UserId, ct);
        }

        return ResolveEffectiveRole(exists, deleted, user.IsPlatformAdmin, memberRole);
    }

    /// <summary>
    /// Convenience: true when the caller holds at least <paramref name="min"/>
    /// in <paramref name="projectId"/>. Flat routes should map false → 404.
    /// </summary>
    public async Task<bool> HasRoleAsync(
        HttpContext ctx, string projectId, ProjectRole min, CancellationToken ct)
    {
        var role = await GetRoleForProjectAsync(ctx, projectId, ct);
        return role is { } effective && effective.HasPermission(min);
    }

    /// <summary>
    /// Pure decision core (unit-testable, shared with <see cref="ProjectRoleHandler"/>):
    /// project must exist; soft-deleted projects are visible only to platform
    /// admins; platform admins get implicit Admin; everyone else gets their
    /// membership role (null = not a member = no access).
    /// </summary>
    public static ProjectRole? ResolveEffectiveRole(
        bool projectExists, bool projectDeleted, bool isPlatformAdmin, ProjectRole? memberRole)
    {
        if (!projectExists)
        {
            return null;
        }

        if (projectDeleted && !isPlatformAdmin)
        {
            return null;
        }

        return isPlatformAdmin ? ProjectRole.Admin : memberRole;
    }
}
