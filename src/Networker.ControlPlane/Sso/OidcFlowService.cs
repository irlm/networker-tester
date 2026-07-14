using System.Text.Json;

namespace Networker.ControlPlane.Sso;

/// <summary>
/// Provider fields the flow logic needs, decoupled from the EF entity so tests
/// can construct them without a database. Endpoints map
/// <c>Networker.Data.Entities.SsoProvider</c> rows into this.
/// </summary>
public sealed record OidcProviderInfo(
    Guid ProviderId,
    string ProviderType,
    string ClientId,
    string? IssuerUrl,
    string? TenantId);

/// <summary>
/// Thrown when a provider is misconfigured (missing issuer_url, unsupported
/// type). Maps to 400 in the init endpoint, mirroring the Rust match arms.
/// </summary>
public sealed class OidcConfigException(string message) : Exception(message);

/// <summary>
/// The OIDC Authorization Code flow logic, ported from the SSO half of
/// <c>crates/networker-dashboard/src/api/auth.rs</c>. Everything deterministic —
/// authorize-URL construction, state generation/validation, cookie handling,
/// id_token payload decoding, and the iss/aud claim checks — lives here as
/// testable methods; the two provider round-trips (OIDC discovery and the
/// code→token exchange) go through the injected <see cref="IOidcProviderClient"/>.
///
/// <para><b>id_token validation choice:</b> the Rust implementation decodes the
/// id_token payload WITHOUT verifying the JWS signature, on the argument that the
/// token was just received first-hand from the provider's token endpoint over
/// HTTPS in the confidential-client code flow (it never transits the browser).
/// It then validates <c>iss</c>/<c>aud</c> per provider type. This port matches
/// that behavior exactly (see <see cref="ValidateIdTokenClaims"/> and
/// <see cref="DecodeJwtPayload"/>) rather than adding JWKS signature validation,
/// so the two implementations accept/reject identically during the staged
/// cutover.</para>
/// </summary>
public sealed class OidcFlowService(IOidcProviderClient providerClient)
{
    public const string StateCookieName = "sso_state";
    public const string Scope = "openid email profile";
    public const int StateRandomLength = 32;

    // ── Endpoint resolution (Rust: sso_init / sso_callback match arms) ────────

    /// <summary>
    /// Resolve the provider's authorization endpoint. Microsoft/Google are
    /// well-known; oidc_generic goes through discovery (outbound call).
    /// Throws <see cref="OidcConfigException"/> for bad config and lets
    /// discovery I/O errors propagate (endpoint maps them to 502, like Rust's
    /// BAD_GATEWAY arm).
    /// </summary>
    public async Task<string> ResolveAuthorizeEndpointAsync(OidcProviderInfo provider, CancellationToken ct = default)
        => provider.ProviderType switch
        {
            "microsoft" =>
                $"https://login.microsoftonline.com/{provider.TenantId ?? "common"}/oauth2/v2.0/authorize",
            "google" => "https://accounts.google.com/o/oauth2/v2/auth",
            "oidc_generic" => await DiscoverEndpointAsync(provider, "authorization_endpoint", ct),
            _ => throw new OidcConfigException("Unsupported provider type"),
        };

    /// <summary>Resolve the provider's token endpoint (same shape as above).</summary>
    public async Task<string> ResolveTokenEndpointAsync(OidcProviderInfo provider, CancellationToken ct = default)
        => provider.ProviderType switch
        {
            "microsoft" =>
                $"https://login.microsoftonline.com/{provider.TenantId ?? "common"}/oauth2/v2.0/token",
            "google" => "https://oauth2.googleapis.com/token",
            "oidc_generic" => await DiscoverEndpointAsync(provider, "token_endpoint", ct),
            _ => throw new OidcConfigException("Unsupported provider type"),
        };

