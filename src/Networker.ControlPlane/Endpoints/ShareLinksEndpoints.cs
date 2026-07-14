using System.Text.Json;
using System.Text.Json.Serialization;
using Microsoft.AspNetCore.Mvc;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Phase-2 M5: SHARE LINKS — the C# port of the Rust
/// <c>crates/networker-dashboard/src/api/share_links.rs</c>.
///
/// Project-scoped (admin): create / list / extend-or-revoke (PUT with an
/// <c>action</c> discriminator, same as Rust) / delete. Public (no auth):
/// GET /api/share/{token} resolves the token to the shared resource.
///
/// Tokens share the invite machinery (<see cref="CollabTokens"/>): 32 CSPRNG
/// bytes → URL-safe base64 no-pad, SHA-256 hex stored. Public resolve bumps
/// access_count/last_accessed atomically-enough (single UPDATE in Rust; load +
/// save here), and expired/revoked/unknown tokens are all the same 404 —
/// exactly the Rust behavior (no 410; no oracle for why the link is dead).
///
/// v2 supports only <c>resource_type = "run"</c> (the legacy "job" type died
/// with the polymorphic job table in v0.28).
/// </summary>
public static class ShareLinksEndpoints
{
    public static IEndpointRouteBuilder MapShareLinksEndpoints(this IEndpointRouteBuilder app)
    {
        // POST /api/projects/{projectId}/share-links — create (project admin).
        // Mirrors Rust create_share_link: resource_type must be "run";
        // expires_in_days ∈ [1, DASHBOARD_SHARE_MAX_DAYS (default 365)].
        // Returns 200 { link_id, url, expires_at, label }.
        app.MapPost("/api/projects/{projectId}/share-links", async (
            string projectId,
            [FromBody] CreateShareLinkRequest req,
            HttpContext ctx,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var user = ctx.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }

            if (req.ResourceType != "run")
            {
                return Results.BadRequest(new { error = "resource_type must be 'run'" });
            }

            var maxDays = CollabConfig.ShareMaxDays();
            if (!ShareLinkRules.IsValidExpiryDays(req.ExpiresInDays, maxDays))
            {
                return Results.BadRequest(new
                {
                    error = $"expires_in_days must be between 1 and {maxDays}",
                });
            }

            var rawToken = CollabTokens.Generate();
            var expiresAt = DateTime.UtcNow.AddDays(req.ExpiresInDays);

            var link = new ShareLink
            {
                LinkId = Guid.NewGuid(),
                ProjectId = projectId,
                TokenHash = CollabTokens.Sha256Hex(rawToken),
                ResourceType = req.ResourceType,
                ResourceId = req.ResourceId,
                Label = req.Label,
                ExpiresAt = expiresAt,
                CreatedBy = user.UserId,
            };
            db.ShareLinks.Add(link);
            await db.SaveChangesAsync(ct);

            return Results.Ok(new
            {
                link_id = link.LinkId.ToString(),
                url = $"{CollabConfig.ShareBaseUrl()}/share/{rawToken}",
                expires_at = expiresAt,
                label = req.Label,
            });
        }).RequireAuthorization(AuthPolicies.ProjectAdmin);

        // GET /api/projects/{projectId}/share-links — list (project admin).
        // Mirrors Rust list_share_links → db::share_links::list_links: bare
        // array of the full row (token_hash included — it's the hash, the raw
        // token is never stored), created_by_email COALESCE'd to "unknown".
        app.MapGet("/api/projects/{projectId}/share-links", async (
            string projectId,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var links = await (
                from s in db.ShareLinks.AsNoTracking()
                join u in db.DashUsers on s.CreatedBy equals u.UserId into gj
                from u in gj.DefaultIfEmpty()
                where s.ProjectId == projectId
                orderby s.CreatedAt descending
                select new ShareLinkRow(
                    s.LinkId,
                    s.ProjectId,
                    s.TokenHash,
                    s.ResourceType,
                    s.ResourceId,
                    s.Label,
                    s.ExpiresAt,
                    s.CreatedBy,
                    s.CreatedAt,
                    s.Revoked,
                    s.AccessCount,
                    s.LastAccessed,
                    u != null && u.Email != null ? u.Email : "unknown")).ToListAsync(ct);

            return Results.Ok(links);
        }).RequireAuthorization(AuthPolicies.ProjectAdmin);

