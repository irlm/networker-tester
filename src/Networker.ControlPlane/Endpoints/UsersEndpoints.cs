using System.Security.Cryptography;
using System.Text;
using System.Text.Json.Serialization;
using Microsoft.AspNetCore.Mvc;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Phase-2 M5: platform-global USER administration — the C# port of the Rust
/// <c>crates/networker-dashboard/src/api/users.rs</c> router
/// (list / pending / invite / approve / deny / set-role / disable).
///
/// <para>Every route is platform-global (no <c>{projectId}</c> scope) and gated
/// by <see cref="AuthPolicies.GlobalAdmin"/>, matching the Rust
/// <c>require_role(Role::Admin)</c> guard on each handler. Response shapes are
/// snake_case and field-for-field compatible with the Rust
/// <c>db::users::UserRow</c> serde output (plus <c>is_platform_admin</c>, which
/// the frontend admin views consume) — <c>password_hash</c> and the reset-token
/// columns are never serialized.</para>
///
/// <para>Invite emails: the retired Rust dashboard sent a setup link via its
/// <c>email::send_email</c> helper. There is no mailer in the C# control plane
/// yet, so the setup token is generated + stored (SHA-256 hash, 24h expiry,
/// same as Rust) and the send is a loud TODO log.</para>
/// </summary>
public static class UsersEndpoints
{
    /// <summary>Role whitelist shared by invite/approve/set-role — mirrors the
    /// Rust <c>VALID_ROLES</c> const.</summary>
    public static readonly string[] ValidRoles = ["admin", "operator", "viewer"];

    /// <summary>Pure validation core (unit-testable): exact lowercase match only.</summary>
    public static bool IsValidRole(string? role) => role is not null && ValidRoles.Contains(role);

