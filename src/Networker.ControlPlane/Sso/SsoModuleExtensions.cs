namespace Networker.ControlPlane.Sso;

/// <summary>
/// DI wiring for the SSO + account module (Phase-2 M5). Program.cs adds
/// <c>builder.Services.AddSsoModule();</c> next to the other module
/// registrations, then maps the three endpoint groups:
/// <code>
/// app.MapAccountEndpoints();   // change/forgot/reset password
/// app.MapSsoEndpoints();       // public OIDC flow (providers/init/callback/exchange)
/// app.MapSsoAdminEndpoints();  // /api/sso-providers CRUD (platform admin)
/// </code>
///
/// Depends on services Program.cs already registers: IMemoryCache +
/// JwtTokenService (AddNetworkerAuth), CredentialCipher (AddCredentialCipher),
/// and NetworkerDbContext.
/// </summary>
public static class SsoModuleExtensions
{
    public static IServiceCollection AddSsoModule(this IServiceCollection services)
    {
        // The only outbound-HTTP seam of the flow. 10s timeout matches the Rust
        // reqwest builders in sso_init/sso_callback/discover_oidc_endpoint.
        services.AddHttpClient<IOidcProviderClient, HttpOidcProviderClient>(c =>
        {
            c.Timeout = TimeSpan.FromSeconds(10);
        });

        services.AddScoped<OidcFlowService>();
        services.AddSingleton<SsoExchangeCodeCache>();
        return services;
    }
}
