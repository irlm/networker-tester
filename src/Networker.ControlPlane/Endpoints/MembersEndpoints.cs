using System.Text.Json.Serialization;
using Microsoft.AspNetCore.Mvc;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Phase-2 M5: PROJECT MEMBERS — the C# port of the Rust
/// <c>crates/networker-dashboard/src/api/project_members.rs</c> (list / add /
/// update-role / remove), <c>.../api/pending_projects.rs</c> (accept / deny +
/// GET /api/me/pending-projects) and <c>.../api/member_import.rs</c>
/// (bulk import + send-invites).
///
/// JSON field names are snake_case to match the Rust serde shapes exactly
/// (<c>ProjectMemberRow</c>, <c>PendingProject</c>, <c>ImportResponse</c>,
/// <c>SendInvitesResponse</c>). Status codes mirror the Rust handlers:
/// last-admin guards are 400 (not 409), a missing member on DELETE is 400
/// (Rust maps every <c>Err(msg)</c> from <c>remove_member</c> to 400), and a
/// missing pending membership on accept/deny is 404.
///
/// Divergence from Rust (noted per the migration plan):
/// <list type="bullet">
///   <item>POST /members/import takes a JSON array of <c>{email, role}</c>
///         rows instead of a multipart CSV upload — the per-row semantics and
///         the response shape are identical.</item>
///   <item>send-invites does not send email — the ACS integration is a later
///         pass; it logs a TODO and returns <c>invite_url</c> when email is
///         not configured, exactly like the Rust fallback path.</item>
/// </list>
/// </summary>
public static class MembersEndpoints
{
    private static readonly string[] ValidRoles = ["admin", "operator", "viewer"];

    public static IEndpointRouteBuilder MapMembersEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/projects/{projectId}/members — list members (project admin).
        // Mirrors Rust list_members → db::projects::list_members: inner join
        // dash_user for email/display_name (never password_hash), ordered by
        // joined_at, wrapped as { "members": [...] }.
        app.MapGet("/api/projects/{projectId}/members", async (
            string projectId,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var members = await db.ProjectMembers
                .AsNoTracking()
                .Where(m => m.ProjectId == projectId)
                .Join(
                    db.DashUsers,
                    m => m.UserId,
                    u => u.UserId,
                    (m, u) => new MemberRow(
                        m.ProjectId,
                        m.UserId,
                        m.Role,
                        m.JoinedAt,
                        m.InvitedBy,
                        u.Email ?? string.Empty,
                        u.DisplayName,
                        m.Status,
                        m.InviteSentAt))
                .OrderBy(x => x.joined_at)
                .ToListAsync(ct);

            return Results.Ok(new { members });
        }).RequireAuthorization(AuthPolicies.ProjectAdmin);

