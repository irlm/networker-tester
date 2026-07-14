using Microsoft.AspNetCore.Authorization;
using Microsoft.AspNetCore.Http;

namespace Networker.ControlPlane.Auth;

/// <summary>
/// Authorization requirement asserting the caller has at least
/// <see cref="MinRole"/> within the project named by the <c>{projectId}</c> route
/// value. Mirrors the Rust <c>require_project</c> + <c>require_project_role</c>
/// pair (platform admins get implicit project Admin and bypass membership).
/// </summary>
public sealed class ProjectRoleRequirement(ProjectRole minRole) : IAuthorizationRequirement
{
    public ProjectRole MinRole { get; } = minRole;
}

/// <summary>
/// Resolves the <c>{projectId}</c> route value and checks membership/role in
/// <c>project_member</c> via raw SQL (<see cref="AuthRepository"/>). Order of
/// checks matches the Rust middleware:
/// 1. must be authenticated; 2. route must carry a project id; 3. project must
/// exist; 4. soft-deleted projects are forbidden unless platform admin;
/// 5. platform admins get implicit Admin; 6. otherwise the project_member role
/// must satisfy the requirement.
/// </summary>
public sealed class ProjectRoleHandler(
    IHttpContextAccessor httpContextAccessor,
    AuthRepository repo) : AuthorizationHandler<ProjectRoleRequirement>
{
    public const string ProjectRoleItemKey = "ProjectRole";

    protected override async Task HandleRequirementAsync(
        AuthorizationHandlerContext context,
        ProjectRoleRequirement requirement)
    {
        var httpContext = httpContextAccessor.HttpContext;
        if (httpContext is null)
        {
            return; // no context → stay unauthorized
        }

        var user = httpContext.GetAuthUser();
        if (user is null)
        {
            return;
        }

        var projectId = httpContext.Request.RouteValues.TryGetValue("projectId", out var raw)
            ? raw?.ToString()
            : null;
        if (string.IsNullOrEmpty(projectId))
        {
            return;
        }

        var (exists, deleted) = await repo.GetProjectStateAsync(projectId, httpContext.RequestAborted);
        if (!exists)
        {
            return; // not found → not authorized (endpoint may still 404 separately)
        }

        // Soft-deleted workspaces are forbidden to everyone except platform admins.
        if (deleted && !user.IsPlatformAdmin)
        {
            return;
        }

        ProjectRole effectiveRole;
        if (user.IsPlatformAdmin)
        {
            effectiveRole = ProjectRole.Admin; // implicit admin, bypass membership
        }
        else
        {
            var memberRole = await repo.GetMemberRoleAsync(projectId, user.UserId, httpContext.RequestAborted);
            if (memberRole is null)
            {
                return; // not a member
            }

            effectiveRole = memberRole.Value;
        }

        if (effectiveRole.HasPermission(requirement.MinRole))
        {
            httpContext.Items[ProjectRoleItemKey] = effectiveRole;
            context.Succeed(requirement);
        }
    }
}

/// <summary>Named policies available across the control plane.</summary>
public static class AuthPolicies
{
    // Global role policies (JWT/DB role hierarchy).
    public const string GlobalAdmin = "GlobalAdmin";
    public const string GlobalOperator = "GlobalOperator";
    public const string GlobalViewer = "GlobalViewer";

    // Project-scoped policies (resolve {projectId}, check project_member).
    public const string ProjectMember = "ProjectMember";   // any role
    public const string ProjectOperator = "ProjectOperator";
    public const string ProjectAdmin = "ProjectAdmin";
}
