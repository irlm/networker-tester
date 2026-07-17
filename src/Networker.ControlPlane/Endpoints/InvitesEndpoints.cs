using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using Microsoft.AspNetCore.Mvc;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Phase-2 M5: WORKSPACE INVITES — the C# port of the Rust
/// <c>crates/networker-dashboard/src/api/invites.rs</c>.
///
/// Project-scoped (admin): create / list / revoke. Public (no auth):
/// GET /api/invite/{token} resolve and POST /api/invite/{token}/accept.
///
/// Tokens are 32 random bytes (RandomNumberGenerator) encoded as URL-safe
/// base64 without padding (43 chars) — only the SHA-256 hex hash is stored,
/// identical to the Rust side, so tokens minted by either backend resolve on
/// the other. Single-use: resolve requires <c>status = 'pending'</c> and
/// <c>expires_at &gt; now</c>; accept flips the row to 'accepted', so a second
/// use 404s ("Invite expired or invalid" — expired, revoked, and consumed
/// tokens are indistinguishable to the caller, same as Rust).
///
/// Email delivery is a TODO stub (logged); the invite URL is still returned to
/// the admin in the create response, exactly like the Rust response shape.
/// </summary>
public static class InvitesEndpoints
{
    private static readonly string[] ValidRoles = ["admin", "operator", "viewer"];

    public static IEndpointRouteBuilder MapInvitesEndpoints(this IEndpointRouteBuilder app)
    {
        // POST /api/projects/{projectId}/invites — create (project admin).
        // Mirrors Rust create_invite: validate role + email, mint token, store
        // the hash, expiry = now + DASHBOARD_INVITE_EXPIRY_DAYS (default 7).
        // Returns 200 { invite_id, url, expires_at }.
        app.MapPost("/api/projects/{projectId}/invites", async (
            string projectId,
            [FromBody] CreateInviteRequest req,
            HttpContext ctx,
            NetworkerDbContext db,
            ILoggerFactory loggerFactory,
            CancellationToken ct) =>
        {
            var user = ctx.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }

            if (!ValidRoles.Contains(req.Role))
            {
                return ApiError.BadRequest("role must be 'admin', 'operator', or 'viewer'");
            }

            var email = req.Email ?? string.Empty;
            if (!email.Contains('@') || email.Length < 3)
            {
                return ApiError.BadRequest("Invalid email address");
            }

            var rawToken = CollabTokens.Generate();
            var expiresAt = DateTime.UtcNow.AddDays(CollabConfig.InviteExpiryDays());

            var invite = new WorkspaceInvite
            {
                InviteId = Guid.NewGuid(),
                ProjectId = projectId,
                Email = email,
                Role = req.Role,
                TokenHash = CollabTokens.Sha256Hex(rawToken),
                Status = "pending",
                InvitedBy = user.UserId,
                ExpiresAt = expiresAt,
            };
            db.WorkspaceInvites.Add(invite);
            await db.SaveChangesAsync(ct);

            var inviteUrl = $"{CollabConfig.PublicUrl()}/invite/{rawToken}";

            // TODO(phase2): wire IEmailSender for the best-effort invite email
            // (the retired Rust dashboard sent it via its email::send_email
            // helper). The Rust side already tolerated send failures, so the
            // response contract is unchanged by this stub.
            loggerFactory
                .CreateLogger("Networker.ControlPlane.InvitesEndpoints")
                .LogWarning(
                    "create-invite: email sending not yet implemented in the C# control plane " +
                    "(would send invite to {Email} for project {ProjectId})", email, projectId);

            return Results.Ok(new
            {
                invite_id = invite.InviteId.ToString(),
                url = inviteUrl,
                expires_at = expiresAt,
            });
        }).RequireAuthorization(AuthPolicies.ProjectAdmin);

        // GET /api/projects/{projectId}/invites — list (project admin).
        // Mirrors Rust list_invites → db::invites::list_invites: bare array,
        // inner join dash_user for invited_by_email, newest first.
        app.MapGet("/api/projects/{projectId}/invites", async (
            string projectId,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var invites = await (
                from i in db.WorkspaceInvites.AsNoTracking()
                join u in db.DashUsers on i.InvitedBy equals u.UserId
                where i.ProjectId == projectId
                orderby i.CreatedAt descending
                select new InviteRow(
                    i.InviteId,
                    i.ProjectId,
                    i.Email,
                    i.Role,
                    i.Status,
                    i.InvitedBy,
                    u.Email ?? string.Empty,
                    i.CreatedAt,
                    i.ExpiresAt,
                    i.AcceptedAt)).ToListAsync(ct);

            return Results.Ok(invites);
        }).RequireAuthorization(AuthPolicies.ProjectAdmin);

