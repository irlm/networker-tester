using System.Text;
using System.Text.Json;
using Networker.ControlPlane.Sso;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Offline tests of the OIDC flow logic — endpoint resolution, CSRF state
/// cookie round-trip, id_token payload decoding, and the per-provider iss/aud
/// checks — with the provider HTTP round-trips faked through
/// <see cref="IOidcProviderClient"/> (CI has no route to real providers).
/// </summary>
public sealed class OidcFlowServiceTests
{
    private sealed class FakeProviderClient : IOidcProviderClient
    {
        public string? DiscoveryJson { get; set; }
        public string? TokenJson { get; set; }
        public string? LastDiscoveryUrl { get; private set; }
        public string? LastTokenUrl { get; private set; }
        public IReadOnlyDictionary<string, string>? LastForm { get; private set; }

        public Task<JsonDocument> GetDiscoveryDocumentAsync(string discoveryUrl, CancellationToken ct = default)
        {
            LastDiscoveryUrl = discoveryUrl;
            return Task.FromResult(JsonDocument.Parse(DiscoveryJson ?? "{}"));
        }

        public Task<JsonDocument> ExchangeCodeAsync(
            string tokenUrl, IReadOnlyDictionary<string, string> form, CancellationToken ct = default)
        {
            LastTokenUrl = tokenUrl;
            LastForm = form;
            return Task.FromResult(JsonDocument.Parse(TokenJson ?? "{}"));
        }
    }

    private static readonly Guid ProviderId = Guid.Parse("6f9619ff-8b86-d011-b42d-00c04fc964ff");

    private static OidcProviderInfo Microsoft(string? tenant = "contoso.onmicrosoft.com")
        => new(ProviderId, "microsoft", "ms-client", null, tenant);

    private static OidcProviderInfo Google()
        => new(ProviderId, "google", "goog-client", null, null);

    private static OidcProviderInfo Generic(string? issuer = "https://auth.example.com")
        => new(ProviderId, "oidc_generic", "gen-client", issuer, null);

    // ── Endpoint resolution ───────────────────────────────────────────────────

    [Fact]
    public async Task AuthorizeEndpoint_Microsoft_UsesTenant_AndCommonFallback()
    {
        var flow = new OidcFlowService(new FakeProviderClient());

        Assert.Equal(
            "https://login.microsoftonline.com/contoso.onmicrosoft.com/oauth2/v2.0/authorize",
            await flow.ResolveAuthorizeEndpointAsync(Microsoft()));
        Assert.Equal(
            "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
            await flow.ResolveAuthorizeEndpointAsync(Microsoft(tenant: null)));
    }

    [Fact]
    public async Task TokenEndpoint_WellKnownProviders()
    {
        var flow = new OidcFlowService(new FakeProviderClient());

        Assert.Equal(
            "https://login.microsoftonline.com/contoso.onmicrosoft.com/oauth2/v2.0/token",
            await flow.ResolveTokenEndpointAsync(Microsoft()));
        Assert.Equal("https://oauth2.googleapis.com/token", await flow.ResolveTokenEndpointAsync(Google()));
    }

    [Fact]
    public async Task GenericProvider_ResolvesEndpointsViaDiscovery()
    {
        var client = new FakeProviderClient
        {
            DiscoveryJson = """
                {
                  "authorization_endpoint": "https://auth.example.com/authorize",
                  "token_endpoint": "https://auth.example.com/token"
                }
                """,
        };
        var flow = new OidcFlowService(client);

        Assert.Equal("https://auth.example.com/authorize", await flow.ResolveAuthorizeEndpointAsync(Generic()));
        Assert.Equal("https://auth.example.com/token", await flow.ResolveTokenEndpointAsync(Generic()));
        Assert.Equal(
            "https://auth.example.com/.well-known/openid-configuration",
            client.LastDiscoveryUrl);
    }

    [Fact]
    public async Task GenericProvider_MissingIssuer_ThrowsConfigError()
    {
        var flow = new OidcFlowService(new FakeProviderClient());
        await Assert.ThrowsAsync<OidcConfigException>(() => flow.ResolveAuthorizeEndpointAsync(Generic(issuer: null)));
    }

    [Fact]
    public async Task UnknownProviderType_ThrowsConfigError()
    {
        var flow = new OidcFlowService(new FakeProviderClient());
        var saml = new OidcProviderInfo(ProviderId, "saml", "cid", null, null);
        await Assert.ThrowsAsync<OidcConfigException>(() => flow.ResolveAuthorizeEndpointAsync(saml));
    }

    // ── Authorize redirect + state ────────────────────────────────────────────

