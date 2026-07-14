using System.Text.Json;
using System.Text.Json.Serialization;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Sso;
using Networker.Data;
using Networker.Data.Entities;
using Networker.Security;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// SSO provider admin CRUD — port of
/// <c>crates/networker-dashboard/src/api/sso_admin.rs</c>, mounted at
/// <c>/api/sso-providers</c>. Requires the GlobalAdmin policy AND
/// <c>is_platform_admin</c> (Rust's <c>extract_admin</c> gates on the platform
/// flag specifically, not the workspace role).
///
/// <para><b>Secrets:</b> the <c>sso_provider</c> table stores
/// <c>client_secret_enc</c> + <c>client_secret_nonce</c> (bytea) — the same
/// AES-256-GCM scheme as cloud accounts — so create/update encrypt via
/// <see cref="CredentialCipher"/> and responses NEVER carry secret material
/// (only <c>has_client_secret</c>).</para>
///
/// <para><b>Delete:</b> the Rust implementation HARD-deletes the row
/// (<c>DELETE FROM sso_provider</c> — there is no deleted_at column), so this
/// port does too.</para>
/// </summary>
public static class SsoAdminEndpoints
{
    public static IEndpointRouteBuilder MapSsoAdminEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/sso-providers — list all (enabled + disabled), redacted.
        app.MapGet("/api/sso-providers", async (
            HttpContext http,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            if (RequirePlatformAdmin(http) is { } denied)
            {
                return denied;
            }

            var rows = await db.SsoProviders
                .AsNoTracking()
                .OrderBy(p => p.DisplayOrder)
                .ThenBy(p => p.CreatedAt)
                .ToListAsync(ct);

            return Results.Ok(rows.Select(ToResponse).ToList());
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        // POST /api/sso-providers — create (encrypts client_secret). 201.
        app.MapPost("/api/sso-providers", async (
            CreateProviderRequest req,
            HttpContext http,
            NetworkerDbContext db,
            CredentialCipher cipher,
            CancellationToken ct) =>
        {
            if (RequirePlatformAdmin(http) is { } denied)
            {
                return denied;
            }

            var user = http.GetAuthUser()!;

            if (string.IsNullOrWhiteSpace(req.Name))
            {
                return BadRequestText("name must be non-empty");
            }

            if (string.IsNullOrWhiteSpace(req.ClientId))
            {
                return BadRequestText("client_id must be non-empty");
            }

            if (string.IsNullOrEmpty(req.ClientSecret))
            {
                return BadRequestText("client_secret must be non-empty");
            }

            if (SsoProviderValidation.Validate(req.ProviderType, req.IssuerUrl, req.TenantId) is { } error)
            {
                return BadRequestText(error);
            }

            var (ciphertext, nonce) = cipher.Encrypt(System.Text.Encoding.UTF8.GetBytes(req.ClientSecret));

            var now = DateTime.UtcNow;
            var row = new SsoProvider
            {
                ProviderId = Guid.NewGuid(),
                Name = req.Name,
                ProviderType = req.ProviderType,
                ClientId = req.ClientId,
                ClientSecretEnc = ciphertext,
                ClientSecretNonce = nonce,
                IssuerUrl = req.IssuerUrl,
                TenantId = req.TenantId,
                ExtraConfig = req.ExtraConfig is { ValueKind: JsonValueKind.Object } cfg
                    ? cfg.GetRawText()
                    : "{}",
                Enabled = req.Enabled ?? true,
                DisplayOrder = req.DisplayOrder ?? 0,
                CreatedBy = user.UserId,
                CreatedAt = now,
                UpdatedAt = now,
            };

            db.SsoProviders.Add(row);
            await db.SaveChangesAsync(ct);

            return Results.Json(ToResponse(row), statusCode: StatusCodes.Status201Created);
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        // PUT /api/sso-providers/{id} — partial update. The body is parsed as a
        // JSON document so "field absent" (leave unchanged) and "field: null"
        // (set NULL, for issuer_url/tenant_id) stay distinguishable — the Rust
        // UpdateBody models this as Option<Option<String>>.
        app.MapPut("/api/sso-providers/{id:guid}", async (
            Guid id,
            HttpContext http,
            NetworkerDbContext db,
            CredentialCipher cipher,
            CancellationToken ct) =>
        {
            if (RequirePlatformAdmin(http) is { } denied)
            {
                return denied;
            }

            JsonDocument body;
            try
            {
                body = await JsonDocument.ParseAsync(http.Request.Body, cancellationToken: ct);
            }
            catch (JsonException ex)
            {
                return BadRequestText(ex.Message);
            }

            using (body)
            {
                if (body.RootElement.ValueKind != JsonValueKind.Object)
                {
                    return BadRequestText("Invalid request body");
                }

                var row = await db.SsoProviders.FirstOrDefaultAsync(p => p.ProviderId == id, ct);
                if (row is null)
                {
                    return Results.Text("Provider not found", statusCode: StatusCodes.Status404NotFound);
                }

                var root = body.RootElement;

                // Optional non-null scalars.
                var name = GetOptionalString(root, "name", out var namePresent);
                if (namePresent && string.IsNullOrWhiteSpace(name))
                {
                    return BadRequestText("name must be non-empty");
                }

                var clientId = GetOptionalString(root, "client_id", out var clientIdPresent);
                if (clientIdPresent && string.IsNullOrWhiteSpace(clientId))
                {
                    return BadRequestText("client_id must be non-empty");
                }

                var providerType = GetOptionalString(root, "provider_type", out var typePresent);

                // Nullable strings with absent-vs-null semantics.
                var issuerUrl = GetOptionalString(root, "issuer_url", out var issuerPresent);
                var tenantId = GetOptionalString(root, "tenant_id", out var tenantPresent);

                // Cross-field validation against the merged (effective) config.
                var effType = typePresent ? providerType! : row.ProviderType;
                var effIssuer = issuerPresent ? issuerUrl : row.IssuerUrl;
                var effTenant = tenantPresent ? tenantId : row.TenantId;
                if (SsoProviderValidation.Validate(effType, effIssuer, effTenant) is { } error)
                {
                    return BadRequestText(error);
                }

                // New secret (optional; absent = keep the existing ciphertext).
                var clientSecret = GetOptionalString(root, "client_secret", out var secretPresent);
                if (secretPresent)
                {
                    if (string.IsNullOrEmpty(clientSecret))
                    {
                        return BadRequestText("client_secret must be non-empty");
                    }

                    var (ciphertext, nonce) =
                        cipher.Encrypt(System.Text.Encoding.UTF8.GetBytes(clientSecret));
                    row.ClientSecretEnc = ciphertext;
                    row.ClientSecretNonce = nonce;
                }

                if (namePresent)
                {
                    row.Name = name!;
                }

                if (typePresent)
                {
                    row.ProviderType = providerType!;
                }

                if (clientIdPresent)
                {
                    row.ClientId = clientId!;
                }

                if (issuerPresent)
                {
                    row.IssuerUrl = issuerUrl; // may be an explicit null
                }

                if (tenantPresent)
                {
                    row.TenantId = tenantId; // may be an explicit null
                }

                if (root.TryGetProperty("extra_config", out var extra) &&
                    extra.ValueKind == JsonValueKind.Object)
                {
                    row.ExtraConfig = extra.GetRawText();
                }

                if (root.TryGetProperty("enabled", out var enabled) &&
                    enabled.ValueKind is JsonValueKind.True or JsonValueKind.False)
                {
                    row.Enabled = enabled.GetBoolean();
                }

                if (root.TryGetProperty("display_order", out var order) &&
                    order.ValueKind == JsonValueKind.Number)
                {
                    row.DisplayOrder = order.GetInt16();
                }

                row.UpdatedAt = DateTime.UtcNow;
                await db.SaveChangesAsync(ct);

                return Results.Ok(ToResponse(row));
            }
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        // DELETE /api/sso-providers/{id} — hard delete, matching Rust.
        app.MapDelete("/api/sso-providers/{id:guid}", async (
            Guid id,
            HttpContext http,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            if (RequirePlatformAdmin(http) is { } denied)
            {
                return denied;
            }

            var row = await db.SsoProviders.FirstOrDefaultAsync(p => p.ProviderId == id, ct);
            if (row is null)
            {
                return Results.Text("Provider not found", statusCode: StatusCodes.Status404NotFound);
            }

            db.SsoProviders.Remove(row);
            await db.SaveChangesAsync(ct);

            return Results.Ok(new { deleted = true });
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        return app;
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// <summary>
    /// Rust <c>extract_admin</c>: 401 without a user, 403 without the platform
    /// flag. The GlobalAdmin policy already ran; this adds the stricter
    /// is_platform_admin gate (DB-fresh via UserStatusMiddleware).
    /// </summary>
    private static IResult? RequirePlatformAdmin(HttpContext http)
    {
        var user = http.GetAuthUser();
        if (user is null)
        {
            return Results.Unauthorized();
        }

        return user.IsPlatformAdmin
            ? null
            : Results.Text("Platform admin required", statusCode: StatusCodes.Status403Forbidden);
    }

    private static string? GetOptionalString(JsonElement root, string name, out bool present)
    {
        present = root.TryGetProperty(name, out var value);
        if (!present)
        {
            return null;
        }

        return value.ValueKind == JsonValueKind.String ? value.GetString() : null;
    }

    private static IResult BadRequestText(string message)
        => Results.Text(message, statusCode: StatusCodes.Status400BadRequest);

    private static ProviderResponse ToResponse(SsoProvider row)
    {
        JsonElement extraConfig;
        try
        {
            using var doc = JsonDocument.Parse(
                string.IsNullOrWhiteSpace(row.ExtraConfig) ? "{}" : row.ExtraConfig);
            extraConfig = doc.RootElement.Clone();
        }
        catch (JsonException)
        {
            using var doc = JsonDocument.Parse("{}");
            extraConfig = doc.RootElement.Clone();
        }

        return new ProviderResponse(
            row.ProviderId,
            row.Name,
            row.ProviderType,
            row.ClientId,
            row.ClientSecretEnc is { Length: > 0 },
            row.IssuerUrl,
            row.TenantId,
            extraConfig,
            row.Enabled,
            row.DisplayOrder);
    }

    // ── DTOs (snake_case wire shapes, matching sso_admin.rs) ─────────────────

    /// <summary>Mirrors Rust <c>CreateBody</c>.</summary>
    public sealed record CreateProviderRequest(
        [property: JsonPropertyName("name")] string Name,
        [property: JsonPropertyName("provider_type")] string ProviderType,
        [property: JsonPropertyName("client_id")] string ClientId,
        [property: JsonPropertyName("client_secret")] string ClientSecret,
        [property: JsonPropertyName("issuer_url")] string? IssuerUrl,
        [property: JsonPropertyName("tenant_id")] string? TenantId,
        [property: JsonPropertyName("extra_config")] JsonElement? ExtraConfig,
        [property: JsonPropertyName("enabled")] bool? Enabled,
        [property: JsonPropertyName("display_order")] short? DisplayOrder);

    /// <summary>Mirrors Rust <c>SsoProviderResponse</c> — NO secret material,
    /// only <c>has_client_secret</c>.</summary>
    public sealed record ProviderResponse(
        [property: JsonPropertyName("provider_id")] Guid ProviderId,
        [property: JsonPropertyName("name")] string Name,
        [property: JsonPropertyName("provider_type")] string ProviderType,
        [property: JsonPropertyName("client_id")] string ClientId,
        [property: JsonPropertyName("has_client_secret")] bool HasClientSecret,
        [property: JsonPropertyName("issuer_url")] string? IssuerUrl,
        [property: JsonPropertyName("tenant_id")] string? TenantId,
        [property: JsonPropertyName("extra_config")] JsonElement ExtraConfig,
        [property: JsonPropertyName("enabled")] bool Enabled,
        [property: JsonPropertyName("display_order")] short DisplayOrder);
}
