using Microsoft.Extensions.Caching.Memory;

namespace Networker.ControlPlane.Auth;

/// <summary>
/// Per-request middleware that re-reads the caller's <c>dash_user</c> row and
/// enforces account status, mirroring what the Rust <c>require_auth</c> middleware
/// does on every authenticated request (trust the DB over the token):
///
/// <list type="bullet">
///   <item><c>pending</c>: only /auth/profile and /auth/change-password allowed → else 403 "pending_approval".</item>
///   <item><c>disabled</c>/<c>denied</c> (any non-active, non-pending): 403 "Account is not active"
///         (unless it's the change-password path for a must_change user).</item>
///   <item><c>must_change_password</c> (while active): gate everything except
///         /auth/change-password and /auth/profile → 403.</item>
/// </list>
///
/// The DB role + is_platform_admin from the row override the JWT claims (fresh DB
/// wins). Only runs when a valid principal is present; anonymous requests pass
/// through untouched so the existing unauthenticated routes keep working.
///
/// A short-TTL <see cref="IMemoryCache"/> entry (see <see cref="CacheTtl"/>)
/// avoids a DB round-trip on every request for the same user.
/// </summary>
public sealed class UserStatusMiddleware(RequestDelegate next, IMemoryCache cache)
{
    public static readonly TimeSpan CacheTtl = TimeSpan.FromSeconds(10);

    public async Task InvokeAsync(HttpContext ctx, AuthRepository repo, AuthUserAccessor accessor)
    {
        var jwtUser = AuthUser.FromPrincipal(ctx.User);
        if (jwtUser is null)
        {
            // Anonymous request — leave it alone (existing routes stay open).
            await next(ctx);
            return;
        }

        var cacheKey = $"authstatus:{jwtUser.UserId}";
        if (!cache.TryGetValue(cacheKey, out AuthRepository.UserStatusRow? row))
        {
            row = await repo.GetUserStatusAsync(jwtUser.UserId, ctx.RequestAborted);
            cache.Set(cacheKey, row, CacheTtl);
        }

        var path = ctx.Request.Path.Value ?? string.Empty;
        var isChangePassword = path.EndsWith("/auth/change-password", StringComparison.Ordinal);
        var isProfile = path.EndsWith("/auth/profile", StringComparison.Ordinal);

        AuthUser effectiveUser = jwtUser;

        if (row is not null)
        {
            var mustChange = row.MustChangePassword;
            var status = row.Status;
            var isPendingAllowed = isChangePassword || isProfile;

            // Pending users: only /auth/profile + /auth/change-password.
            if (status == "pending" && !isPendingAllowed)
            {
                await Forbid(ctx, "pending_approval");
                return;
            }

            // Block other non-active users (disabled, denied), unless it's a
            // must-change user hitting change-password.
            if (status != "active" && status != "pending" && !(isChangePassword && mustChange))
            {
                await Forbid(ctx, "Account is not active");
                return;
            }

            // Enforce must_change_password on active users.
            if (!isChangePassword && !isProfile && mustChange && status == "active")
            {
                await Forbid(ctx, "Password change required before accessing this resource");
                return;
            }

            // Fresh DB role/is_platform_admin win over the token claims.
            effectiveUser = jwtUser with { Role = row.Role, IsPlatformAdmin = row.IsPlatformAdmin };
        }

        accessor.User = effectiveUser;
        await next(ctx);
    }

    private static async Task Forbid(HttpContext ctx, string message)
    {
        ctx.Response.StatusCode = StatusCodes.Status403Forbidden;
        ctx.Response.ContentType = "text/plain; charset=utf-8";
        await ctx.Response.WriteAsync(message);
    }
}