    [Fact]
    public async Task BuildAuthorizeRedirect_EmbedsStateClientIdAndCallback()
    {
        var flow = new OidcFlowService(new FakeProviderClient());
        var (url, state) = await flow.BuildAuthorizeRedirectAsync(Google(), "https://dash.example.com");

        Assert.StartsWith("https://accounts.google.com/o/oauth2/v2/auth?response_type=code", url);
        Assert.Contains("&client_id=goog-client", url);
        Assert.Contains(Uri.EscapeDataString("https://dash.example.com/auth/sso/callback"), url);
        Assert.Contains($"&state={Uri.EscapeDataString(state)}", url);
        Assert.Contains(Uri.EscapeDataString("openid email profile"), url);

        // State format "{provider_id}:{32 random alnum}" — the callback parses
        // the provider back out of it.
        Assert.StartsWith($"{ProviderId}:", state);
        Assert.Equal(ProviderId, OidcFlowService.ParseProviderIdFromState(state));
        Assert.Equal(OidcFlowService.StateRandomLength, state.Split(':')[1].Length);
    }

    [Fact]
    public void StateCookie_RoundTrip_Matches()
    {
        var state = $"{ProviderId}:abc123XYZ";
        var cookie = OidcFlowService.BuildStateCookie(state, "http://localhost:3000");

        Assert.Contains("HttpOnly", cookie);
        Assert.Contains("SameSite=Lax", cookie);
        Assert.Contains("Max-Age=300", cookie);
        Assert.DoesNotContain("Secure", cookie); // http public URL → no Secure flag

        // Simulate the browser sending the cookie back on the callback.
        var cookieHeader = $"other=1; {OidcFlowService.StateCookieName}={state}; theme=dark";
        Assert.True(OidcFlowService.StateMatchesCookie(cookieHeader, state));
        Assert.False(OidcFlowService.StateMatchesCookie(cookieHeader, $"{ProviderId}:tampered"));
        Assert.False(OidcFlowService.StateMatchesCookie(null, state));
        Assert.False(OidcFlowService.StateMatchesCookie("unrelated=x", state));
    }

    [Fact]
    public void StateCookie_HttpsPublicUrl_SetsSecureFlag()
    {
        var cookie = OidcFlowService.BuildStateCookie("s", "https://dash.example.com");
        Assert.EndsWith("; Secure", cookie);

        var clear = OidcFlowService.BuildClearStateCookie("https://dash.example.com");
        Assert.Contains("Max-Age=0", clear);
        Assert.EndsWith("; Secure", clear);
    }

    [Fact]
    public void ParseProviderIdFromState_RejectsGarbage()
    {
        Assert.Null(OidcFlowService.ParseProviderIdFromState("not-a-uuid:random"));
        Assert.Null(OidcFlowService.ParseProviderIdFromState(""));
    }

    // ── Code → token exchange ─────────────────────────────────────────────────

    [Fact]
    public async Task ExchangeAuthorizationCode_SendsCodeGrantForm_AndReturnsIdToken()
    {
        var client = new FakeProviderClient { TokenJson = """{"id_token":"header.payload.sig"}""" };
        var flow = new OidcFlowService(client);

        var idToken = await flow.ExchangeAuthorizationCodeAsync(
            "https://oauth2.googleapis.com/token", "auth-code", "https://d/auth/sso/callback", "cid", "secret");

        Assert.Equal("header.payload.sig", idToken);
        Assert.NotNull(client.LastForm);
        Assert.Equal("authorization_code", client.LastForm!["grant_type"]);
        Assert.Equal("auth-code", client.LastForm["code"]);
        Assert.Equal("cid", client.LastForm["client_id"]);
        Assert.Equal("secret", client.LastForm["client_secret"]);
        Assert.Equal("https://d/auth/sso/callback", client.LastForm["redirect_uri"]);
    }

    [Fact]
    public async Task ExchangeAuthorizationCode_NoIdToken_ReturnsNull()
    {
        var client = new FakeProviderClient { TokenJson = """{"error":"invalid_grant"}""" };
        var flow = new OidcFlowService(client);

        Assert.Null(await flow.ExchangeAuthorizationCodeAsync("https://t", "c", "r", "id", "sec"));
    }

    // ── id_token decoding + claim validation ─────────────────────────────────

    private static string MakeIdToken(object payload)
    {
        static string B64Url(byte[] bytes) =>
            Convert.ToBase64String(bytes).TrimEnd('=').Replace('+', '-').Replace('/', '_');

        var header = B64Url(Encoding.UTF8.GetBytes("""{"alg":"RS256","typ":"JWT"}"""));
        var body = B64Url(JsonSerializer.SerializeToUtf8Bytes(payload));
        return $"{header}.{body}.fakesignature";
    }

