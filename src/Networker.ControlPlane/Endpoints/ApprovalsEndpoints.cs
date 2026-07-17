using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.Json.Serialization;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Command-approval endpoints — the C# port of the Rust dashboard's
/// <c>api/command_approvals.rs</c> (list_pending / pending_count / decide) plus
/// the <c>api/events.rs</c> <c>GET /events/approval</c> SSE feed.
///
/// <para><b>Contract parity.</b> Response field names are snake_case, matching
/// the Rust <c>db::command_approvals::ApprovalRow</c> serde output field-for-field
/// (including the joined <c>requested_by_email</c> / <c>decided_by_email</c>).
/// The decide body accepts <c>{"approved": bool, "reason"?: string}</c> exactly
/// like Rust's <c>DecideRequest</c>; <c>{"approve": bool}</c> is also accepted
/// as an alias for API ergonomics.</para>
///
/// <para><b>SSE divergence (documented).</b> The Rust feed is push-based: the
/// decide handler broadcasts on <c>state.approval_tx</c> and every subscriber
/// relays the event immediately. The Rust AppState broadcast channel is not
/// ported (the C# realtime layer is SignalR-based); instead this endpoint runs
/// a 3-second poll loop over the caller's projects' pending approval rows and
/// emits an <c>approval</c> event per transition (new pending row appears, or a
/// pending row leaves the set — decided/expired). Worst-case decision latency
/// is therefore one poll interval (~3 s) instead of instant. A comment-line
/// heartbeat is written after 15 s of silence so proxies keep the connection
/// open, and the loop honours request cancellation.</para>
/// </summary>
public static class ApprovalsEndpoints
{
    /// <summary>Poll cadence for the /events/approval SSE loop.</summary>
    public static readonly TimeSpan ApprovalPollInterval = TimeSpan.FromSeconds(3);

    /// <summary>Max silence before a heartbeat comment is emitted.</summary>
    public static readonly TimeSpan HeartbeatInterval = TimeSpan.FromSeconds(15);

    public static IEndpointRouteBuilder MapApprovalsEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/projects/{projectId}/command-approvals — pending approvals
        // (project admin). Mirrors Rust list_pending: status='pending' AND not
        // expired, joined user emails, oldest-first. Response: { approvals: [...] }.
        app.MapGet("/api/projects/{projectId}/command-approvals", async (
            string projectId, NetworkerDbContext db, CancellationToken ct) =>
        {
            var now = DateTime.UtcNow;
            var rows = await PendingWithEmails(db, projectId, now).ToListAsync(ct);
            return Results.Ok(new { approvals = rows.Select(ShapeApproval) });
        })
        .RequireAuthorization(AuthPolicies.ProjectAdmin);

        // GET /api/projects/{projectId}/command-approvals/count — pending count
        // (project admin). Mirrors Rust get_pending_count. Response: { count: n }.
        app.MapGet("/api/projects/{projectId}/command-approvals/count", async (
            string projectId, NetworkerDbContext db, CancellationToken ct) =>
        {
            var now = DateTime.UtcNow;
            var count = await db.CommandApprovals
                .AsNoTracking()
                .Where(a => a.ProjectId == projectId && a.Status == "pending" && a.ExpiresAt > now)
                .LongCountAsync(ct);
            return Results.Ok(new { count });
        })
        .RequireAuthorization(AuthPolicies.ProjectAdmin);

        // POST /api/projects/{projectId}/command-approvals/{approvalId} —
        // approve or deny (project admin). Mirrors Rust decide_approval:
        // 404 unknown/foreign approval, 409 when no longer pending, then the
        // UPDATE stamps status/decided_by/decided_at/reason. Response: { status }.
        app.MapPost("/api/projects/{projectId}/command-approvals/{approvalId:guid}", async (
            string projectId,
            Guid approvalId,
            DecideApprovalRequest body,
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

            if (body.EffectiveApproved() is not { } approved)
            {
                return ApiError.BadRequest("body must carry a boolean 'approved'");
            }

            var approval = await db.CommandApprovals
                .FirstOrDefaultAsync(a => a.ApprovalId == approvalId && a.ProjectId == projectId, ct);
            if (approval is null)
            {
                return ApiError.NotFound("Approval not found");
            }

            if (approval.Status != "pending")
            {
                return ApiError.Conflict("Approval is no longer pending");
            }

            var status = DecisionStatus(approved);
            approval.Status = status;
            approval.DecidedBy = user.UserId;
            approval.DecidedAt = DateTime.UtcNow;
            approval.Reason = body.Reason;
            await db.SaveChangesAsync(ct);

            loggerFactory.CreateLogger("Networker.ControlPlane.Approvals").LogInformation(
                "Command approval {ApprovalId} {Status} by {UserId} (project {ProjectId})",
                approvalId, status, user.UserId, projectId);

            // Rust also broadcasts the decision on approval_tx here; the C#
            // /events/approval poller picks the transition up on its next tick.
            return Results.Ok(new { status });
        })
        .RequireAuthorization(AuthPolicies.ProjectAdmin);

