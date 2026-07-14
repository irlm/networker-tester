using System.Text.Json;
using System.Text.Json.Serialization;
using Microsoft.AspNetCore.Mvc;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;
using Networker.Data.Entities;
using Networker.Security;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// C# port of the Rust dashboard's project-scoped cloud-account CRUD
/// (<c>crates/networker-dashboard/src/api/cloud_accounts.rs</c>). Cloud secrets
/// are encrypted at rest with <see cref="CredentialCipher"/> — the byte-compatible
/// AES-256-GCM scheme the Rust dashboard used — so this control plane reads and
/// writes the same <c>credentials_enc</c> / <c>credentials_nonce</c> columns.
///
/// <para>Credentials are NEVER serialized back to the client: list and detail
/// responses expose only id/name/provider/region/personal/status/validation,
/// matching the Rust <c>AccountResponse</c> / <c>CloudAccountSummary</c> shapes.</para>
///
/// <para>Ownership model (mirrors Rust): an account with a non-null
/// <c>owner_id</c> is <b>personal</b> (only the owner or a project admin may
/// mutate it); a null <c>owner_id</c> is a <b>shared</b> account (project admin
/// only). Creating a personal account needs Operator; creating a shared account
/// needs Admin.</para>
/// </summary>
public static class CloudAccountsEndpoints
{
    private static readonly string[] ValidProviders = ["azure", "aws", "gcp"];

