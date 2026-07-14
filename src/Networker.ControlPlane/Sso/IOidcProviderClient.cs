using System.Text.Json;

namespace Networker.ControlPlane.Sso;

/// <summary>
/// The outbound HTTP seam of the OIDC flow: everything that must leave the
/// process to talk to a real identity provider goes through this interface, so
/// unit tests (and CI, which has no route to login.microsoftonline.com /
/// accounts.google.com) can substitute a fake while the rest of the flow —
/// state cookie CSRF, exchange-code cache, user provisioning — runs for real.
///
/// Mirrors the two reqwest round-trips in the Rust
/// <c>crates/networker-dashboard/src/api/auth.rs</c> SSO module:
/// <c>discover_oidc_endpoint</c> (GET the .well-known document) and the
/// authorization-code → token POST.
/// </summary>
public interface IOidcProviderClient
{
    /// <summary>
    /// GET an OIDC discovery document (<c>.well-known/openid-configuration</c>)
    /// and return the parsed JSON. Caller disposes the document.
    /// </summary>
    Task<JsonDocument> GetDiscoveryDocumentAsync(string discoveryUrl, CancellationToken ct = default);

    /// <summary>
    /// POST the authorization-code grant as form-urlencoded to the provider's
    /// token endpoint and return the parsed JSON response. Caller disposes.
    /// </summary>
    Task<JsonDocument> ExchangeCodeAsync(
        string tokenUrl,
        IReadOnlyDictionary<string, string> form,
        CancellationToken ct = default);
}

/// <summary>
/// Production <see cref="IOidcProviderClient"/> backed by a typed
/// <see cref="HttpClient"/> (10s timeout, matching the Rust reqwest builder).
/// Registered via <see cref="SsoModuleExtensions.AddSsoModule"/>.
/// </summary>
public sealed class HttpOidcProviderClient(HttpClient http) : IOidcProviderClient
{
    public async Task<JsonDocument> GetDiscoveryDocumentAsync(string discoveryUrl, CancellationToken ct = default)
    {
        using var resp = await http.GetAsync(discoveryUrl, ct);
        var stream = await resp.Content.ReadAsStreamAsync(ct);
        return await JsonDocument.ParseAsync(stream, cancellationToken: ct);
    }

    public async Task<JsonDocument> ExchangeCodeAsync(
        string tokenUrl,
        IReadOnlyDictionary<string, string> form,
        CancellationToken ct = default)
    {
        // Match Rust: parse the body as JSON regardless of HTTP status — OAuth
        // error responses are JSON too, and the caller decides via the presence
        // of id_token (the Rust code never checks resp.status()).
        using var content = new FormUrlEncodedContent(form);
        using var resp = await http.PostAsync(tokenUrl, content, ct);
        var stream = await resp.Content.ReadAsStreamAsync(ct);
        return await JsonDocument.ParseAsync(stream, cancellationToken: ct);
    }
}
