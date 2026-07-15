using System.Text.Json;
using System.Text.Json.Serialization;
using Microsoft.AspNetCore.Mvc;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;
using Networker.Data.Entities;
using Networker.Security;

namespace Networker.ControlPlane.Sso;

/// <summary>
/// The public (anonymous) SSO login flow — port of the SSO half of
/// <c>crates/networker-dashboard/src/api/auth.rs</c>:
///
/// <list type="bullet">
///   <item><c>GET /auth/sso/providers</c> — enabled providers (id/name/type only,
///         never secrets) for the login page buttons.</item>
///   <item><c>GET /auth/sso/init?provider={id}</c> — 307 to the provider's
///         authorize endpoint with a CSRF state cookie.</item>
///   <item><c>GET /api/auth/sso/callback?code=&amp;state=</c> — verify state, swap the
///         code for tokens, validate the id_token claims, find-or-create the
///         dash_user, then 307 to <c>{public_url}/sso-complete?code=</c> with a
///         single-use exchange code (the JWT never rides in a URL).</item>
///   <item><c>POST /auth/sso/exchange</c> — swap the exchange code for the
///         session JWT; response shape matches <c>POST /auth/login</c>.</item>
/// </list>
///
/// All provider round-trips go through <see cref="IOidcProviderClient"/> so the
/// flow logic stays testable offline; error paths redirect to
/// <c>{public_url}/login?error={label}</c> with the same labels Rust emits.
/// </summary>
public static class SsoFlowEndpoints
{
    /// <summary>Same env var + default the Rust config resolves for redirects.</summary>
    public const string PublicUrlEnvVar = "DASHBOARD_PUBLIC_URL";

    public static string ResolvePublicUrl()
        => Environment.GetEnvironmentVariable(PublicUrlEnvVar) is { Length: > 0 } url
            ? url.TrimEnd('/')
            : "http://localhost:3000";

    public static IEndpointRouteBuilder MapSsoEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /auth/sso/providers — public; enabled providers, no secrets.
        app.MapGet("/api/auth/sso/providers", async (NetworkerDbContext db, CancellationToken ct) =>
        {
            var providers = await db.SsoProviders
                .AsNoTracking()
                .Where(p => p.Enabled)
                .OrderBy(p => p.DisplayOrder)
                .ThenBy(p => p.CreatedAt)
                .Select(p => new PublicProviderDto(p.ProviderId.ToString(), p.Name, p.ProviderType))
                .ToListAsync(ct);

            return Results.Ok(new { providers });
        }).AllowAnonymous();

        // GET /auth/sso/init?provider={uuid} — 307 to the provider authorize URL.
        app.MapGet("/api/auth/sso/init", async (
            [FromQuery(Name = "provider")] string provider,
            HttpContext http,
            NetworkerDbContext db,
            OidcFlowService flow,
            CancellationToken ct) =>
        {
            if (!Guid.TryParse(provider, out var providerId))
            {
                return Results.Text("Invalid provider ID", statusCode: StatusCodes.Status400BadRequest);
            }

            var row = await db.SsoProviders
                .AsNoTracking()
                .FirstOrDefaultAsync(p => p.ProviderId == providerId && p.Enabled, ct);
            if (row is null)
            {
                return Results.Text("SSO provider not found", statusCode: StatusCodes.Status404NotFound);
            }

            var publicUrl = ResolvePublicUrl();
            var info = ToInfo(row);

            string redirectUrl;
            string state;
            try
            {
                (redirectUrl, state) = await flow.BuildAuthorizeRedirectAsync(info, publicUrl, ct);
            }
            catch (OidcConfigException ex)
            {
                return Results.Text(ex.Message, statusCode: StatusCodes.Status400BadRequest);
            }
            catch (Exception)
            {
                // Discovery round-trip failed (network / malformed document) —
                // Rust answers 502 "OIDC discovery failed".
                return Results.Text("OIDC discovery failed", statusCode: StatusCodes.Status502BadGateway);
            }

            http.Response.Headers.Append("Set-Cookie", OidcFlowService.BuildStateCookie(state, publicUrl));
            return Results.Redirect(redirectUrl, permanent: false, preserveMethod: true); // 307, like Rust
        }).AllowAnonymous();