        // DELETE /api/projects/{projectId}/invites/{inviteId} — revoke (admin).
        // Mirrors Rust revoke_invite: only pending invites flip to 'revoked';
        // the response is { revoked: true } regardless of whether a row matched
        // (Rust ignores the affected-row count).
        app.MapDelete("/api/projects/{projectId}/invites/{inviteId:guid}", async (
            string projectId,
            Guid inviteId,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            await db.WorkspaceInvites
                .Where(i => i.InviteId == inviteId && i.ProjectId == projectId && i.Status == "pending")
                .ExecuteUpdateAsync(s => s.SetProperty(i => i.Status, "revoked"), ct);

            return Results.Ok(new { revoked = true });
        }).RequireAuthorization(AuthPolicies.ProjectAdmin);

        // GET /api/invite/{token} — PUBLIC resolve. Mirrors Rust resolve_invite:
        // pending + unexpired only; expired/revoked/consumed/unknown are all the
        // same 404 (no oracle for which). has_account reflects whether the
        // invited email already maps to an ACTIVE dash_user.
        app.MapGet("/api/invite/{token}", async (
            string token,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var resolved = await ResolveInviteAsync(db, token, ct);
            if (resolved is null)
            {
                return ApiError.NotFound("Invite expired or invalid");
            }

            return Results.Ok(new
            {
                invite_id = resolved.InviteId,
                project_id = resolved.ProjectId,
                project_name = resolved.ProjectName,
                email = resolved.Email,
                role = resolved.Role,
                has_account = resolved.HasAccount,
                expires_at = resolved.ExpiresAt,
            });
        }).AllowAnonymous();