        // GET /api/events/approval — authenticated SSE feed of approval
        // transitions across every project the caller can see (platform admins:
        // all live projects; everyone else: their memberships). See the class
        // doc for the poll-vs-broadcast divergence from Rust.
        app.MapGet("/api/events/approval", async (
            HttpContext ctx, NetworkerDbContext db) =>
        {
            var user = ctx.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }

            var response = ctx.Response;
            response.Headers.ContentType = "text/event-stream";
            response.Headers.CacheControl = "no-cache";
            // Disable proxy buffering so events flush immediately (nginx et al).
            response.Headers["X-Accel-Buffering"] = "no";
            await response.Body.FlushAsync(ctx.RequestAborted);

            var ct = ctx.RequestAborted;
            var lastWrite = DateTimeOffset.UtcNow;

            // Baseline: the pending set at connect time. Only *transitions*
            // after this point are emitted, matching the Rust broadcast (a
            // subscriber never sees historical decisions).
            Dictionary<Guid, string> known;
            try
            {
                known = await PendingSetAsync(db, user, ct);
            }
            catch (OperationCanceledException)
            {
                return Results.Empty;
            }

            try
            {
                while (!ct.IsCancellationRequested)
                {
                    await Task.Delay(ApprovalPollInterval, ct);

                    var current = await PendingSetAsync(db, user, ct);
                    var wrote = false;

                    // New pending approvals (request appeared).
                    foreach (var (id, projectId) in current)
                    {
                        if (known.ContainsKey(id))
                        {
                            continue;
                        }

                        var payload = JsonSerializer.Serialize(new
                        {
                            approval_id = id,
                            project_id = projectId,
                            status = "pending",
                            decided_by = (Guid?)null,
                        });
                        await response.WriteAsync(ServerSentEvents.FormatEvent("approval", payload), ct);
                        wrote = true;
                    }

                    // Approvals that left the pending set (decided or expired) —
                    // fetch their terminal state so the payload matches the Rust
                    // broadcast shape {approval_id, project_id, status, decided_by}.
                    var gone = known.Keys.Where(id => !current.ContainsKey(id)).ToList();
                    if (gone.Count > 0)
                    {
                        var decided = await db.CommandApprovals
                            .AsNoTracking()
                            .Where(a => gone.Contains(a.ApprovalId))
                            .Select(a => new { a.ApprovalId, a.ProjectId, a.Status, a.DecidedBy })
                            .ToListAsync(ct);
                        foreach (var d in decided)
                        {
                            var payload = JsonSerializer.Serialize(new
                            {
                                approval_id = d.ApprovalId,
                                project_id = d.ProjectId,
                                status = d.Status == "pending" ? "expired" : d.Status,
                                decided_by = d.DecidedBy,
                            });
                            await response.WriteAsync(ServerSentEvents.FormatEvent("approval", payload), ct);
                            wrote = true;
                        }
                    }

                    if (wrote)
                    {
                        await response.Body.FlushAsync(ct);
                        lastWrite = DateTimeOffset.UtcNow;
                    }
                    else if (DateTimeOffset.UtcNow - lastWrite >= HeartbeatInterval)
                    {
                        await response.WriteAsync(ServerSentEvents.FormatComment("keep-alive"), ct);
                        await response.Body.FlushAsync(ct);
                        lastWrite = DateTimeOffset.UtcNow;
                    }

                    known = current;
                }
            }
            catch (OperationCanceledException)
            {
                // Client went away — normal SSE termination.
            }

            return Results.Empty;
        })
        .RequireAuthorization();

        return app;
    }

    /// <summary>
    /// Maps the decide flag to the stored status string — the exact mapping of
    /// Rust <c>db::command_approvals::decide</c>: true → "approved", false → "denied".
    /// </summary>
    public static string DecisionStatus(bool approved) => approved ? "approved" : "denied";

    /// <summary>
    /// Pending approvals joined with requester/decider emails — the LINQ mirror
    /// of the Rust <c>SELECT_WITH_JOINS</c> + list_pending WHERE clause.
    /// </summary>
    private static IQueryable<ApprovalWithEmails> PendingWithEmails(
        NetworkerDbContext db, string projectId, DateTime now)
        => from ca in db.CommandApprovals.AsNoTracking()
           where ca.ProjectId == projectId && ca.Status == "pending" && ca.ExpiresAt > now
           join req in db.DashUsers.AsNoTracking() on ca.RequestedBy equals req.UserId
           join dec0 in db.DashUsers.AsNoTracking() on ca.DecidedBy equals (Guid?)dec0.UserId into decJoin
           from dec in decJoin.DefaultIfEmpty()
           orderby ca.RequestedAt
           select new ApprovalWithEmails(ca, req.Email, dec != null ? dec.Email : null);

    private sealed record ApprovalWithEmails(
        Data.Entities.CommandApproval Row, string? RequestedByEmail, string? DecidedByEmail);

    private static object ShapeApproval(ApprovalWithEmails a) => new
    {
        approval_id = a.Row.ApprovalId,
        project_id = a.Row.ProjectId,
        agent_id = a.Row.AgentId,
        command_type = a.Row.CommandType,
        command_detail = ParseJson(a.Row.CommandDetail),
        status = a.Row.Status,
        requested_by = a.Row.RequestedBy,
        requested_by_email = a.RequestedByEmail,
        decided_by = a.Row.DecidedBy,
        decided_by_email = a.DecidedByEmail,
        requested_at = a.Row.RequestedAt,
        decided_at = a.Row.DecidedAt,
        expires_at = a.Row.ExpiresAt,
        reason = a.Row.Reason,
    };

    /// <summary>
    /// The caller's visible pending-approval set: approval_id → project_id for
    /// every non-expired pending approval in the caller's projects (platform
    /// admins see every live project; others see their memberships).
    /// </summary>
    private static async Task<Dictionary<Guid, string>> PendingSetAsync(
        NetworkerDbContext db, AuthUser user, CancellationToken ct)
    {
        var now = DateTime.UtcNow;

        IQueryable<string> projects = user.IsPlatformAdmin
            ? db.Projects.AsNoTracking()
                .Where(p => p.DeletedAt == null)
                .Select(p => p.ProjectId)
            : db.ProjectMembers.AsNoTracking()
                .Where(m => m.UserId == user.UserId)
                .Select(m => m.ProjectId);

        var rows = await db.CommandApprovals
            .AsNoTracking()
            .Where(a => a.Status == "pending" && a.ExpiresAt > now && projects.Contains(a.ProjectId))
            .Select(a => new { a.ApprovalId, a.ProjectId })
            .ToListAsync(ct);

        return rows.ToDictionary(r => r.ApprovalId, r => r.ProjectId);
    }

    /// <summary>
    /// Parse a jsonb text column into a JsonNode so it serializes as a real
    /// object rather than an escaped string (Rust serde_json::Value parity).
    /// </summary>
    private static JsonNode? ParseJson(string? raw)
    {
        if (string.IsNullOrWhiteSpace(raw))
        {
            return null;
        }

        try
        {
            return JsonNode.Parse(raw);
        }
        catch (JsonException)
        {
            return null;
        }
    }
}