    [Fact]
    public void DecodeJwtPayload_ReadsClaims_WithoutSignatureVerification()
    {
        var token = MakeIdToken(new { iss = "https://accounts.google.com", aud = "cid", email = "User@Example.COM", sub = "s-1", name = "Ada" });

        using var doc = OidcFlowService.DecodeJwtPayload(token);
        Assert.NotNull(doc);
        Assert.Equal("https://accounts.google.com", doc.RootElement.GetProperty("iss").GetString());

        var identity = OidcFlowService.ExtractIdentity(doc.RootElement);
        Assert.NotNull(identity);
        Assert.Equal("user@example.com", identity.Value.Email); // lowercased, like Rust
        Assert.Equal("s-1", identity.Value.SubjectId);
        Assert.Equal("Ada", identity.Value.DisplayName);
    }

    [Fact]
    public void DecodeJwtPayload_RejectsMalformedTokens()
    {
        Assert.Null(OidcFlowService.DecodeJwtPayload("only.two"));
        Assert.Null(OidcFlowService.DecodeJwtPayload("a.!!!notbase64!!!.c"));
        Assert.Null(OidcFlowService.DecodeJwtPayload(string.Empty));
    }

    [Fact]
    public void ExtractIdentity_FallsBackToPreferredUsername_AndRequiresSub()
    {
        using var withPreferred = JsonDocument.Parse(
            """{"preferred_username":"PU@Corp.com","sub":"s2"}""");
        var id = OidcFlowService.ExtractIdentity(withPreferred.RootElement);
        Assert.NotNull(id);
        Assert.Equal("pu@corp.com", id.Value.Email);

        using var noSub = JsonDocument.Parse("""{"email":"a@b.com"}""");
        Assert.Null(OidcFlowService.ExtractIdentity(noSub.RootElement));

        using var noEmail = JsonDocument.Parse("""{"sub":"s3"}""");
        Assert.Null(OidcFlowService.ExtractIdentity(noEmail.RootElement));
    }

    [Theory]
    // Microsoft: issuer must be under login.microsoftonline.com AND aud must match.
    [InlineData("microsoft", "https://login.microsoftonline.com/tid/v2.0", "cid", null)]
    [InlineData("microsoft", "https://evil.example.com/", "cid", "id_token_invalid")]
    [InlineData("microsoft", "https://login.microsoftonline.com/tid/v2.0", "other", "id_token_invalid")]
    // Google: exact issuer AND aud.
    [InlineData("google", "https://accounts.google.com", "cid", null)]
    [InlineData("google", "https://accounts.google.evil", "cid", "id_token_invalid")]
    [InlineData("google", "https://accounts.google.com", "other", "id_token_invalid")]
    // Generic: aud only (issuer is whatever the discovery doc said).
    [InlineData("oidc_generic", "https://auth.example.com", "cid", null)]
    [InlineData("oidc_generic", "https://anything.example.com", "cid", null)]
    [InlineData("oidc_generic", "https://auth.example.com", "other", "id_token_invalid")]
    public void ValidateIdTokenClaims_PerProviderRules(string type, string iss, string aud, string? expectedError)
    {
        using var claims = JsonDocument.Parse(JsonSerializer.Serialize(new { iss, aud }));
        Assert.Equal(expectedError, OidcFlowService.ValidateIdTokenClaims(type, "cid", claims.RootElement));
    }

    // ── Provider-config validation (admin CRUD) ──────────────────────────────

    [Theory]
    [InlineData("google", null, null, null)]
    [InlineData("microsoft", null, "tenant-1", null)]
    [InlineData("microsoft", null, null, "tenant_id is required for microsoft provider")]
    [InlineData("microsoft", null, "", "tenant_id is required for microsoft provider")]
    [InlineData("oidc_generic", "https://auth.example.com", null, null)]
    [InlineData("oidc_generic", null, null, "issuer_url is required for oidc_generic provider")]
    [InlineData("oidc_generic", "http://auth.example.com", null, "issuer_url must start with https://")]
    public void ProviderConfigValidation_MatchesRustRules(
        string type, string? issuer, string? tenant, string? expectedError)
    {
        Assert.Equal(expectedError, SsoProviderValidation.Validate(type, issuer, tenant));
    }

    [Fact]
    public void ProviderConfigValidation_RejectsUnknownType()
    {
        var error = SsoProviderValidation.Validate("saml", null, null);
        Assert.NotNull(error);
        Assert.Contains("Invalid provider_type", error);
    }
}