        // GET /api/auth/sso/callback?code=&state=[&error=] — the provider redirect.
        app.MapGet("/api/auth/sso/callback", async (
            HttpContext http,
            NetworkerDbContext db,
            OidcFlowService flow,
            CredentialCipher cipher,
            SsoExchangeCodeCache codes,
            CancellationToken ct) =>
        {
            var publicUrl = ResolvePublicUrl();
            var query = http.Request.Query;

            if (query.TryGetValue("error", out var providerError) && providerError.Count > 0)
            {
                return LoginError(publicUrl, "provider_error");
            }

            var code = query["code"].ToString();
            if (code.Length == 0)
            {
                return LoginError(publicUrl, "missing_code");
            }

            var callbackState = query["state"].ToString();
            if (callbackState.Length == 0)
            {
                return LoginError(publicUrl, "missing_state");
            }

            // CSRF: state echoed by the provider must equal the cookie we set.
            var cookieHeader = http.Request.Headers.Cookie.ToString();
            if (!OidcFlowService.StateMatchesCookie(cookieHeader, callbackState))
            {
                return LoginError(publicUrl, "state_mismatch");
            }

            var providerId = OidcFlowService.ParseProviderIdFromState(callbackState);
            if (providerId is null)
            {
                return LoginError(publicUrl, "invalid_state");
            }

            var row = await db.SsoProviders
                .AsNoTracking()
                .FirstOrDefaultAsync(p => p.ProviderId == providerId, ct);
            if (row is null)
            {
                return LoginError(publicUrl, "unknown_provider");
            }

            // Decrypt the client secret (same AES-256-GCM columns Rust wrote).
            string clientSecret;
            try
            {
                clientSecret = System.Text.Encoding.UTF8.GetString(
                    cipher.Decrypt(row.ClientSecretEnc, row.ClientSecretNonce));
            }
            catch (Exception)
            {
                return LoginError(publicUrl, "internal_error");
            }

            var info = ToInfo(row);

            string tokenUrl;
            try
            {
                tokenUrl = await flow.ResolveTokenEndpointAsync(info, ct);
            }
            catch (Exception)
            {
                return LoginError(publicUrl, "internal_error");
            }

            // Exchange the authorization code for tokens at the provider.
            string? idToken;
            try
            {
                idToken = await flow.ExchangeAuthorizationCodeAsync(
                    tokenUrl,
                    code,
                    OidcFlowService.BuildCallbackRedirectUri(publicUrl),
                    info.ClientId,
                    clientSecret,
                    ct);
            }
            catch (JsonException)
            {
                return LoginError(publicUrl, "token_parse_failed");
            }
            catch (Exception)
            {
                return LoginError(publicUrl, "token_exchange_failed");
            }

            if (idToken is null)
            {
                return LoginError(publicUrl, "no_id_token");
            }

            using var claimsDoc = OidcFlowService.DecodeJwtPayload(idToken);
            if (claimsDoc is null)
            {
                return LoginError(publicUrl, "id_token_decode_failed");
            }

            var claims = claimsDoc.RootElement;
            if (OidcFlowService.ValidateIdTokenClaims(info.ProviderType, info.ClientId, claims) is not null)
            {
                return LoginError(publicUrl, "id_token_invalid");
            }

            var identity = OidcFlowService.ExtractIdentity(claims);
            if (identity is null)
            {
                return LoginError(publicUrl, "missing_claims");
            }

            var (email, subjectId, displayName) = identity.Value;

            // Find-or-create the dash_user (Rust: find_by_email is LOWER()-matched).
            var existing = await db.DashUsers
                .FirstOrDefaultAsync(u => u.Email != null && u.Email.ToLower() == email, ct);

            Guid userId;
            string role;
            string userStatus;

            if (existing is not null)
            {
                if (existing.AuthProvider == "local")
                {
                    // Rust "Fix 2": never auto-link admin accounts to SSO.
                    if (existing.Role == "admin")
                    {
                        return LoginError(publicUrl, "admin_link_blocked");
                    }

                    // Auto-link the local account to this SSO identity.
                    existing.AuthProvider = info.ProviderType;
                    existing.SsoSubjectId = subjectId;
                    existing.DisplayName = displayName ?? existing.DisplayName;
                    existing.LastLoginAt = DateTime.UtcNow;
                }
                else
                {
                    // Existing SSO user — record the login.
                    existing.LastLoginAt = DateTime.UtcNow;
                }

                await db.SaveChangesAsync(ct);
                (userId, role, userStatus) = (existing.UserId, existing.Role, existing.Status);
            }
            else
            {
                // New SSO user: viewer + pending (requires admin approval), sso_only.
                var user = new DashUser
                {
                    UserId = Guid.NewGuid(),
                    Email = email,
                    Role = "viewer",
                    Status = "pending",
                    AuthProvider = info.ProviderType,
                    SsoSubjectId = subjectId,
                    SsoOnly = true,
                    DisplayName = displayName,
                    MustChangePassword = false,
                    CreatedAt = DateTime.UtcNow,
                };
                db.DashUsers.Add(user);
                await db.SaveChangesAsync(ct);
                (userId, role, userStatus) = (user.UserId, user.Role, user.Status);
            }

            if (userStatus is not ("active" or "pending"))
            {
                return LoginError(publicUrl, "account_disabled");
            }

            // Single-use exchange code → the frontend swaps it via POST /auth/sso/exchange.
            var exchangeCode = codes.Issue(userId, email, role);

            http.Response.Headers.Append("Set-Cookie", OidcFlowService.BuildClearStateCookie(publicUrl));
            return Results.Redirect(
                $"{publicUrl}/sso-complete?code={exchangeCode}",
                permanent: false,
                preserveMethod: true);
        }).AllowAnonymous();