    private async Task<string> DiscoverEndpointAsync(OidcProviderInfo provider, string key, CancellationToken ct)
    {
        if (string.IsNullOrEmpty(provider.IssuerUrl))
        {
            throw new OidcConfigException("Missing issuer_url");
        }

        var discoveryUrl = $"{provider.IssuerUrl}/.well-known/openid-configuration";
        using var doc = await providerClient.GetDiscoveryDocumentAsync(discoveryUrl, ct);
        if (doc.RootElement.ValueKind == JsonValueKind.Object &&
            doc.RootElement.TryGetProperty(key, out var value) &&
            value.ValueKind == JsonValueKind.String)
        {
            return value.GetString()!;
        }

        throw new InvalidOperationException($"missing {key} in OIDC discovery");
    }

    // ── Authorize redirect + CSRF state (Rust: sso_init) ─────────────────────

    /// <summary>
    /// Build the full authorize redirect for a provider. Returns the redirect
    /// URL and the state value ("{provider_id}:{32 random alnum}") that must
    /// also be set as the <c>sso_state</c> cookie.
    /// </summary>
    public async Task<(string RedirectUrl, string State)> BuildAuthorizeRedirectAsync(
        OidcProviderInfo provider,
        string publicUrl,
        CancellationToken ct = default)
    {
        var authorizeUrl = await ResolveAuthorizeEndpointAsync(provider, ct);
        var state = $"{provider.ProviderId}:{AccountSecurity.GenerateAlphanumericToken(StateRandomLength)}";
        var redirectUri = BuildCallbackRedirectUri(publicUrl);

        var url =
            $"{authorizeUrl}?response_type=code" +
            $"&client_id={Uri.EscapeDataString(provider.ClientId)}" +
            $"&redirect_uri={Uri.EscapeDataString(redirectUri)}" +
            $"&scope={Uri.EscapeDataString(Scope)}" +
            $"&state={Uri.EscapeDataString(state)}";

        return (url, state);
    }

    /// <summary>
    /// The redirect_uri registered with the provider. NOTE: the Rust dashboard
    /// used <c>{public_url}/api/auth/sso/callback</c> (its router nested under
    /// /api); this control plane mounts auth routes without the /api prefix
    /// (see AuthExtensions: /auth/login), so the callback lives at
    /// <c>{public_url}/auth/sso/callback</c>. Provider app registrations must
    /// list this URI.
    /// </summary>
    public static string BuildCallbackRedirectUri(string publicUrl)
        => $"{publicUrl.TrimEnd('/')}/auth/sso/callback";

    /// <summary>
    /// Set-Cookie value for the CSRF state — HttpOnly, SameSite=Lax, Path=/,
    /// 5-minute lifetime, Secure iff the public URL is https (Rust "Fix 3").
    /// </summary>
    public static string BuildStateCookie(string state, string publicUrl)
        => $"{StateCookieName}={state}; HttpOnly; SameSite=Lax; Path=/; Max-Age=300{SecureFlag(publicUrl)}";

    /// <summary>Set-Cookie value that clears the state cookie after the callback.</summary>
    public static string BuildClearStateCookie(string publicUrl)
        => $"{StateCookieName}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0{SecureFlag(publicUrl)}";

    private static string SecureFlag(string publicUrl)
        => publicUrl.StartsWith("https://", StringComparison.OrdinalIgnoreCase) ? "; Secure" : string.Empty;

    /// <summary>
    /// CSRF check: the state echoed by the provider must equal the state we set
    /// in the cookie, byte for byte (Rust compares the raw strings).
    /// </summary>
    public static bool StateMatchesCookie(string? cookieHeader, string callbackState)
    {
        var cookieState = ExtractCookie(cookieHeader ?? string.Empty, StateCookieName);
        return cookieState is not null && cookieState == callbackState;
    }

    /// <summary>Parse a cookie value out of a raw Cookie header (Rust <c>extract_cookie</c>).</summary>
    public static string? ExtractCookie(string header, string name)
    {
        foreach (var part in header.Split(';'))
        {
            var trimmed = part.Trim();
            if (trimmed.StartsWith(name + "=", StringComparison.Ordinal))
            {
                return trimmed[(name.Length + 1)..];
            }
        }

        return null;
    }

    /// <summary>Provider id from the "{provider_id}:{random}" state, or null.</summary>
    public static Guid? ParseProviderIdFromState(string state)
    {
        var prefix = state.Split(':')[0];
        return Guid.TryParse(prefix, out var id) ? id : null;
    }

