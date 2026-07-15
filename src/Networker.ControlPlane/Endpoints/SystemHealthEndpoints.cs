using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// C# port of the Rust dashboard's <c>api/system_health.rs</c> admin-only
/// <c>GET /api/system/health</c> handler (detailed internal health overview).
///
/// <para>The Rust module also exposes a PUBLIC <c>GET /api/health</c> liveness
/// endpoint via its <c>public_router</c>; that route is already served by
/// Program.cs in the C# ControlPlane (the deploy health check / connection dot),
/// so it is intentionally NOT re-mapped here to avoid a duplicate-route
/// conflict. Only the admin overview is ported in this file.</para>
///
/// <para>Auth: the Rust handler requires a valid JWT and then rejects
/// non-<c>is_platform_admin</c> callers with 403 FORBIDDEN. Because
/// <c>is_platform_admin</c> is a distinct flag from the global role hierarchy,
/// this is enforced with an inline <c>IsPlatformAdmin</c> check rather than the
/// <c>GlobalAdmin</c> (role=Admin) policy.</para>
///
/// <para>Response shape (matches Rust): <c>{ "live": { "core_db": bool,
/// "logs_db": bool }, "checks": [ HealthCheck... ] }</c>. Each check row is
/// snake_case: check_name, status, value, message, details, checked_at.</para>
///
/// <para>Second-DB divergence: the Rust <c>live.logs_db</c> probes a separate
/// logs pool. The C# ControlPlane uses a single <see cref="NetworkerDbContext"/>
/// against the core DB, so <c>logs_db</c> reflects the same core connection
/// (documented divergence — there is no split logs pool in the C# port).</para>
/// </summary>
public static class SystemHealthEndpoints
{
    public static IEndpointRouteBuilder MapSystemHealthEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/system/health — admin-only system health overview.
        app.MapGet("/api/system/health", async (
            HttpContext ctx,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var user = ctx.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }
            if (!user.IsPlatformAdmin)
            {
                return Results.StatusCode(StatusCodes.Status403Forbidden);
            }

            // latest_all: DISTINCT ON (check_name) ordered by check_name,
            // checked_at DESC — i.e. the most recent row per check_name.
            var latest = await db.SystemHealths
                .AsNoTracking()
                .GroupBy(h => h.CheckName)
                .Select(g => g
                    .OrderByDescending(h => h.CheckedAt)
                    .First())
                .OrderBy(h => h.CheckName)
                .ToListAsync(ct);

            var checks = latest.Select(h => new
            {
                check_name = h.CheckName,
                status = h.Status,
                value = h.Value,
                message = h.Message,
                details = RawJsonOrNull(h.Details),
                checked_at = h.CheckedAt,
            });

            // live.core_db / live.logs_db — both probe the single core DB in the
            // C# port (no split logs pool; see class remarks).
            bool coreLive;
            try
            {
                coreLive = await db.Database.CanConnectAsync(ct);
            }
            catch (Exception)
            {
                coreLive = false;
            }

            return Results.Ok(new
            {
                live = new
                {
                    core_db = coreLive,
                    logs_db = coreLive,
                },
                checks,
            });
        }).RequireAuthorization();

        return app;
    }

    /// <summary>
    /// Emit a JSONB-as-text <c>details</c> column as raw JSON so it matches the
    /// Rust <c>serde_json::Value</c> field, or null when absent.
    /// </summary>
    private static object? RawJsonOrNull(string? value)
    {
        if (value is null)
        {
            return null;
        }

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
}
