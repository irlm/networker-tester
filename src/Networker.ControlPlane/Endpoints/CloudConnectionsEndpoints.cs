using System.Text.Json;
using System.Text.Json.Serialization;
using Microsoft.AspNetCore.Mvc;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// C# port of the Rust dashboard's project-scoped cloud-connection CRUD
/// (<c>crates/networker-dashboard/src/api/cloud_connections.rs</c>). A
/// <c>cloud_connection</c> is a provider-level connection whose <c>config</c> is
/// stored as <b>plaintext JSON</b> (NOT encrypted — unlike a cloud-account's
/// credentials). All operations require ProjectAdmin, matching Rust
/// (<c>require_role(Admin)</c> / <c>require_project_role(Admin)</c>).
///
/// <para>Because the config may hold provider identifiers (and, depending on the
/// provider, sensitive values), the <b>list</b> response omits it entirely — the
/// same caution the Rust list handler applies. The single-item GET returns the
/// full row (config included) to admins, matching Rust's
/// <c>CloudConnectionRow</c> get.</para>
/// </summary>
public static class CloudConnectionsEndpoints
{
    private static readonly string[] ValidProviders = ["azure", "aws", "gcp"];

    public static IEndpointRouteBuilder MapCloudConnectionsEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/projects/{projectId}/cloud-connections — list WITHOUT config.
        app.MapGet("/api/projects/{projectId}/cloud-connections", async (
            string projectId,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var rows = await db.CloudConnections
                .AsNoTracking()
                .Where(c => c.ProjectId == projectId)
                .OrderBy(c => c.Name)
                .Select(c => new ConnectionSummaryDto(
                    c.ConnectionId,
                    c.Name,
                    c.Provider,
                    c.Status,
                    c.LastValidated,
                    c.ValidationError,
                    c.CreatedBy,
                    c.CreatedAt,
                    c.UpdatedAt))
                .ToListAsync(ct);

            return Results.Ok(rows);
        }).RequireAuthorization(AuthPolicies.ProjectAdmin);