        // POST /auth/sso/exchange — swap the short-lived code for the session JWT.
        app.MapPost("/api/auth/sso/exchange", async (
            SsoExchangeRequest req,
            SsoExchangeCodeCache codes,
            NetworkerDbContext db,
            JwtTokenService tokens,
            CancellationToken ct) =>
        {
            var entry = codes.Redeem(req.Code);
            if (entry is null)
            {
                return Results.Unauthorized();
            }

            // Fresh status/is_platform_admin from the DB for the response +
            // token, exactly like Rust sso_exchange.
            var row = await db.DashUsers
                .AsNoTracking()
                .Where(u => u.UserId == entry.UserId)
                .Select(u => new { u.Status, u.IsPlatformAdmin })
                .FirstOrDefaultAsync(ct);

            var status = row?.Status ?? "active";
            var isPlatformAdmin = row?.IsPlatformAdmin ?? false;

            var token = tokens.CreateToken(entry.UserId, entry.Email, entry.Role, isPlatformAdmin);

            // Same shape as POST /auth/login (LoginResponse).
            return Results.Ok(new LoginResponse(token, entry.Role, entry.Email, status, false));
        }).AllowAnonymous();

        return app;
    }

    private static OidcProviderInfo ToInfo(SsoProvider row)
        => new(row.ProviderId, row.ProviderType, row.ClientId, row.IssuerUrl, row.TenantId);

    /// <summary>307 to the login page with an error label (Rust <c>redirect_to_login_with_error</c>).</summary>
    private static IResult LoginError(string publicUrl, string error)
        => Results.Redirect($"{publicUrl}/login?error={error}", permanent: false, preserveMethod: true);

    /// <summary>Login-page provider button data — id/name/type only.</summary>
    public sealed record PublicProviderDto(
        [property: JsonPropertyName("id")] string Id,
        [property: JsonPropertyName("name")] string Name,
        [property: JsonPropertyName("type")] string Type);

    /// <summary>POST /auth/sso/exchange body — matches Rust <c>SsoExchangeRequest</c>.</summary>
    public sealed record SsoExchangeRequest([property: JsonPropertyName("code")] string Code);
}
