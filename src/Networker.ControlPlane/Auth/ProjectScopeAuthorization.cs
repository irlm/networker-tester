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
/// Resolves the <c>{projectId}</c> route value and checks membership/role via the
/// shared <see cref="ProjectAccessChecker"/> (must-be-authenticated → project must
/// exist → soft-deleted projects forbidden unless platform admin → platform admins
/// get implicit Admin → otherwise the project_member role must satisfy the
/// requirement). Flat routes without <c>{projectId}</c> can't use this policy —
/// they call <see cref="ProjectAccessChecker"/> directly against the loaded row's
/// ProjectId, so both paths enforce identical rules.
/// </summary>
public sealed class ProjectRoleHandler(
    IHttpContextAccessor httpContextAccessor,
    ProjectAccessChecker accessChecker) : AuthorizationHandler<ProjectRoleRequirement>
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

        var projectId = httpContext.Request.RouteValues.TryGetValue("projectId", out var raw)
            ? raw?.ToString()
            : null;
        if (string.IsNullOrEmpty(projectId))
        {
            return;
        }

        var effectiveRole = await accessChecker.GetRoleForProjectAsync(
            httpContext, projectId, httpContext.RequestAborted);

        if (effectiveRole is { } role && role.HasPermission(requirement.MinRole))
        {
            httpContext.Items[ProjectRoleItemKey] = role;
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