        // POST /api/projects/{projectId}/cloud-connections — create (config plaintext).
        app.MapPost("/api/projects/{projectId}/cloud-connections", async (
            string projectId,
            [FromBody] CreateConnectionRequest req,
            HttpContext http,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var user = http.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }

            if (string.IsNullOrWhiteSpace(req.Name))
            {
                return Results.BadRequest(new { error = "name is required" });
            }
            if (!ValidProviders.Contains(req.Provider))
            {
                return Results.BadRequest(new
                {
                    error = $"Invalid provider '{req.Provider}'. Valid: {string.Join(", ", ValidProviders)}",
                });
            }

            var now = DateTime.UtcNow;
            var conn = new CloudConnection
            {
                ConnectionId = Guid.NewGuid(),
                Name = req.Name,
                Provider = req.Provider,
                Config = RawConfig(req.Config),
                Status = "pending",
                CreatedBy = user.UserId,
                ProjectId = projectId,
                CreatedAt = now,
                UpdatedAt = now,
            };

            db.CloudConnections.Add(conn);
            await db.SaveChangesAsync(ct);

            return Results.Ok(new { connection_id = conn.ConnectionId.ToString() });
        }).RequireAuthorization(AuthPolicies.ProjectAdmin);

        // GET /api/projects/{projectId}/cloud-connections/{id} — full row (admin).
        app.MapGet("/api/projects/{projectId}/cloud-connections/{id:guid}", async (
            string projectId,
            Guid id,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var c = await db.CloudConnections
                .AsNoTracking()
                .FirstOrDefaultAsync(x => x.ConnectionId == id && x.ProjectId == projectId, ct);

            return c is null
                ? Results.NotFound(new { error = "Connection not found" })
                : Results.Ok(ToFullDto(c));
        }).RequireAuthorization(AuthPolicies.ProjectAdmin);

        // PUT /api/projects/{projectId}/cloud-connections/{id} — update name+config.
        app.MapPut("/api/projects/{projectId}/cloud-connections/{id:guid}", async (
            string projectId,
            Guid id,
            [FromBody] UpdateConnectionRequest req,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var conn = await db.CloudConnections
                .FirstOrDefaultAsync(x => x.ConnectionId == id && x.ProjectId == projectId, ct);
            if (conn is null)
            {
                return Results.NotFound(new { error = "Connection not found" });
            }

            if (string.IsNullOrWhiteSpace(req.Name))
            {
                return Results.BadRequest(new { error = "name is required" });
            }

            conn.Name = req.Name;
            conn.Config = RawConfig(req.Config);
            conn.UpdatedAt = DateTime.UtcNow;

            await db.SaveChangesAsync(ct);
            return Results.Ok(new { updated = true });
        }).RequireAuthorization(AuthPolicies.ProjectAdmin);

        // DELETE /api/projects/{projectId}/cloud-connections/{id}.
        app.MapDelete("/api/projects/{projectId}/cloud-connections/{id:guid}", async (
            string projectId,
            Guid id,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var affected = await db.CloudConnections
                .Where(x => x.ConnectionId == id && x.ProjectId == projectId)
                .ExecuteDeleteAsync(ct);

            return affected > 0
                ? Results.Ok(new { deleted = true })
                : Results.NotFound(new { error = "Connection not found" });
        }).RequireAuthorization(AuthPolicies.ProjectAdmin);

        // POST /api/projects/{projectId}/cloud-connections/{id}/validate.
        // STUBBED provider check (same rationale as the cloud-accounts validate):
        // CI has no cloud access. We verify the config carries the provider's
        // required identifier and persist Status/LastValidated/ValidationError,
        // matching the Rust endpoint's shape and DB effects.
        app.MapPost("/api/projects/{projectId}/cloud-connections/{id:guid}/validate", async (
            string projectId,
            Guid id,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var conn = await db.CloudConnections
                .FirstOrDefaultAsync(x => x.ConnectionId == id && x.ProjectId == projectId, ct);
            if (conn is null)
            {
                return Results.NotFound(new { error = "Connection not found" });
            }

            var (status, error) = ValidateConfigStub(conn.Provider, conn.Config);
            conn.Status = status;
            conn.LastValidated = DateTime.UtcNow;
            conn.ValidationError = error;
            await db.SaveChangesAsync(ct);

            return Results.Ok(new { status, validation_error = error });
        }).RequireAuthorization(AuthPolicies.ProjectAdmin);

        return app;
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// <summary>Store the config as its raw JSON text (Rust persists the JSON
    /// value verbatim). An undefined/absent body becomes an empty object.</summary>
    private static string RawConfig(JsonElement config)
        => config.ValueKind == JsonValueKind.Undefined ? "{}" : config.GetRawText();

    private static object? ParseConfig(string config)
    {
        try
        {
            using var doc = JsonDocument.Parse(config);
            return doc.RootElement.Clone();
        }
        catch (JsonException)
        {
            return config;
        }
    }

    private static object ToFullDto(CloudConnection c) => new
    {
        connection_id = c.ConnectionId,
        name = c.Name,
        provider = c.Provider,
        config = ParseConfig(c.Config),
        status = c.Status,
        last_validated = c.LastValidated,
        validation_error = c.ValidationError,
        created_by = c.CreatedBy,
        created_at = c.CreatedAt,
        updated_at = c.UpdatedAt,
        project_id = c.ProjectId,
    };

    /// <summary>
    /// STUB config validation — mirrors the required-field checks the Rust
    /// validators run before shelling out to az/aws/gcloud, without performing
    /// any provider round-trip.
    /// </summary>
    private static (string status, string? error) ValidateConfigStub(string provider, string config)
    {
        JsonElement root;
        try
        {
            using var doc = JsonDocument.Parse(config);
            root = doc.RootElement.Clone();
        }
        catch (JsonException)
        {
            return ("error", "Config is not valid JSON");
        }

        bool Has(string k) =>
            root.ValueKind == JsonValueKind.Object &&
            root.TryGetProperty(k, out var v) &&
            v.ValueKind == JsonValueKind.String &&
            !string.IsNullOrEmpty(v.GetString());

        switch (provider)
        {
            case "azure":
                if (!Has("subscription_id"))
                {
                    return ("error", "Missing subscription_id in config");
                }
                break;
            case "aws":
                if (!Has("role_arn"))
                {
                    return ("error", "Missing role_arn in config. Create an IAM role that trusts Azure AD, then provide its ARN.");
                }
                break;
            case "gcp":
                if (!Has("project_id"))
                {
                    return ("error", "Missing project_id in config");
                }
                break;
            default:
                return ("error", "Unknown provider");
        }

        return ("active", null);
    }

    // ── DTOs ──────────────────────────────────────────────────────────────────

    /// <summary>Mirrors Rust <c>CreateRequest</c>.</summary>
    public sealed record CreateConnectionRequest(
        [property: JsonPropertyName("name")] string Name,
        [property: JsonPropertyName("provider")] string Provider,
        [property: JsonPropertyName("config")] JsonElement Config);

    /// <summary>Mirrors Rust <c>UpdateRequest</c>.</summary>
    public sealed record UpdateConnectionRequest(
        [property: JsonPropertyName("name")] string Name,
        [property: JsonPropertyName("config")] JsonElement Config);

    /// <summary>List item — deliberately OMITS <c>config</c> to avoid leaking
    /// connection secrets in bulk listings.</summary>
    public sealed record ConnectionSummaryDto(
        [property: JsonPropertyName("connection_id")] Guid ConnectionId,
        [property: JsonPropertyName("name")] string Name,
        [property: JsonPropertyName("provider")] string Provider,
        [property: JsonPropertyName("status")] string Status,
        [property: JsonPropertyName("last_validated")] DateTime? LastValidated,
        [property: JsonPropertyName("validation_error")] string? ValidationError,
        [property: JsonPropertyName("created_by")] Guid? CreatedBy,
        [property: JsonPropertyName("created_at")] DateTime CreatedAt,
        [property: JsonPropertyName("updated_at")] DateTime UpdatedAt);
}