        // POST /api/projects/{projectId}/members — add a member (project admin).
        // Mirrors Rust add_member: validate role, resolve the user by email
        // (case-insensitive; 404 when no account), then upsert the membership
        // (INSERT ... ON CONFLICT DO UPDATE SET role). Returns 201 {success:true}.
        app.MapPost("/api/projects/{projectId}/members", async (
            string projectId,
            [FromBody] AddMemberRequest req,
            HttpContext ctx,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var user = ctx.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }

            if (!ValidRoles.Contains(req.Role))
            {
                return ApiError.BadRequest("Role must be admin, operator, or viewer");
            }

            var email = req.Email ?? string.Empty;
            var target = await db.DashUsers
                .AsNoTracking()
                .Where(u => u.Email != null && u.Email.ToLower() == email.ToLower())
                .Select(u => new { u.UserId })
                .FirstOrDefaultAsync(ct);

            if (target is null)
            {
                return ApiError.NotFound("User not found with that email");
            }

            var existing = await db.ProjectMembers
                .FirstOrDefaultAsync(m => m.ProjectId == projectId && m.UserId == target.UserId, ct);
            if (existing is null)
            {
                // status is left to the DB default 'active' (Rust INSERT omits it).
                db.ProjectMembers.Add(new ProjectMember
                {
                    ProjectId = projectId,
                    UserId = target.UserId,
                    Role = req.Role,
                    JoinedAt = DateTime.UtcNow,
                    InvitedBy = user.UserId,
                });
            }
            else
            {
                // ON CONFLICT (project_id, user_id) DO UPDATE SET role — role only.
                existing.Role = req.Role;
            }

            await db.SaveChangesAsync(ct);
            return Results.Json(new { success = true }, statusCode: StatusCodes.Status201Created);
        }).RequireAuthorization(AuthPolicies.ProjectAdmin);

        // PUT /api/projects/{projectId}/members/{memberUserId} — change role.
        // Mirrors Rust update_member_role: validate role; refuse demoting the
        // last admin (400); unknown member → 404; success → {success:true}.
        app.MapPut("/api/projects/{projectId}/members/{memberUserId:guid}", async (
            string projectId,
            Guid memberUserId,
            [FromBody] UpdateMemberRoleRequest req,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            if (!ValidRoles.Contains(req.Role))
            {
                return ApiError.BadRequest("Role must be admin, operator, or viewer");
            }

            var member = await db.ProjectMembers
                .FirstOrDefaultAsync(m => m.ProjectId == projectId && m.UserId == memberUserId, ct);
            if (member is null)
            {
                return ApiError.NotFound("Member not found");
            }

            // Demoting an admin: make sure they aren't the last one.
            if (req.Role != "admin" && member.Role == "admin")
            {
                var adminCount = await db.ProjectMembers
                    .CountAsync(m => m.ProjectId == projectId && m.Role == "admin", ct);
                if (adminCount <= 1)
                {
                    return ApiError.BadRequest("Cannot demote the last admin");
                }
            }

            member.Role = req.Role;
            await db.SaveChangesAsync(ct);
            return Results.Ok(new { success = true });
        }).RequireAuthorization(AuthPolicies.ProjectAdmin);

        // DELETE /api/projects/{projectId}/members/{memberUserId} — remove.
        // Mirrors Rust remove_member: refuse removing the last admin (400);
        // Rust maps BOTH guard failures and "Member not found" to 400.
        app.MapDelete("/api/projects/{projectId}/members/{memberUserId:guid}", async (
            string projectId,
            Guid memberUserId,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var member = await db.ProjectMembers
                .FirstOrDefaultAsync(m => m.ProjectId == projectId && m.UserId == memberUserId, ct);
            if (member is null)
            {
                return ApiError.BadRequest("Member not found");
            }

            if (member.Role == "admin")
            {
                var adminCount = await db.ProjectMembers
                    .CountAsync(m => m.ProjectId == projectId && m.Role == "admin", ct);
                if (adminCount <= 1)
                {
                    return ApiError.BadRequest("Cannot remove the last admin from a project");
                }
            }

            db.ProjectMembers.Remove(member);
            await db.SaveChangesAsync(ct);
            return Results.Ok(new { success = true });
        }).RequireAuthorization(AuthPolicies.ProjectAdmin);

        // PUT /api/projects/{projectId}/members/me/accept — accept a pending
        // membership. Mirrors Rust accept_membership (mounted in protected_flat:
        // any authenticated user; the pending row itself is the authorization).
        // Only transitions 'pending_acceptance' → 'active'; nothing pending → 404.
        app.MapPut("/api/projects/{projectId}/members/me/accept", (
            string projectId, HttpContext ctx, NetworkerDbContext db, CancellationToken ct) =>
                UpdateOwnMembershipStatusAsync(projectId, ctx, db, "active", ct))
            .RequireAuthorization();

        // PUT /api/projects/{projectId}/members/me/deny — decline a pending
        // membership (Rust deny_membership, same router as accept).
        app.MapPut("/api/projects/{projectId}/members/me/deny", (
            string projectId, HttpContext ctx, NetworkerDbContext db, CancellationToken ct) =>
                UpdateOwnMembershipStatusAsync(projectId, ctx, db, "denied", ct))
            .RequireAuthorization();

        // GET /api/me/pending-projects — the caller's pending memberships.
        // Mirrors Rust list_pending_projects: join project for the name, LEFT
        // JOIN dash_user for the inviter's email, newest first, wrapped as
        // { "pending": [...] } (invited_at is the membership's joined_at).
        app.MapGet("/api/me/pending-projects", async (
            HttpContext ctx,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var user = ctx.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }

            var pending = await (
                from pm in db.ProjectMembers.AsNoTracking()
                where pm.UserId == user.UserId && pm.Status == "pending_acceptance"
                join p in db.Projects on pm.ProjectId equals p.ProjectId
                join inviter in db.DashUsers on pm.InvitedBy equals (Guid?)inviter.UserId into gj
                from inviter in gj.DefaultIfEmpty()
                orderby pm.JoinedAt descending
                select new PendingProjectRow(
                    pm.ProjectId,
                    p.Name,
                    pm.Role,
                    inviter != null ? inviter.Email : null,
                    pm.JoinedAt)).ToListAsync(ct);

            return Results.Ok(new { pending });
        }).RequireAuthorization();

        // POST /api/projects/{projectId}/members/import — bulk invite (admin).
        // Rust member_import::import_members takes a multipart CSV; this port
        // takes a JSON array of {email, role} rows (same per-row pipeline:
        // validate → create placeholder dash_user if needed → add pending
        // member) and returns the identical ImportResponse shape.
        app.MapPost("/api/projects/{projectId}/members/import", async (
            string projectId,
            [FromBody] List<ImportMemberRow> rows,
            HttpContext ctx,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var user = ctx.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }

            if (rows is null || rows.Count == 0)
            {
                // Rust: empty CSV → 400 "No file field found or file is empty".
                return ApiError.BadRequest("No members provided");
            }

            var imported = 0;
            var skipped = 0;
            var errors = 0;
            var details = new List<ImportDetail>();

            foreach (var row in rows)
            {
                var email = (row.Email ?? string.Empty).Trim();
                var role = (row.Role ?? string.Empty).Trim().ToLowerInvariant();

                if (!email.Contains('@') || email.Length < 3)
                {
                    errors++;
                    details.Add(new ImportDetail(email, "error", "Invalid email format"));
                    continue;
                }

                if (!ValidRoles.Contains(role))
                {
                    errors++;
                    details.Add(new ImportDetail(
                        email, "error", $"Invalid role '{role}' (must be admin, operator, or viewer)"));
                    continue;
                }

                try
                {
                    var userId = await GetOrCreatePlaceholderUserAsync(db, email, ct);
                    var (result, resultLabel, message) =
                        await AddPendingMemberAsync(db, projectId, userId, role, user.UserId, ct);

                    if (result)
                    {
                        imported++;
                    }
                    else
                    {
                        skipped++;
                    }

                    details.Add(new ImportDetail(email, resultLabel, message));
                }
                catch (Exception ex) when (ex is DbUpdateException or InvalidOperationException)
                {
                    errors++;
                    details.Add(new ImportDetail(email, "error", "Failed to add member"));
                }
            }

            return Results.Ok(new ImportResponse(imported, skipped, errors, details));
        }).RequireAuthorization(AuthPolicies.ProjectAdmin);

        // POST /api/projects/{projectId}/members/send-invites — (re)send invite
        // links for pending members (admin). Mirrors Rust member_import::send_invites:
        // per user_id — verify pending_acceptance, mint a workspace_invite row with
        // a fresh token, mark invite_sent_at = now. Email sending is a TODO stub
        // (logged); when email is not configured the invite_url is returned so
        // admins can copy it manually (identical to the Rust fallback).
        app.MapPost("/api/projects/{projectId}/members/send-invites", async (
            string projectId,
            [FromBody] SendInvitesRequest req,
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

            var logger = loggerFactory.CreateLogger("Networker.ControlPlane.MembersEndpoints");

            // Same detection the Rust side uses (both ACS vars must be present).
            var emailConfigured =
                !string.IsNullOrEmpty(Environment.GetEnvironmentVariable("DASHBOARD_ACS_CONNECTION_STRING"))
                && !string.IsNullOrEmpty(Environment.GetEnvironmentVariable("DASHBOARD_ACS_SENDER"));

            var expiryDays = CollabConfig.InviteExpiryDays();
            var expiresAt = DateTime.UtcNow.AddDays(expiryDays);

            var sent = 0;
            var skipped = 0;
            var errors = 0;
            var details = new List<InviteDetail>();

            foreach (var userId in req.UserIds ?? [])
            {
                var member = await (
                    from pm in db.ProjectMembers.AsNoTracking()
                    join u in db.DashUsers on pm.UserId equals u.UserId
                    where pm.ProjectId == projectId && pm.UserId == userId
                    select new { u.Email, pm.Role, pm.Status }).FirstOrDefaultAsync(ct);

                if (member is null)
                {
                    errors++;
                    details.Add(new InviteDetail(
                        userId, string.Empty, "error", "User is not a member of this project", null));
                    continue;
                }

                var email = member.Email ?? string.Empty;
                if (member.Status != "pending_acceptance")
                {
                    skipped++;
                    details.Add(new InviteDetail(
                        userId, email, "skipped",
                        $"Member status is '{member.Status}', not pending_acceptance", null));
                    continue;
                }

                var rawToken = CollabTokens.Generate();
                db.WorkspaceInvites.Add(new WorkspaceInvite
                {
                    InviteId = Guid.NewGuid(),
                    ProjectId = projectId,
                    Email = email,
                    Role = member.Role,
                    TokenHash = CollabTokens.Sha256Hex(rawToken),
                    Status = "pending",
                    InvitedBy = user.UserId,
                    ExpiresAt = expiresAt,
                });

                var inviteUrl = $"{CollabConfig.PublicUrl()}/invite/{rawToken}";

                if (emailConfigured)
                {
                    // TODO(phase2): wire IEmailSender (Azure Communication
                    // Services) here. Until then the invite exists and
                    // resolves; only the email delivery is stubbed.
                    logger.LogWarning(
                        "send-invites: email sending not yet implemented in the C# control plane " +
                        "(would send invite to {Email} for project {ProjectId})", email, projectId);
                }

                await db.ProjectMembers
                    .Where(m => m.ProjectId == projectId && m.UserId == userId)
                    .ExecuteUpdateAsync(s => s.SetProperty(m => m.InviteSentAt, DateTime.UtcNow), ct);

                sent++;
                details.Add(new InviteDetail(
                    userId,
                    email,
                    "sent",
                    emailConfigured
                        ? "Invite email sent"
                        : "Invite created (email not configured — use invite_url)",
                    emailConfigured ? null : inviteUrl));
            }

            await db.SaveChangesAsync(ct);
            return Results.Ok(new SendInvitesResponse(sent, skipped, errors, emailConfigured, details));
        }).RequireAuthorization(AuthPolicies.ProjectAdmin);

        return app;
    }

    /// <summary>
    /// Shared body of accept/deny: transition the caller's own membership from
    /// 'pending_acceptance' to <paramref name="newStatus"/> (Rust
    /// db::projects::update_member_status). No pending row → 404.
    /// </summary>
    private static async Task<IResult> UpdateOwnMembershipStatusAsync(
        string projectId, HttpContext ctx, NetworkerDbContext db, string newStatus, CancellationToken ct)
    {
        var user = ctx.GetAuthUser();
        if (user is null)
        {
            return Results.Unauthorized();
        }

        var updated = await db.ProjectMembers
            .Where(m => m.ProjectId == projectId
                        && m.UserId == user.UserId
                        && m.Status == "pending_acceptance")
            .ExecuteUpdateAsync(s => s.SetProperty(m => m.Status, newStatus), ct);

        if (updated == 0)
        {
            return ApiError.NotFound("No pending membership found");
        }

        return newStatus == "active"
            ? Results.Ok(new { accepted = true })
            : Results.Ok(new { denied = true });
    }

    /// <summary>
    /// Rust db::users::create_placeholder_user — reuse the account if the email
    /// already exists, else create an active passwordless user (role and
    /// auth_provider fall to the DB defaults, same as the Rust INSERT).
    /// </summary>
    private static async Task<Guid> GetOrCreatePlaceholderUserAsync(
        NetworkerDbContext db, string email, CancellationToken ct)
    {
        var existing = await db.DashUsers
            .AsNoTracking()
            .Where(u => u.Email != null && u.Email.ToLower() == email.ToLower())
            .Select(u => (Guid?)u.UserId)
            .FirstOrDefaultAsync(ct);
        if (existing is { } userId)
        {
            return userId;
        }

        var placeholder = new DashUser
        {
            UserId = Guid.NewGuid(),
            Email = email,
            Status = "active",
            MustChangePassword = false,
        };
        db.DashUsers.Add(placeholder);
        await db.SaveChangesAsync(ct);
        return placeholder.UserId;
    }

    /// <summary>
    /// Rust db::projects::add_pending_member — returns (countsAsImported,
    /// result label, message) matching the Rust AddMemberResult mapping.
    /// </summary>
    private static async Task<(bool Imported, string Label, string Message)> AddPendingMemberAsync(
        NetworkerDbContext db, string projectId, Guid userId, string role, Guid invitedBy,
        CancellationToken ct)
    {
        var existing = await db.ProjectMembers
            .FirstOrDefaultAsync(m => m.ProjectId == projectId && m.UserId == userId, ct);

        if (existing is null)
        {
            db.ProjectMembers.Add(new ProjectMember
            {
                ProjectId = projectId,
                UserId = userId,
                Role = role,
                InvitedBy = invitedBy,
                Status = "pending_acceptance",
                JoinedAt = DateTime.UtcNow,
            });
            await db.SaveChangesAsync(ct);
            return (true, "invited", $"New user created + invited as {role}");
        }

        switch (existing.Status)
        {
            case "active":
                return (false, "already_member", "Already active member");
            case "pending_acceptance":
                return (false, "already_pending", "Already has pending invitation");
            case "denied":
                // Re-invite: reset to pending with the new role/inviter.
                existing.Status = "pending_acceptance";
                existing.Role = role;
                existing.InvitedBy = invitedBy;
                existing.JoinedAt = DateTime.UtcNow;
                await db.SaveChangesAsync(ct);
                return (true, "re_invited", $"Re-invited as {role} (was previously denied)");
            default:
                return (false, "already_member", "Already active member");
        }
    }
}