        // POST /api/invite/{token}/accept — PUBLIC accept. Mirrors Rust
        // accept_invite:
        //   existing account → authenticate via Bearer JWT (email must match the
        //     invite) or current_password (bcrypt) → 401 otherwise;
        //   no account → create a local viewer account (password ≥ 8 chars);
        //   then mark the invite accepted (single-use), upsert the project
        //   membership with the invite's role, and mint a JWT.
        // Returns { token, email, role, project_id }.
        app.MapPost("/api/invite/{token}/accept", async (
            string token,
            HttpContext ctx,
            NetworkerDbContext db,
            JwtTokenService tokens,
            CancellationToken ct) =>
        {
            // Rust tolerates an empty/invalid JSON body (JWT-only accept).
            var payload = new AcceptInviteRequest(null, null, null);
            try
            {
                payload = await ctx.Request.ReadFromJsonAsync<AcceptInviteRequest>(ct) ?? payload;
            }
            catch (JsonException)
            {
                // keep the empty payload
            }

            var invite = await ResolveInviteAsync(db, token, ct);
            if (invite is null)
            {
                return ApiError.NotFound("Invite expired or invalid");
            }

            Guid acceptedUserId;
            string acceptedEmail;
            bool isPlatformAdmin;

            if (invite.HasAccount)
            {
                // Try the validated JWT first (authentication middleware has
                // already verified any Bearer token even on this anonymous route).
                var authUser = ctx.GetAuthUser();
                if (authUser is not null
                    && string.Equals(authUser.Email, invite.Email, StringComparison.OrdinalIgnoreCase))
                {
                    acceptedUserId = authUser.UserId;
                    acceptedEmail = authUser.Email;
                    isPlatformAdmin = authUser.IsPlatformAdmin;
                }
                else if (payload.CurrentPassword is { } pwd)
                {
                    var account = await db.DashUsers
                        .AsNoTracking()
                        .Where(u => u.Email != null
                                    && u.Email.ToLower() == invite.Email.ToLower()
                                    && u.Status == "active")
                        .Select(u => new { u.UserId, u.Email, u.PasswordHash, u.IsPlatformAdmin })
                        .FirstOrDefaultAsync(ct);

                    bool valid;
                    try
                    {
                        valid = account?.PasswordHash is not null
                                && BCrypt.Net.BCrypt.Verify(pwd, account.PasswordHash);
                    }
                    catch (Exception)
                    {
                        valid = false; // malformed hash → invalid credentials, never 500
                    }

                    if (!valid)
                    {
                        return ApiError.Status(StatusCodes.Status401Unauthorized, "Invalid credentials");
                    }

                    acceptedUserId = account!.UserId;
                    acceptedEmail = account.Email!;
                    isPlatformAdmin = account.IsPlatformAdmin;
                }
                else
                {
                    return ApiError.Status(StatusCodes.Status401Unauthorized, "Authentication required — provide Authorization header or current_password");
                }
            }
            else
            {
                // No account for the invited email — create one (Rust
                // db::users::create_local_user: viewer / active / local).
                if (payload.Password is null)
                {
                    return ApiError.BadRequest("password is required to create an account");
                }

                if (payload.Password.Length < 8)
                {
                    return ApiError.BadRequest("Password must be at least 8 characters");
                }

                var created = new DashUser
                {
                    UserId = Guid.NewGuid(),
                    Email = invite.Email,
                    PasswordHash = BCrypt.Net.BCrypt.HashPassword(payload.Password),
                    Role = "viewer",
                    Status = "active",
                    AuthProvider = "local",
                    MustChangePassword = false,
                    IsPlatformAdmin = false,
                };
                db.DashUsers.Add(created);
                await db.SaveChangesAsync(ct);

                acceptedUserId = created.UserId;
                acceptedEmail = invite.Email;
                isPlatformAdmin = false;
            }

            // Consume the token (single-use) — Rust db::invites::accept_invite.
            await db.WorkspaceInvites
                .Where(i => i.InviteId == invite.InviteId)
                .ExecuteUpdateAsync(s => s
                    .SetProperty(i => i.Status, "accepted")
                    .SetProperty(i => i.AcceptedAt, DateTime.UtcNow)
                    .SetProperty(i => i.AcceptedBy, acceptedUserId), ct);

            // Add (or activate) the membership with the invite's role. Rust
            // add_member only upserts the role on conflict; we also flip a
            // pending_acceptance row to active so an invite accepted by an
            // imported-pending user actually grants access.
            var membership = await db.ProjectMembers
                .FirstOrDefaultAsync(m => m.ProjectId == invite.ProjectId && m.UserId == acceptedUserId, ct);
            if (membership is null)
            {
                db.ProjectMembers.Add(new ProjectMember
                {
                    ProjectId = invite.ProjectId,
                    UserId = acceptedUserId,
                    Role = invite.Role,
                    JoinedAt = DateTime.UtcNow,
                    InvitedBy = acceptedUserId,
                    Status = "active",
                });
            }
            else
            {
                membership.Role = invite.Role;
                if (membership.Status == "pending_acceptance")
                {
                    membership.Status = "active";
                }
            }

            await db.SaveChangesAsync(ct);

            var jwt = tokens.CreateToken(acceptedUserId, acceptedEmail, invite.Role, isPlatformAdmin);

            return Results.Ok(new
            {
                token = jwt,
                email = acceptedEmail,
                role = invite.Role,
                project_id = invite.ProjectId,
            });
        }).AllowAnonymous();

        return app;
    }

    /// <summary>
    /// Rust db::invites::resolve_invite — pending + unexpired, joined with the
    /// project name, plus the has_account probe (active dash_user with the
    /// invited email).
    /// </summary>
    private static async Task<ResolvedInvite?> ResolveInviteAsync(
        NetworkerDbContext db, string token, CancellationToken ct)
    {
        var tokenHash = CollabTokens.Sha256Hex(token);
        var now = DateTime.UtcNow;

        var invite = await (
            from i in db.WorkspaceInvites.AsNoTracking()
            join p in db.Projects on i.ProjectId equals p.ProjectId
            where i.TokenHash == tokenHash && i.Status == "pending" && i.ExpiresAt > now
            select new { i.InviteId, i.ProjectId, ProjectName = p.Name, i.Email, i.Role, i.ExpiresAt })
            .FirstOrDefaultAsync(ct);

        if (invite is null)
        {
            return null;
        }

        var hasAccount = await db.DashUsers
            .AsNoTracking()
            .AnyAsync(u => u.Email != null
                           && u.Email.ToLower() == invite.Email.ToLower()
                           && u.Status == "active", ct);

        return new ResolvedInvite(
            invite.InviteId, invite.ProjectId, invite.ProjectName,
            invite.Email, invite.Role, hasAccount, invite.ExpiresAt);
    }

    private sealed record ResolvedInvite(
        Guid InviteId,
        string ProjectId,
        string ProjectName,
        string Email,
        string Role,
        bool HasAccount,
        DateTime ExpiresAt);
}

