using System.Security.Claims;

namespace Networker.ControlPlane.Auth;

/// <summary>
/// Strongly-typed view of the authenticated user, mirroring the Rust
/// <c>auth::AuthUser</c> struct injected by <c>require_auth</c>. Built from the
/// validated JWT's claims, then optionally overridden with the fresh
/// <c>dash_user</c> row by <see cref="UserStatusMiddleware"/> (the Rust side
/// trusts the DB over the token for role/is_platform_admin).
/// </summary>
public sealed record AuthUser(Guid UserId, string Email, string Role, bool IsPlatformAdmin)
{
    public Role RoleEnum => RoleExtensions.ParseRoleOrViewer(Role);

    /// <summary>
    /// Build an <see cref="AuthUser"/> from a validated principal, or null if the
    /// principal is unauthenticated / missing the required claims.
    /// </summary>
    public static AuthUser? FromPrincipal(ClaimsPrincipal? principal)
    {
        if (principal?.Identity is not { IsAuthenticated: true })
        {
            return null;
        }

        var sub = principal.FindFirstValue(JwtTokenService.SubClaim);
        if (string.IsNullOrEmpty(sub) || !Guid.TryParse(sub, out var userId))
        {
            return null;
        }

        var email = principal.FindFirstValue(JwtTokenService.EmailClaim) ?? string.Empty;
        var role = principal.FindFirstValue(JwtTokenService.RoleClaim) ?? "viewer";
        var isPlatformAdmin =
            bool.TryParse(principal.FindFirstValue(JwtTokenService.PlatformAdminClaim), out var pa) && pa;

        return new AuthUser(userId, email, role, isPlatformAdmin);
    }
}

/// <summary>
/// Per-request accessor for the current <see cref="AuthUser"/>. Registered scoped;
/// populated by <see cref="UserStatusMiddleware"/> and read by endpoints/handlers
/// (mirrors reaching into axum request extensions for <c>AuthUser</c>).
/// </summary>
public sealed class AuthUserAccessor
{
    public AuthUser? User { get; set; }
}

public static class HttpContextAuthExtensions
{
    /// <summary>
    /// Resolve the current <see cref="AuthUser"/> — prefers the middleware-populated
    /// accessor (DB-fresh role/status), falling back to raw JWT claims.
    /// </summary>
    public static AuthUser? GetAuthUser(this HttpContext ctx)
    {
        var accessor = ctx.RequestServices.GetService(typeof(AuthUserAccessor)) as AuthUserAccessor;
        return accessor?.User ?? AuthUser.FromPrincipal(ctx.User);
    }
}