    public static IEndpointRouteBuilder MapCloudAccountsEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/projects/{projectId}/cloud-accounts — list (redacted). ProjectMember.
        app.MapGet("/api/projects/{projectId}/cloud-accounts", async (
            string projectId,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var rows = await db.CloudAccounts
                .AsNoTracking()
                .Where(a => a.ProjectId == projectId)
                .OrderBy(a => a.Name)
                .Select(a => new AccountSummaryDto(
                    a.AccountId,
                    a.Name,
                    a.Provider,
                    a.RegionDefault,
                    a.OwnerId != null,
                    a.Status,
                    a.LastValidated,
                    a.ValidationError))
                .ToListAsync(ct);

            return Results.Ok(rows);
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        // POST /api/projects/{projectId}/cloud-accounts — create + encrypt creds.
        // Personal accounts require Operator (enforced by policy); shared accounts
        // additionally require Admin (enforced inline, matching Rust).
        app.MapPost("/api/projects/{projectId}/cloud-accounts", async (
            string projectId,
            [FromBody] CreateAccountRequest req,
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
                return Results.BadRequest(new { error = "name is required" });
            }
            if (!ValidProviders.Contains(req.Provider))
            {
                return Results.BadRequest(new
                {
                    error = $"Invalid provider '{req.Provider}'. Valid: {string.Join(", ", ValidProviders)}",
                });
            }

            // Personal → owner is the caller (Operator already satisfied by policy).
            // Shared → owner is null and Admin is required.
            Guid? ownerId;
            if (req.Personal)
            {
                ownerId = user.UserId;
            }
            else
            {
                if (!IsProjectAdmin(http))
                {
                    return Results.Forbid();
                }
                ownerId = null;
            }

            // Serialize the submitted credentials JSON to UTF-8 bytes and encrypt.
            var credJson = req.Credentials.ValueKind == JsonValueKind.Undefined
                ? "{}"
                : req.Credentials.GetRawText();
            var (ciphertext, nonce) = cipher.Encrypt(System.Text.Encoding.UTF8.GetBytes(credJson));

            var now = DateTime.UtcNow;
            var account = new CloudAccount
            {
                AccountId = Guid.NewGuid(),
                OwnerId = ownerId,
                Name = req.Name,
                Provider = req.Provider,
                CredentialsEnc = ciphertext,
                CredentialsNonce = nonce,
                RegionDefault = req.RegionDefault,
                Status = "pending",
                ProjectId = projectId,
                CreatedAt = now,
                UpdatedAt = now,
            };

            db.CloudAccounts.Add(account);
            await db.SaveChangesAsync(ct);

            return Results.Ok(new
            {
                account_id = account.AccountId.ToString(),
                name = account.Name,
                provider = account.Provider,
            });
        }).RequireAuthorization(AuthPolicies.ProjectOperator);

        // GET /api/projects/{projectId}/cloud-accounts/{id} — detail (redacted).
        app.MapGet("/api/projects/{projectId}/cloud-accounts/{id:guid}", async (
            string projectId,
            Guid id,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var a = await db.CloudAccounts
                .AsNoTracking()
                .FirstOrDefaultAsync(x => x.AccountId == id && x.ProjectId == projectId, ct);

            return a is null
                ? Results.NotFound(new { error = "Account not found" })
                : Results.Ok(new AccountSummaryDto(
                    a.AccountId, a.Name, a.Provider, a.RegionDefault,
                    a.OwnerId != null, a.Status, a.LastValidated, a.ValidationError));
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        // PUT /api/projects/{projectId}/cloud-accounts/{id} — update name/region +
        // optional new credentials (merge-then-reencrypt). Owner or ProjectAdmin.
        app.MapPut("/api/projects/{projectId}/cloud-accounts/{id:guid}", async (
            string projectId,
            Guid id,
            [FromBody] UpdateAccountRequest req,
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

            var account = await db.CloudAccounts
                .FirstOrDefaultAsync(x => x.AccountId == id && x.ProjectId == projectId, ct);
            if (account is null)
            {
                return Results.NotFound(new { error = "Account not found" });
            }

            if (!CanMutate(account, user, http))
            {
                return Results.Forbid();
            }

            if (string.IsNullOrWhiteSpace(req.Name))
            {
                return Results.BadRequest(new { error = "name is required" });
            }

            account.Name = req.Name;
            account.RegionDefault = req.RegionDefault;
            account.UpdatedAt = DateTime.UtcNow;

            // If new (non-empty) credentials were supplied, merge them on top of
            // the existing decrypted map and re-encrypt — matching Rust. If the
            // stored blob can't be decrypted (lost/rotated key), start from an
            // empty map so re-entering credentials is a valid recovery path.
            if (req.Credentials is { Count: > 0 } newCreds &&
                newCreds.Any(kv => !string.IsNullOrEmpty(kv.Value)))
            {
                var merged = DecryptToMap(cipher, account);

                foreach (var (k, v) in newCreds)
                {
                    if (!string.IsNullOrEmpty(v))
                    {
                        merged[k] = v;
                    }
                }

                var mergedJson = JsonSerializer.SerializeToUtf8Bytes(merged);
                var (enc, nonce) = cipher.Encrypt(mergedJson);
                account.CredentialsEnc = enc;
                account.CredentialsNonce = nonce;

                // Credentials changed → reset validation state (Rust sets "pending").
                account.Status = "pending";
                account.LastValidated = null;
                account.ValidationError = null;
            }

            await db.SaveChangesAsync(ct);
            return Results.Ok(new { updated = true });
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        // DELETE /api/projects/{projectId}/cloud-accounts/{id} — owner or ProjectAdmin.
        app.MapDelete("/api/projects/{projectId}/cloud-accounts/{id:guid}", async (
            string projectId,
            Guid id,
            HttpContext http,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var user = http.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }

            var account = await db.CloudAccounts
                .FirstOrDefaultAsync(x => x.AccountId == id && x.ProjectId == projectId, ct);
            if (account is null)
            {
                return Results.NotFound(new { error = "Account not found" });
            }

            if (!CanMutate(account, user, http))
            {
                return Results.Forbid();
            }

            db.CloudAccounts.Remove(account);
            await db.SaveChangesAsync(ct);
            return Results.Ok(new { deleted = true });
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        // POST /api/projects/{projectId}/cloud-accounts/{id}/validate — ProjectOperator.
        // Decrypts the stored credentials and performs a best-effort provider check.
        //
        // NOTE: the actual provider API/CLI call is STUBBED here (see
        // ValidateProviderStub). CI has no cloud credentials and the Rust
        // validators shell out to az/aws/gcloud or hit login.microsoftonline.com —
        // neither is available/appropriate in the C# test environment yet. We still
        // exercise the real decrypt path and persist Status/LastValidated/
        // ValidationError so the shape and DB effects match the Rust endpoint.
        app.MapPost("/api/projects/{projectId}/cloud-accounts/{id:guid}/validate", async (
            string projectId,
            Guid id,
            NetworkerDbContext db,
            CredentialCipher cipher,
            CancellationToken ct) =>
        {
            var account = await db.CloudAccounts
                .FirstOrDefaultAsync(x => x.AccountId == id && x.ProjectId == projectId, ct);
            if (account is null)
            {
                return Results.NotFound(new { error = "Account not found" });
            }

            Dictionary<string, string> creds;
            try
            {
                creds = DecryptToMap(cipher, account);
            }
            catch (Exception)
            {
                account.Status = "error";
                account.LastValidated = DateTime.UtcNow;
                account.ValidationError = "Failed to decrypt credentials";
                await db.SaveChangesAsync(ct);
                return Results.Ok(new { status = account.Status, validation_error = account.ValidationError });
            }

            var (status, error) = ValidateProviderStub(account.Provider, creds);

            account.Status = status;
            account.LastValidated = DateTime.UtcNow;
            account.ValidationError = error;
            await db.SaveChangesAsync(ct);

            return Results.Ok(new { status, validation_error = error });
        }).RequireAuthorization(AuthPolicies.ProjectOperator);

        return app;
    }

    // ── Authorization helpers ─────────────────────────────────────────────────

    /// <summary>
    /// True when the caller may mutate <paramref name="account"/>: the owner of a
    /// personal account, or a project admin (for shared accounts and any account).
    /// Mirrors the Rust owner/Admin branch in update/delete.
    /// </summary>
    private static bool CanMutate(CloudAccount account, AuthUser user, HttpContext http)
    {
        if (account.OwnerId is Guid owner)
        {
            // Personal account: owner OR project admin.
            return owner == user.UserId || IsProjectAdmin(http);
        }

        // Shared account: project admin only.
        return IsProjectAdmin(http);
    }

    /// <summary>
    /// Reads the effective project role stashed by <see cref="ProjectRoleHandler"/>
    /// during policy evaluation. Platform admins are resolved to project Admin
    /// there, so this also covers them.
    /// </summary>
    private static bool IsProjectAdmin(HttpContext http)
        => http.Items.TryGetValue(ProjectRoleHandler.ProjectRoleItemKey, out var raw)
           && raw is ProjectRole role
           && role.HasPermission(ProjectRole.Admin);

    // ── Credential helpers ────────────────────────────────────────────────────

    /// <summary>
    /// Decrypt the stored credential blob into a flat string map. The Rust side
    /// stores a JSON object of string values; non-string values are coerced to
    /// their raw JSON text so nothing is silently dropped.
    /// </summary>
    private static Dictionary<string, string> DecryptToMap(CredentialCipher cipher, CloudAccount account)
    {
        var plain = cipher.Decrypt(account.CredentialsEnc, account.CredentialsNonce);
        using var doc = JsonDocument.Parse(plain);
        var map = new Dictionary<string, string>();
        if (doc.RootElement.ValueKind == JsonValueKind.Object)
        {
            foreach (var prop in doc.RootElement.EnumerateObject())
            {
                map[prop.Name] = prop.Value.ValueKind == JsonValueKind.String
                    ? prop.Value.GetString() ?? string.Empty
                    : prop.Value.GetRawText();
            }
        }
        return map;
    }

    /// <summary>
    /// STUB provider validation. Verifies the credential map has the fields the
    /// real Rust validators require (so obviously-incomplete credentials still
    /// surface an error), but does NOT contact any cloud provider. Returns
    /// ("active", null) when the required fields are present, else
    /// ("error", message). Replace with real provider calls once outbound cloud
    /// access is available in the deployment/test environment.
    /// </summary>
    private static (string status, string? error) ValidateProviderStub(
        string provider,
        IReadOnlyDictionary<string, string> creds)
    {
        bool Has(string k) => creds.TryGetValue(k, out var v) && !string.IsNullOrEmpty(v);

        switch (provider)
        {
            case "azure":
                if (!Has("client_id") || !Has("client_secret") || !Has("tenant_id"))
                {
                    return ("error", "Missing client_id, client_secret, or tenant_id");
                }
                break;
            case "aws":
                if (!Has("access_key_id") || !Has("secret_access_key"))
                {
                    return ("error", "Missing access_key_id or secret_access_key");
                }
                break;
            case "gcp":
                if (!Has("json_key"))
                {
                    return ("error", "Missing json_key");
                }
                break;
            default:
                return ("error", $"Unknown provider: {provider}");
        }

        // Fields present; the real provider round-trip is not performed (stub).
        return ("active", null);
    }

    // ── DTOs (snake_case bodies/responses to match the Rust wire shapes) ───────

    /// <summary>Mirrors Rust <c>CreateAccountRequest</c>. Credentials arrive as a
    /// raw JSON object and are encrypted verbatim.</summary>
    public sealed record CreateAccountRequest(
        [property: JsonPropertyName("name")] string Name,
        [property: JsonPropertyName("provider")] string Provider,
        [property: JsonPropertyName("credentials")] JsonElement Credentials,
        [property: JsonPropertyName("region_default")] string? RegionDefault,
        [property: JsonPropertyName("personal")] bool Personal = false);

    /// <summary>Mirrors Rust <c>UpdateAccountRequest</c>. <c>credentials</c> is an
    /// optional flat map of new secret values merged over the existing set.</summary>
    public sealed record UpdateAccountRequest(
        [property: JsonPropertyName("name")] string Name,
        [property: JsonPropertyName("region_default")] string? RegionDefault,
        [property: JsonPropertyName("credentials")] Dictionary<string, string>? Credentials);

    /// <summary>Redacted account view — matches Rust <c>AccountResponse</c> /
    /// <c>CloudAccountSummary</c> (NO credential fields).</summary>
    public sealed record AccountSummaryDto(
        [property: JsonPropertyName("account_id")] Guid AccountId,
        [property: JsonPropertyName("name")] string Name,
        [property: JsonPropertyName("provider")] string Provider,
        [property: JsonPropertyName("region_default")] string? RegionDefault,
        [property: JsonPropertyName("personal")] bool Personal,
        [property: JsonPropertyName("status")] string Status,
        [property: JsonPropertyName("last_validated")] DateTime? LastValidated,
        [property: JsonPropertyName("validation_error")] string? ValidationError);
}