/// <summary>POST invites body — Rust <c>CreateInviteRequest</c> {email, role}.</summary>
public sealed record CreateInviteRequest(
    [property: JsonPropertyName("email")] string? Email,
    [property: JsonPropertyName("role")] string Role);

/// <summary>
/// POST /api/invite/{token}/accept body — Rust <c>AcceptInviteRequest</c>.
/// All fields optional; an empty body is valid for JWT-authenticated accepts.
/// </summary>
public sealed record AcceptInviteRequest(
    [property: JsonPropertyName("password")] string? Password,
    [property: JsonPropertyName("email")] string? Email,
    [property: JsonPropertyName("current_password")] string? CurrentPassword);

/// <summary>One row of GET invites — the Rust <c>db::invites::InviteRow</c> serde shape.</summary>
public sealed record InviteRow(
    Guid invite_id,
    string project_id,
    string email,
    string role,
    string status,
    Guid invited_by,
    string invited_by_email,
    DateTime created_at,
    DateTime expires_at,
    DateTime? accepted_at);

/// <summary>
/// Shared token minting/hashing for invites and share links — byte-compatible
/// with the Rust side (32 random bytes → URL-safe base64 no-pad; SHA-256 hex
/// for storage), so tokens issued by either backend resolve on the other.
/// </summary>
public static class CollabTokens
{
    public const int TokenBytes = 32;

    /// <summary>32 CSPRNG bytes as URL-safe base64 without padding (43 chars).</summary>
    public static string Generate()
    {
        var raw = RandomNumberGenerator.GetBytes(TokenBytes);
        return ToUrlSafeBase64NoPad(raw);
    }

    /// <summary>base64url (RFC 4648 §5) without padding — matches Rust URL_SAFE_NO_PAD.</summary>
    public static string ToUrlSafeBase64NoPad(byte[] raw) =>
        Convert.ToBase64String(raw).TrimEnd('=').Replace('+', '-').Replace('/', '_');

    /// <summary>Lowercase SHA-256 hex of the raw token string — matches Rust hash_token.</summary>
    public static string Sha256Hex(string token) =>
        Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(token)));
}

/// <summary>
/// Environment-derived collaboration settings, mirroring the Rust
/// <c>config.rs</c> fields (public_url, share_base_url, share_max_days,
/// invite_expiry_days) and their defaults.
/// </summary>
public static class CollabConfig
{
    public const int DefaultInviteExpiryDays = 7;
    public const int DefaultShareMaxDays = 365;

    /// <summary>DASHBOARD_INVITE_EXPIRY_DAYS (default 7).</summary>
    public static int InviteExpiryDays() =>
        ParseDays(Environment.GetEnvironmentVariable("DASHBOARD_INVITE_EXPIRY_DAYS"), DefaultInviteExpiryDays);

    /// <summary>DASHBOARD_SHARE_MAX_DAYS (default 365).</summary>
    public static int ShareMaxDays() =>
        ParseDays(Environment.GetEnvironmentVariable("DASHBOARD_SHARE_MAX_DAYS"), DefaultShareMaxDays);

    /// <summary>DASHBOARD_PUBLIC_URL, else http://localhost:{DASHBOARD_PORT|3000}.</summary>
    public static string PublicUrl()
    {
        var configured = Environment.GetEnvironmentVariable("DASHBOARD_PUBLIC_URL");
        if (!string.IsNullOrWhiteSpace(configured))
        {
            return configured.TrimEnd('/');
        }

        var port = Environment.GetEnvironmentVariable("DASHBOARD_PORT");
        return $"http://localhost:{(int.TryParse(port, out var p) && p > 0 ? p : 3000)}";
    }

    /// <summary>DASHBOARD_SHARE_URL, else the public URL (Rust share_base_url).</summary>
    public static string ShareBaseUrl()
    {
        var configured = Environment.GetEnvironmentVariable("DASHBOARD_SHARE_URL");
        return string.IsNullOrWhiteSpace(configured) ? PublicUrl() : configured.TrimEnd('/');
    }

    /// <summary>
    /// Rust config parsing: <c>parse().ok().unwrap_or(fallback)</c> — anything
    /// non-numeric or non-positive falls back.
    /// </summary>
    public static int ParseDays(string? raw, int fallback) =>
        int.TryParse(raw, out var days) && days > 0 ? days : fallback;
}
