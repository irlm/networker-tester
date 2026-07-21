using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.Json.Serialization;
using Microsoft.AspNetCore.Mvc;
using Microsoft.EntityFrameworkCore;
using Npgsql;
using Networker.ControlPlane.Auth;
using Networker.Data;
using Networker.Security;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// CRUD for <b>LagHound SDK endpoints</b> — Wave 2 of the SDK feature. An "SDK
/// endpoint" is nothing more than a <see cref="Data.Entities.TestConfig"/> whose
/// workload runs the tester <c>sdkprobe</c> mode against a customer URL that
/// mounts the LagHound SDK routes (<c>docs/sdk/contract-v1.md</c>). This module
/// is a thin, purpose-built wrapper over the generic test-config create/list/
/// delete path so the frontend can register/list/remove SDK endpoints without
/// hand-assembling the polymorphic endpoint/workload JSON.
///
/// <para>The one thing this surface adds over a plain test-config is the
/// <b>LagHound token</b>: the per-endpoint shared secret sent as the
/// <c>X-LagHound-Token</c> header. It is encrypted at rest with
/// <see cref="CredentialCipher"/> — the exact AES-256-GCM scheme cloud-account
/// credentials use — into the <c>token_enc</c>/<c>token_nonce</c> columns
/// (V043). The token is <b>write-only</b>: it is NEVER serialized back to a
/// client; reads report only whether a token is set (masked with
/// <see cref="TokenMask"/>), mirroring the alert-webhook-secret masking in
/// <see cref="AlertsEndpoints"/>.</para>
///
/// <para>Routes are project-scoped under
/// <c>/api/projects/{projectId}/sdk-endpoints</c> — writes require
/// ProjectOperator, reads require ProjectMember (any role). The delete of a
/// missing/foreign endpoint is a flat 404 (never a 403 existence oracle),
/// matching the sibling report/alerting modules. 4xx bodies use the shared
/// <see cref="ApiError"/> envelope.</para>
/// </summary>
public static class SdkEndpointsEndpoints
{
    /// <summary>The tester mode these configs run — the single sdkprobe protocol.</summary>
    public const string SdkProbeMode = "sdkprobe";

    /// <summary>The endpoint_kind for an SDK endpoint (a network target URL).</summary>
    private const string EndpointKindNetwork = "network";

    /// <summary>
    /// Placeholder returned instead of the token on reads. A create/update that
    /// omits the token (or sends this mask) leaves the stored token untouched.
    /// </summary>
    public const string TokenMask = "********";

    private const int DefaultRuns = 10;
    private const int DefaultConcurrency = 1;
    private const int DefaultTimeoutMs = 30_000;
    private const int DefaultMaxDurationSecs = 900;

    public static IEndpointRouteBuilder MapSdkEndpointsEndpoints(this IEndpointRouteBuilder app)
    {
        // POST /api/projects/{projectId}/sdk-endpoints — register (operator).
        app.MapPost("/api/projects/{projectId}/sdk-endpoints", async (
            string projectId,
            [FromBody] SdkEndpointRequest req,
            HttpContext http,
            NetworkerDbContext db,
            CredentialCipher cipher,
            CancellationToken ct) =>
        {
            var user = http.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }

            if (string.IsNullOrWhiteSpace(req.Name))
            {
                return ApiError.BadRequest("name is required");
            }
            if (!TryNormalizeUrl(req.Url, out var url))
            {
                return ApiError.BadRequest("url must be an absolute http(s) URL");
            }
            if (req.Route is not null && !IsValidRoute(req.Route))
            {
                return ApiError.BadRequest("route must be an absolute path beginning with '/'");
            }
            if (string.IsNullOrWhiteSpace(req.Token))
            {
                return ApiError.BadRequest("token is required for an SDK endpoint");
            }

            var (enc, nonce) = cipher.Encrypt(System.Text.Encoding.UTF8.GetBytes(req.Token));

            var now = DateTime.UtcNow;
            var cfg = new Data.Entities.TestConfig
            {
                Id = Guid.NewGuid(),
                ProjectId = projectId,
                Name = req.Name.Trim(),
                Description = req.Description,
                EndpointKind = EndpointKindNetwork,
                EndpointRef = BuildEndpointJson(url),
                Workload = BuildWorkloadJson(req),
                MaxDurationSecs = req.MaxDurationSecs ?? DefaultMaxDurationSecs,
                TokenEnc = enc,
                TokenNonce = nonce,
                CreatedBy = user.UserId,
                CreatedAt = now,
                UpdatedAt = now,
            };

            db.TestConfigs.Add(cfg);
            try
            {
                await db.SaveChangesAsync(ct);
            }
            catch (DbUpdateException ex) when (IsUniqueViolation(ex))
            {
                return ApiError.Conflict("an SDK endpoint (or test config) with this name already exists");
            }

            return Results.Ok(ToDto(cfg));
        }).RequireAuthorization(AuthPolicies.ProjectOperator);