        // PUT /api/projects/{projectId}/share-links/{linkId} — extend or revoke
        // (project admin). Mirrors Rust update_share_link: action ∈
        // {"extend","revoke"}; extend defaults to 30 days and re-validates the
        // cap; both respond success regardless of affected-row count (Rust
        // ignores it). Divergence: Rust's extend UPDATE forgot the project_id
        // scope — this port scopes both actions to the route's project.
        app.MapPut("/api/projects/{projectId}/share-links/{linkId:guid}", async (
            string projectId,
            Guid linkId,
            [FromBody] UpdateShareLinkRequest req,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            switch (req.Action)
            {
                case "revoke":
                    await db.ShareLinks
                        .Where(s => s.LinkId == linkId && s.ProjectId == projectId)
                        .ExecuteUpdateAsync(s => s.SetProperty(l => l.Revoked, true), ct);
                    return Results.Ok(new { revoked = true });

                case "extend":
                    var days = req.ExpiresInDays ?? 30;
                    var maxDays = CollabConfig.ShareMaxDays();
                    if (!ShareLinkRules.IsValidExpiryDays(days, maxDays))
                    {
                        return Results.BadRequest(new
                        {
                            error = $"expires_in_days must be between 1 and {maxDays}",
                        });
                    }

                    var newExpires = DateTime.UtcNow.AddDays(days);
                    await db.ShareLinks
                        .Where(s => s.LinkId == linkId && s.ProjectId == projectId)
                        .ExecuteUpdateAsync(s => s.SetProperty(l => l.ExpiresAt, newExpires), ct);
                    return Results.Ok(new { extended = true, expires_at = newExpires });

                default:
                    return Results.BadRequest(new { error = "action must be 'extend' or 'revoke'" });
            }
        }).RequireAuthorization(AuthPolicies.ProjectAdmin);

        // DELETE /api/projects/{projectId}/share-links/{linkId} — hard delete
        // (project admin). Mirrors Rust delete_share_link; { deleted: true }
        // regardless of affected-row count.
        app.MapDelete("/api/projects/{projectId}/share-links/{linkId:guid}", async (
            string projectId,
            Guid linkId,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            await db.ShareLinks
                .Where(s => s.LinkId == linkId && s.ProjectId == projectId)
                .ExecuteDeleteAsync(ct);

            return Results.Ok(new { deleted = true });
        }).RequireAuthorization(AuthPolicies.ProjectAdmin);