    public static IEndpointRouteBuilder MapUsersEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/users — all users ordered by creation date (Rust list_users).
        // Returns a bare JSON array, matching Json(Vec<UserRow>).
        app.MapGet("/api/users", async (NetworkerDbContext db, CancellationToken ct) =>
        {
            var users = await QueryUsers(db).ToListAsync(ct);
            return Results.Ok(users);
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        // GET /api/users/pending — users awaiting approval (Rust list_pending).
        // Wrapped as { users: [...], count: N }.
        app.MapGet("/api/users/pending", async (NetworkerDbContext db, CancellationToken ct) =>
        {
            var users = await QueryUsers(db, status: "pending").ToListAsync(ct);
            return Results.Ok(new { users, count = users.Count });
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        // POST /api/users/invite — create a pending account with a hashed setup
        // token (Rust invite_user). 409 on duplicate email, 400 on bad role.
        app.MapPost("/api/users/invite", async (
            [FromBody] InviteRequest req,
            HttpContext ctx,
            NetworkerDbContext db,
            ILoggerFactory loggerFactory,
            CancellationToken ct) =>
        {
            var admin = ctx.GetAuthUser();
            if (admin is null)
            {
                return Results.Unauthorized();
            }

            if (!IsValidRole(req.Role))
            {
                return ApiError.BadRequest("Invalid role (must be admin, operator, or viewer)");
            }
            if (string.IsNullOrWhiteSpace(req.Email))
            {
                return ApiError.BadRequest("Email is required");
            }

            var email = req.Email.Trim();
            var emailLower = email.ToLowerInvariant();
            var exists = await db.DashUsers
                .AnyAsync(u => u.Email != null && u.Email.ToLower() == emailLower, ct);
            if (exists)
            {
                return ApiError.Conflict("Email already registered");
            }

            // Setup token: 64 alphanumeric chars, stored SHA-256-hashed with a
            // 24h expiry — identical scheme to the Rust invite_user.
            var token = GenerateToken(64);
            var user = new Data.Entities.DashUser
            {
                UserId = Guid.NewGuid(),
                Email = email,
                Role = req.Role,
                Status = "pending",
                AuthProvider = "local",
                MustChangePassword = true,
                PasswordResetToken = Sha256Hex(token),
                PasswordResetExpires = DateTime.UtcNow.AddHours(24),
                CreatedAt = DateTime.UtcNow,
            };
            db.DashUsers.Add(user);
            await db.SaveChangesAsync(ct);

            // TODO(M6 mailer): send the invite email with the setup link
            // ({public_url}/reset-password?token=<raw token>) exactly like the
            // Rust side. No mailer exists in the C# control plane yet.
            loggerFactory.CreateLogger("UsersEndpoints").LogWarning(
                "TODO email stub: invite email NOT sent to {Email} (role {Role}, invited by {Admin}) — no mailer wired yet",
                email, req.Role, admin.Email);

            return Results.Ok(new { user_id = user.UserId.ToString() });
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        // POST /api/users/{userId}/approve — pending → active with the given
        // role (Rust approve_user). 400 on bad role, 404 when not pending.
        app.MapPost("/api/users/{userId:guid}/approve", async (
            Guid userId,
            [FromBody] RoleBody req,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            if (!IsValidRole(req.Role))
            {
                return ApiError.BadRequest("Invalid role");
            }

            var n = await db.DashUsers
                .Where(u => u.UserId == userId && u.Status == "pending")
                .ExecuteUpdateAsync(s => s
                    .SetProperty(u => u.Status, "active")
                    .SetProperty(u => u.Role, req.Role), ct);

            return n > 0 ? Results.Ok(new { approved = true }) : Results.NotFound();
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        // POST /api/users/{userId}/deny — pending → denied (Rust deny_user).
        app.MapPost("/api/users/{userId:guid}/deny", async (
            Guid userId,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var n = await db.DashUsers
                .Where(u => u.UserId == userId && u.Status == "pending")
                .ExecuteUpdateAsync(s => s.SetProperty(u => u.Status, "denied"), ct);

            return n > 0 ? Results.Ok(new { denied = true }) : Results.NotFound();
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        // PUT /api/users/{userId}/role — change an ACTIVE user's role (Rust
        // set_role). 400 on bad role or self-demotion, 404 when not active.
        app.MapPut("/api/users/{userId:guid}/role", async (
            Guid userId,
            [FromBody] RoleBody req,
            HttpContext ctx,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            if (!IsValidRole(req.Role))
            {
                return ApiError.BadRequest("Invalid role");
            }

            // Prevent an admin from demoting themselves (Rust parity).
            var admin = ctx.GetAuthUser();
            if (admin is not null && admin.UserId == userId)
            {
                return ApiError.BadRequest("Cannot change your own role");
            }

            var n = await db.DashUsers
                .Where(u => u.UserId == userId && u.Status == "active")
                .ExecuteUpdateAsync(s => s.SetProperty(u => u.Role, req.Role), ct);

            return n > 0 ? Results.Ok(new { updated = true }) : Results.NotFound();
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        // POST /api/users/{userId}/disable — active → disabled (Rust
        // disable_user). 400 on self-disable, 404 when not active.
        app.MapPost("/api/users/{userId:guid}/disable", async (
            Guid userId,
            HttpContext ctx,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var admin = ctx.GetAuthUser();
            if (admin is not null && admin.UserId == userId)
            {
                return ApiError.BadRequest("Cannot disable your own account");
            }

            var n = await db.DashUsers
                .Where(u => u.UserId == userId && u.Status == "active")
                .ExecuteUpdateAsync(s => s.SetProperty(u => u.Status, "disabled"), ct);

            return n > 0 ? Results.Ok(new { disabled = true }) : Results.NotFound();
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        return app;
    }

    /// <summary>
    /// Projection shared by list + pending — the Rust UserRow SELECT (never
    /// password_hash / reset token columns), plus is_platform_admin.
    /// </summary>
    private static IQueryable<UserRow> QueryUsers(NetworkerDbContext db, string? status = null)
    {
        var q = db.DashUsers.AsNoTracking();
        if (status is not null)
        {
            q = q.Where(u => u.Status == status);
        }

        return q
            .OrderBy(u => u.CreatedAt)
            .Select(u => new UserRow(
                u.UserId,
                u.Email ?? string.Empty,
                u.Role,
                u.Status,
                u.AuthProvider,
                u.DisplayName,
                u.IsPlatformAdmin,
                u.LastLoginAt,
                u.CreatedAt));
    }

    /// <summary>64-char URL-safe alphanumeric token (Rust: Alphanumeric sample).</summary>
    private static string GenerateToken(int length)
    {
        const string alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        return RandomNumberGenerator.GetString(alphabet, length);
    }

    /// <summary>SHA-256 hex — same at-rest token hashing as Rust <c>hash_token</c>.</summary>
    private static string Sha256Hex(string token)
        => Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(token)));

    /// <summary>POST /api/users/invite body — mirrors Rust <c>InviteRequest</c>.</summary>
    public sealed record InviteRequest(
        [property: JsonPropertyName("email")] string Email,
        [property: JsonPropertyName("role")] string Role);

    /// <summary>approve / set-role body — mirrors Rust <c>ApproveRequest</c> /
    /// <c>SetRoleRequest</c> (both are just <c>{ role }</c>).</summary>
    public sealed record RoleBody([property: JsonPropertyName("role")] string Role);

    /// <summary>
    /// One user row — the Rust <c>db::users::UserRow</c> serde shape
    /// (user_id, email, role, status, auth_provider, display_name,
    /// last_login_at, created_at) plus <c>is_platform_admin</c>.
    /// </summary>
    public sealed record UserRow(
        Guid user_id,
        string email,
        string role,
        string status,
        string auth_provider,
        string? display_name,
        bool is_platform_admin,
        DateTime? last_login_at,
        DateTime created_at);
}