        // GET /api/projects/{projectId}/sdk-endpoints — list (member). Only the
        // sdkprobe configs of the project; token masked.
        app.MapGet("/api/projects/{projectId}/sdk-endpoints", async (
            string projectId,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var rows = await db.TestConfigs
                .AsNoTracking()
                .Where(c => c.ProjectId == projectId)
                .OrderByDescending(c => c.CreatedAt)
                .ToListAsync(ct);

            var dtos = rows.Where(IsSdkEndpoint).Select(ToDto).ToList();
            return Results.Ok(dtos);
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        // GET /api/projects/{projectId}/sdk-endpoints/{id} — detail (member).
        app.MapGet("/api/projects/{projectId}/sdk-endpoints/{id:guid}", async (
            string projectId,
            Guid id,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var cfg = await db.TestConfigs
                .AsNoTracking()
                .FirstOrDefaultAsync(c => c.Id == id && c.ProjectId == projectId, ct);

            return cfg is null || !IsSdkEndpoint(cfg)
                ? ApiError.NotFound("SDK endpoint not found")
                : Results.Ok(ToDto(cfg));
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        // DELETE /api/projects/{projectId}/sdk-endpoints/{id} — 204 (operator).
        // 404 (not 403) on a missing/foreign/non-sdkprobe id.
        app.MapDelete("/api/projects/{projectId}/sdk-endpoints/{id:guid}", async (
            string projectId,
            Guid id,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var cfg = await db.TestConfigs
                .FirstOrDefaultAsync(c => c.Id == id && c.ProjectId == projectId, ct);
            if (cfg is null || !IsSdkEndpoint(cfg))
            {
                return Results.NotFound();
            }

            db.TestConfigs.Remove(cfg);
            await db.SaveChangesAsync(ct);
            return Results.NoContent();
        }).RequireAuthorization(AuthPolicies.ProjectOperator);

        return app;
    }

    // ── Classification ─────────────────────────────────────────────────────────

    /// <summary>
    /// True when a config's workload runs the <c>sdkprobe</c> mode. The generic
    /// test-config surface can also create sdkprobe configs, so membership is
    /// decided by the workload modes, not by the presence of a token.
    /// </summary>
    internal static bool IsSdkEndpoint(Data.Entities.TestConfig cfg)
    {
        try
        {
            using var doc = JsonDocument.Parse(cfg.Workload);
            if (doc.RootElement.ValueKind == JsonValueKind.Object
                && doc.RootElement.TryGetProperty("modes", out var modes)
                && modes.ValueKind == JsonValueKind.Array)
            {
                foreach (var m in modes.EnumerateArray())
                {
                    if (m.ValueKind == JsonValueKind.String
                        && string.Equals(m.GetString(), SdkProbeMode, StringComparison.OrdinalIgnoreCase))
                    {
                        return true;
                    }
                }
            }
        }
        catch (JsonException)
        {
            // Malformed workload → not classifiable as an SDK endpoint.
        }
        return false;
    }

    // ── JSON builders ──────────────────────────────────────────────────────────

    private static string BuildEndpointJson(string url) =>
        JsonSerializer.Serialize(new { kind = EndpointKindNetwork, host = url });

    private static string BuildWorkloadJson(SdkEndpointRequest req)
    {
        var workload = new JsonObject
        {
            ["modes"] = new JsonArray(SdkProbeMode),
            ["runs"] = req.Runs ?? DefaultRuns,
            ["concurrency"] = req.Concurrency ?? DefaultConcurrency,
            ["timeout_ms"] = req.TimeoutMs ?? DefaultTimeoutMs,
        };
        if (req.Route is not null)
        {
            // The tester reads this as --laghound-route; the dispatcher passes
            // it through untouched (it is not a secret).
            workload["laghound_route"] = req.Route;
        }
        return workload.ToJsonString();
    }

    // ── Validation helpers ─────────────────────────────────────────────────────

    private static bool TryNormalizeUrl(string? raw, out string url)
    {
        url = "";
        if (string.IsNullOrWhiteSpace(raw)
            || !Uri.TryCreate(raw.Trim(), UriKind.Absolute, out var uri)
            || (uri.Scheme != Uri.UriSchemeHttp && uri.Scheme != Uri.UriSchemeHttps))
        {
            return false;
        }
        url = uri.ToString();
        return true;
    }

    private static bool IsValidRoute(string route) =>
        route.StartsWith('/') && !route.Contains(' ');

    private static bool IsUniqueViolation(DbUpdateException ex)
        => ex.InnerException is PostgresException { SqlState: PostgresErrorCodes.UniqueViolation };

    // ── DTO ────────────────────────────────────────────────────────────────────

    /// <summary>
    /// The redacted wire view of an SDK endpoint. The URL and route are echoed
    /// back, but the token is NEVER returned — <c>token_set</c> reports whether
    /// one is stored and <c>token</c> is always the mask.
    /// </summary>
    private static object ToDto(Data.Entities.TestConfig cfg)
    {
        var url = ReadEndpointHost(cfg.EndpointRef);
        var route = ReadWorkloadRoute(cfg.Workload);
        var tokenSet = cfg.TokenEnc is { Length: > 0 };
        return new
        {
            id = cfg.Id,
            project_id = cfg.ProjectId,
            name = cfg.Name,
            description = cfg.Description,
            mode = SdkProbeMode,
            url,
            route,
            token_set = tokenSet,
            token = tokenSet ? TokenMask : null,
            max_duration_secs = cfg.MaxDurationSecs,
            created_by = cfg.CreatedBy,
            created_at = cfg.CreatedAt,
            updated_at = cfg.UpdatedAt,
        };
    }

    private static string? ReadEndpointHost(string endpointRef)
    {
        try
        {
            using var doc = JsonDocument.Parse(endpointRef);
            return doc.RootElement.TryGetProperty("host", out var h) && h.ValueKind == JsonValueKind.String
                ? h.GetString()
                : null;
        }
        catch (JsonException)
        {
            return null;
        }
    }

    private static string? ReadWorkloadRoute(string workload)
    {
        try
        {
            using var doc = JsonDocument.Parse(workload);
            return doc.RootElement.TryGetProperty("laghound_route", out var r) && r.ValueKind == JsonValueKind.String
                ? r.GetString()
                : null;
        }
        catch (JsonException)
        {
            return null;
        }
    }

    // ── Request body (snake_case JSON) ──────────────────────────────────────────

    /// <summary>Create body for an SDK endpoint.</summary>
    public sealed record SdkEndpointRequest(
        [property: JsonPropertyName("name")] string? Name,
        [property: JsonPropertyName("description")] string? Description,
        [property: JsonPropertyName("url")] string? Url,
        [property: JsonPropertyName("token")] string? Token,
        [property: JsonPropertyName("route")] string? Route,
        [property: JsonPropertyName("runs")] int? Runs,
        [property: JsonPropertyName("concurrency")] int? Concurrency,
        [property: JsonPropertyName("timeout_ms")] int? TimeoutMs,
        [property: JsonPropertyName("max_duration_secs")] int? MaxDurationSecs);
}