/// <summary>POST members body — Rust <c>AddMemberRequest</c> {email, role}.</summary>
public sealed record AddMemberRequest(
    [property: JsonPropertyName("email")] string? Email,
    [property: JsonPropertyName("role")] string Role);

/// <summary>PUT members/{uid} body — Rust <c>UpdateMemberRoleRequest</c> {role}.</summary>
public sealed record UpdateMemberRoleRequest(
    [property: JsonPropertyName("role")] string Role);

/// <summary>
/// One row of GET members — the Rust <c>db::projects::ProjectMemberRow</c> serde
/// shape (project_id, user_id, role, joined_at, invited_by, email, display_name,
/// status, invite_sent_at). password_hash is never selected.
/// </summary>
public sealed record MemberRow(
    string project_id,
    Guid user_id,
    string role,
    DateTime joined_at,
    Guid? invited_by,
    string email,
    string? display_name,
    string status,
    DateTime? invite_sent_at);

/// <summary>GET /api/me/pending-projects row — Rust <c>PendingProject</c>.</summary>
public sealed record PendingProjectRow(
    string project_id,
    string project_name,
    string role,
    string? invited_by_email,
    DateTime invited_at);

/// <summary>POST members/import row — {email, role} (CSV column pair in Rust).</summary>
public sealed record ImportMemberRow(
    [property: JsonPropertyName("email")] string? Email,
    [property: JsonPropertyName("role")] string? Role);

/// <summary>Per-row import outcome — Rust <c>ImportDetail</c>.</summary>
public sealed record ImportDetail(string email, string result, string message);

/// <summary>POST members/import response — Rust <c>ImportResponse</c>.</summary>
public sealed record ImportResponse(int imported, int skipped, int errors, List<ImportDetail> details);

/// <summary>POST members/send-invites body — Rust <c>SendInvitesRequest</c>.</summary>
public sealed record SendInvitesRequest(
    [property: JsonPropertyName("user_ids")] List<Guid>? UserIds);

/// <summary>Per-user send-invites outcome — Rust <c>InviteDetail</c> (invite_url omitted when null).</summary>
public sealed record InviteDetail(
    Guid user_id,
    string email,
    string result,
    string message,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] string? invite_url);

/// <summary>POST members/send-invites response — Rust <c>SendInvitesResponse</c>.</summary>
public sealed record SendInvitesResponse(
    int sent,
    int skipped,
    int errors,
    bool email_configured,
    List<InviteDetail> details);