        // GET /api/share/{token} — PUBLIC resolve. Mirrors Rust
        // resolve_share_link: valid (not revoked, not expired) token → bump
        // access counters → load the run (+ newest artifact) → wrap in the
        // descriptor. Missing run / unknown resource type / dead token → 404.
        // Nothing beyond the descriptor + run payload leaks (no project
        // settings, no member info).
        app.MapGet("/api/share/{token}", async (
            string token,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var tokenHash = CollabTokens.Sha256Hex(token);
            var now = DateTime.UtcNow;

            var link = await db.ShareLinks
                .FirstOrDefaultAsync(s => s.TokenHash == tokenHash && !s.Revoked && s.ExpiresAt > now, ct);
            if (link is null)
            {
                return Results.NotFound(new { error = "Link expired or invalid" });
            }

            // Rust: UPDATE ... RETURNING bumps the counters on every resolve.
            link.AccessCount += 1;
            link.LastAccessed = now;
            await db.SaveChangesAsync(ct);

            var sharedBy = await db.DashUsers
                .AsNoTracking()
                .Where(u => u.UserId == link.CreatedBy)
                .Select(u => u.Email)
                .FirstOrDefaultAsync(ct) ?? "unknown";

            if (link.ResourceType != "run")
            {
                return Results.NotFound(new { error = "Unknown resource type" });
            }

            if (link.ResourceId is not { } runId)
            {
                return Results.NotFound(new { error = "Resource not found" });
            }

            // networker_common::TestRun serde shape (same projection as the
            // authenticated GET /api/v2/test-runs/{id}).
            var run = await db.TestRuns
                .AsNoTracking()
                .Where(r => r.Id == runId)
                .Select(r => new
                {
                    id = r.Id,
                    test_config_id = r.TestConfigId,
                    project_id = r.ProjectId,
                    status = r.Status,
                    started_at = r.StartedAt,
                    finished_at = r.FinishedAt,
                    success_count = r.SuccessCount,
                    failure_count = r.FailureCount,
                    error_message = r.ErrorMessage,
                    artifact_id = r.ArtifactId,
                    tester_id = r.TesterId,
                    worker_id = r.WorkerId,
                    last_heartbeat = r.LastHeartbeat,
                    created_at = r.CreatedAt,
                    comparison_group_id = r.ComparisonGroupId,
                })
                .FirstOrDefaultAsync(ct);

            if (run is null)
            {
                return Results.NotFound(new { error = "Resource not found" });
            }

            // Newest artifact for the run, if any (db::benchmark_artifacts::get_for_run).
            var art = await db.BenchmarkArtifacts
                .AsNoTracking()
                .Where(a => a.TestRunId == runId)
                .OrderByDescending(a => a.CreatedAt)
                .FirstOrDefaultAsync(ct);

            object? artifact = art is null
                ? null
                : new
                {
                    id = art.Id,
                    test_run_id = art.TestRunId,
                    environment = RawJson(art.Environment),
                    methodology = RawJson(art.Methodology),
                    launches = RawJson(art.Launches),
                    cases = RawJson(art.Cases),
                    samples = RawJsonOrNull(art.Samples),
                    summaries = RawJson(art.Summaries),
                    data_quality = RawJson(art.DataQuality),
                    created_at = art.CreatedAt,
                };

            return Results.Ok(new
            {
                resource_type = link.ResourceType,
                resource_id = link.ResourceId?.ToString(),
                label = link.Label,
                data = new { run, artifact },
                shared_by = sharedBy,
                expires_at = link.ExpiresAt,
            });
        }).AllowAnonymous();

        return app;
    }

    // Parse a JSONB-as-text column into a JsonElement so it serializes as raw
    // JSON rather than an escaped string (same helper as TestRunsEndpoints).
    private static object RawJson(string value)
    {
        try
        {
            using var doc = JsonDocument.Parse(value);
            return doc.RootElement.Clone();
        }
        catch (JsonException)
        {
            return value;
        }
    }

    private static object? RawJsonOrNull(string? value) =>
        value is null ? null : RawJson(value);
}

/// <summary>
/// Pure share-link validation rules (unit-testable; mirrors the Rust guard
/// <c>expires_in_days == 0 || expires_in_days &gt; share_max_days</c>).
/// </summary>
public static class ShareLinkRules
{
    public static bool IsValidExpiryDays(int days, int maxDays) =>
        days >= 1 && days <= maxDays;
}

/// <summary>POST share-links body — Rust <c>CreateShareLinkRequest</c>.</summary>
public sealed record CreateShareLinkRequest(
    [property: JsonPropertyName("resource_type")] string? ResourceType,
    [property: JsonPropertyName("resource_id")] Guid ResourceId,
    [property: JsonPropertyName("label")] string? Label,
    [property: JsonPropertyName("expires_in_days")] int ExpiresInDays);

/// <summary>PUT share-links/{id} body — Rust <c>UpdateShareLinkRequest</c>.</summary>
public sealed record UpdateShareLinkRequest(
    [property: JsonPropertyName("action")] string? Action,
    [property: JsonPropertyName("expires_in_days")] int? ExpiresInDays);

/// <summary>One row of GET share-links — the Rust <c>ShareLinkRow</c> serde shape.</summary>
public sealed record ShareLinkRow(
    Guid link_id,
    string project_id,
    string token_hash,
    string resource_type,
    Guid? resource_id,
    string? label,
    DateTime expires_at,
    Guid created_by,
    DateTime created_at,
    bool revoked,
    int access_count,
    DateTime? last_accessed,
    string created_by_email);
