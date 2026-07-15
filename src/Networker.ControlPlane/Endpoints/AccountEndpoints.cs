using System.Text.Json.Serialization;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Sso;
using Networker.Data;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Account/password endpoints — port of <c>change_password</c>,
/// <c>forgot_password</c>, and <c>reset_password</c> from
/// <c>crates/networker-dashboard/src/api/auth.rs</c> +
/// <c>db/users.rs</c>, completing the /auth surface next to the M0
/// login/profile endpoints in <see cref="AuthExtensions"/>.
///
/// <list type="bullet">
///   <item><c>POST /auth/change-password</c> — authenticated; bcrypt-verify the
///         current password, enforce the policy, store the new hash, clear
///         <c>must_change_password</c>. SSO-only accounts (no hash) get 400.</item>
///   <item><c>POST /auth/forgot-password</c> — public; ALWAYS 200
///         <c>{"sent":true}</c> so the endpoint never reveals whether an email
///         exists. When the user exists, a 64-char reset token is generated and
///         stored SHA-256-hashed with a 1-hour expiry.</item>
///   <item><c>POST /auth/reset-password</c> — public; validates the token +
///         expiry, sets the new hash, clears the token columns.</item>
/// </list>
///
/// Error responses are plain-text 400 bodies with the exact Rust messages (the
/// React frontend surfaces the text verbatim).
/// </summary>
public static class AccountEndpoints
{
    public static IEndpointRouteBuilder MapAccountEndpoints(this IEndpointRouteBuilder app)
    {
        // POST /auth/change-password — authenticated (any role; UserStatusMiddleware
        // allows pending + must-change users onto this exact path, like Rust).
        app.MapPost("/api/auth/change-password", async (
            ChangePasswordRequest req,
            HttpContext http,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var user = http.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }

            // Rust db::users::change_password: only active/pending accounts.
            var row = await db.DashUsers.FirstOrDefaultAsync(
                u => u.UserId == user.UserId && (u.Status == "active" || u.Status == "pending"), ct);
            if (row is null)
            {
                return BadRequestText("User not found");
            }

            // SSO-only account (no local password) — 400, same message as Rust.
            if (row.PasswordHash is null)
            {
                return BadRequestText("SSO accounts cannot change password here");
            }

            bool valid;
            try
            {
                valid = BCrypt.Net.BCrypt.Verify(req.CurrentPassword, row.PasswordHash);
            }
            catch (Exception)
            {
                valid = false; // malformed stored hash → treat as wrong password
            }

            if (!valid)
            {
                return BadRequestText("Current password is incorrect");
            }

            if (AccountSecurity.ValidateChangedPassword(req.CurrentPassword, req.NewPassword) is { } error)
            {
                return BadRequestText(error);
            }

            row.PasswordHash = BCrypt.Net.BCrypt.HashPassword(req.NewPassword);
            row.MustChangePassword = false;
            await db.SaveChangesAsync(ct);

            return Results.Ok(new { success = true });
        }).RequireAuthorization();

        // POST /auth/forgot-password — public; ALWAYS 200 { sent: true }.
        app.MapPost("/api/auth/forgot-password", async (
            ForgotPasswordRequest req,
            NetworkerDbContext db,
            ILoggerFactory loggerFactory,
            CancellationToken ct) =>
        {
            var log = loggerFactory.CreateLogger("Networker.ControlPlane.Account");

            try
            {
                // Rust create_reset_token: exact email match, active users only.
                var row = await db.DashUsers.FirstOrDefaultAsync(
                    u => u.Email == req.Email && u.Status == "active", ct);

                if (row is not null)
                {
                    var token = AccountSecurity.GenerateAlphanumericToken(AccountSecurity.ResetTokenLength);
                    row.PasswordResetToken = AccountSecurity.HashToken(token);
                    row.PasswordResetExpires = DateTime.UtcNow + AccountSecurity.ResetTokenLifetime;
                    await db.SaveChangesAsync(ct);

                    // TODO(email): the Rust dashboard emails
                    // {public_url}/reset-password?token={token} via crate::email.
                    // Email delivery is not wired in the C# control plane yet —
                    // the token is stored (hashed) and the link is NOT logged
                    // (logging the raw token would defeat storing only the hash).
                    log.LogInformation(
                        "Password reset token generated for {Email}; email delivery not implemented (TODO)",
                        req.Email);
                }
                else
                {
                    log.LogInformation("Password reset requested for unknown email {Email}", req.Email);
                }
            }
            catch (Exception ex)
            {
                // Never leak DB errors either — Rust returns { sent: true } on
                // every path.
                log.LogError(ex, "Failed to create password reset token");
            }

            return Results.Ok(new { sent = true });
        }).AllowAnonymous();

        // POST /auth/reset-password — public; token + new password.
        app.MapPost("/api/auth/reset-password", async (
            ResetPasswordRequest req,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            if (AccountSecurity.ValidateResetPassword(req.NewPassword) is { } error)
            {
                return BadRequestText(error);
            }

            var tokenHash = AccountSecurity.HashToken(req.Token);
            var row = await db.DashUsers.FirstOrDefaultAsync(
                u => u.PasswordResetToken == tokenHash && u.Status == "active", ct);
            if (row is null)
            {
                return BadRequestText("Invalid or expired reset link");
            }

            if (row.PasswordResetExpires is null)
            {
                return BadRequestText("Invalid reset link");
            }

            if (AccountSecurity.IsResetTokenExpired(row.PasswordResetExpires, DateTime.UtcNow))
            {
                return BadRequestText("Reset link has expired. Request a new one.");
            }

            row.PasswordHash = BCrypt.Net.BCrypt.HashPassword(req.NewPassword);
            row.MustChangePassword = false;
            row.PasswordResetToken = null;
            row.PasswordResetExpires = null;
            await db.SaveChangesAsync(ct);

            return Results.Ok(new { success = true });
        }).AllowAnonymous();

        return app;
    }

    /// <summary>Plain-text 400, matching the Rust (StatusCode, &amp;str) responses.</summary>
    private static IResult BadRequestText(string message)
        => Results.Text(message, statusCode: StatusCodes.Status400BadRequest);

    /// <summary>Matches Rust <c>ChangePasswordRequest</c>.</summary>
    public sealed record ChangePasswordRequest(
        [property: JsonPropertyName("current_password")] string CurrentPassword,
        [property: JsonPropertyName("new_password")] string NewPassword);

    /// <summary>Matches Rust <c>ForgotPasswordRequest</c>.</summary>
    public sealed record ForgotPasswordRequest(
        [property: JsonPropertyName("email")] string Email);

    /// <summary>Matches Rust <c>ResetPasswordRequest</c>.</summary>
    public sealed record ResetPasswordRequest(
        [property: JsonPropertyName("token")] string Token,
        [property: JsonPropertyName("new_password")] string NewPassword);
}