/// <summary>
/// POST decide body — Rust <c>DecideRequest { approved, reason }</c>. The
/// <c>approve</c> alias is accepted too (some callers use the imperative form);
/// <c>approved</c> wins when both are present.
/// </summary>
public sealed record DecideApprovalRequest(
    [property: JsonPropertyName("approved")] bool? Approved,
    [property: JsonPropertyName("approve")] bool? Approve,
    [property: JsonPropertyName("reason")] string? Reason)
{
    public bool? EffectiveApproved() => Approved ?? Approve;
}

/// <summary>
/// Minimal SSE wire-format helpers shared by the approval feed and the
/// agent-command stream. Kept as pure string formatting so it is unit-testable
/// without an HttpContext.
/// </summary>
public static class ServerSentEvents
{
    /// <summary>
    /// One SSE event frame: optional <c>event:</c> line, then <c>data:</c>
    /// line(s), then the blank-line terminator. Multi-line data is split into
    /// one <c>data:</c> line per line, per the SSE spec.
    /// </summary>
    public static string FormatEvent(string? eventName, string data)
    {
        var sb = new System.Text.StringBuilder();
        if (!string.IsNullOrEmpty(eventName))
        {
            sb.Append("event: ").Append(eventName).Append('\n');
        }

        foreach (var line in data.Split('\n'))
        {
            sb.Append("data: ").Append(line).Append('\n');
        }

        sb.Append('\n');
        return sb.ToString();
    }

    /// <summary>A comment frame (heartbeat) — ignored by EventSource clients.</summary>
    public static string FormatComment(string comment) => $": {comment}\n\n";
}
