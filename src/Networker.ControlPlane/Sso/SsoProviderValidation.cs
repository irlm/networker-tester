namespace Networker.ControlPlane.Sso;

/// <summary>
/// Provider-config validation shared by the admin CRUD — port of
/// <c>validate_provider_config</c> in
/// <c>crates/networker-dashboard/src/api/sso_admin.rs</c> (same rules, same
/// user-facing messages).
/// </summary>
public static class SsoProviderValidation
{
    public static readonly string[] ValidProviderTypes = ["microsoft", "google", "oidc_generic"];

    /// <summary>Returns the error message, or null when the config is valid.</summary>
    public static string? Validate(string providerType, string? issuerUrl, string? tenantId)
    {
        if (!ValidProviderTypes.Contains(providerType))
        {
            return $"Invalid provider_type '{providerType}'. Valid: {string.Join(", ", ValidProviderTypes)}";
        }

        if (providerType == "microsoft" && string.IsNullOrEmpty(tenantId))
        {
            return "tenant_id is required for microsoft provider";
        }

        if (providerType == "oidc_generic")
        {
            if (string.IsNullOrEmpty(issuerUrl))
            {
                return "issuer_url is required for oidc_generic provider";
            }

            if (!issuerUrl.StartsWith("https://", StringComparison.Ordinal))
            {
                return "issuer_url must start with https://";
            }
        }

        return null;
    }
}
