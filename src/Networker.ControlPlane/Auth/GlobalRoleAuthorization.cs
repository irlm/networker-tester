using Microsoft.AspNetCore.Authorization;
using Microsoft.AspNetCore.Http;

namespace Networker.ControlPlane.Auth;

/// <summary>
/// Requirement asserting the caller's global role is at least <see cref="MinRole"/>
/// in the Admin &gt; Operator &gt; Viewer hierarchy. Mirrors Rust <c>require_role</c>.
/// </summary>
public sealed class GlobalRoleRequirement(Role minRole) : IAuthorizationRequirement
{
    public Role MinRole { get; } = minRole;
}

/// <summary>
/// Evaluates <see cref="GlobalRoleRequirement"/> against the DB-fresh role held by
/// the <see cref="AuthUserAccessor"/> (populated by <see cref="UserStatusMiddleware"/>),
/// falling back to the JWT claim. Unknown roles fail closed to Viewer, matching Rust.
/// </summary>
public sealed class GlobalRoleHandler(IHttpContextAccessor httpContextAccessor)
    : AuthorizationHandler<GlobalRoleRequirement>
{
    protected override Task HandleRequirementAsync(
        AuthorizationHandlerContext context,
        GlobalRoleRequirement requirement)
    {
        var user = httpContextAccessor.HttpContext?.GetAuthUser();
        if (user is not null && user.RoleEnum.HasPermission(requirement.MinRole))
        {
            context.Succeed(requirement);
        }

        return Task.CompletedTask;
    }
}