    // ── Token exchange + id_token handling (Rust: sso_callback) ──────────────

    /// <summary>
    /// Exchange the authorization code at the provider's token endpoint and pull
    /// out the id_token. Returns null when the response carries no id_token
    /// (invalid code, provider error) — the caller redirects with
    /// <c>no_id_token</c>, matching Rust.
    /// </summary>
    public async Task<string?> ExchangeAuthorizationCodeAsync(
        string tokenUrl,
        string code,
        string redirectUri,
        string clientId,
        string clientSecret,
        CancellationToken ct = default)
    {
        var form = new Dictionary<string, string>
        {
            ["grant_type"] = "authorization_code",
            ["code"] = code,
            ["redirect_uri"] = redirectUri,
            ["client_id"] = clientId,
            ["client_secret"] = clientSecret,
        };

        using var doc = await providerClient.ExchangeCodeAsync(tokenUrl, form, ct);
        return doc.RootElement.ValueKind == JsonValueKind.Object &&
               doc.RootElement.TryGetProperty("id_token", out var idToken) &&
               idToken.ValueKind == JsonValueKind.String
            ? idToken.GetString()
            : null;
    }

    /// <summary>
    /// Decode the payload segment of a JWT WITHOUT signature verification
    /// (Rust <c>decode_jwt_payload</c> — see the class remarks for why).
    /// Returns null for malformed tokens.
    /// </summary>
    public static JsonDocument? DecodeJwtPayload(string token)
    {
        var parts = token.Split('.');
        if (parts.Length != 3)
        {
            return null;
        }

        try
        {
            var payload = Base64UrlDecode(parts[1]);
            return JsonDocument.Parse(payload);
        }
        catch (Exception)
        {
            return null;
        }
    }

    private static byte[] Base64UrlDecode(string input)
    {
        var s = input.Replace('-', '+').Replace('_', '/');
        s = (s.Length % 4) switch
        {
            2 => s + "==",
            3 => s + "=",
            0 => s,
            _ => throw new FormatException("invalid base64url length"),
        };
        return Convert.FromBase64String(s);
    }

    /// <summary>
    /// Issuer/audience validation per provider type — the exact checks the Rust
    /// callback performs ("Fix 1"): Microsoft requires an
    /// <c>https://login.microsoftonline.com/</c> issuer prefix + aud == client_id;
    /// Google requires <c>https://accounts.google.com</c> + aud; oidc_generic
    /// checks aud only. Returns an error label or null when valid.
    /// </summary>
    public static string? ValidateIdTokenClaims(string providerType, string clientId, JsonElement claims)
    {
        var issuer = GetStringClaim(claims, "iss") ?? string.Empty;
        var audience = GetStringClaim(claims, "aud") ?? string.Empty;

        return providerType switch
        {
            "microsoft" when !issuer.StartsWith("https://login.microsoftonline.com/", StringComparison.Ordinal)
                             || audience != clientId => "id_token_invalid",
            "google" when issuer != "https://accounts.google.com" || audience != clientId => "id_token_invalid",
            "oidc_generic" when audience != clientId => "id_token_invalid",
            _ => null,
        };
    }

    /// <summary>
    /// Pull the identity out of the validated claims: email (falling back to
    /// preferred_username, lowercased), the stable subject id, and the optional
    /// display name. Null when email or sub is missing (Rust: missing_claims).
    /// </summary>
    public static (string Email, string SubjectId, string? DisplayName)? ExtractIdentity(JsonElement claims)
    {
        var email = (GetStringClaim(claims, "email")
                     ?? GetStringClaim(claims, "preferred_username")
                     ?? string.Empty).ToLowerInvariant();
        var subjectId = GetStringClaim(claims, "sub") ?? string.Empty;
        var displayName = GetStringClaim(claims, "name");

        if (email.Length == 0 || subjectId.Length == 0)
        {
            return null;
        }

        return (email, subjectId, displayName);
    }

    private static string? GetStringClaim(JsonElement claims, string name)
        => claims.ValueKind == JsonValueKind.Object &&
           claims.TryGetProperty(name, out var v) &&
           v.ValueKind == JsonValueKind.String
            ? v.GetString()
            : null;
}
