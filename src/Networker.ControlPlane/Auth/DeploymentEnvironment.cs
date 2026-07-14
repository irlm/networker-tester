namespace Networker.ControlPlane.Auth;

/// <summary>
/// Startup-time environment detection for fail-closed secret handling.
///
/// <para>Security posture: insecure dev fallbacks (the hardcoded JWT secret in
/// <see cref="AuthExtensions"/>, the ephemeral credential key in
/// <c>Security.CredentialCipherExtensions</c>) are ONLY permitted when the host
/// is explicitly running as Development. Anything else — including an UNSET /
/// undetectable environment — is treated as Production and the app refuses to
/// start without the real secrets.</para>
///
/// <para>Detection order:</para>
/// <list type="number">
///   <item>The <c>IHostEnvironment</c> instance already registered in the
///     <c>IServiceCollection</c> (WebApplicationBuilder registers it before any
///     Program.cs service registration runs). This is what makes
///     <c>WebApplicationFactory</c> integration tests work: the factory forces
///     Development via <c>--environment</c> host-config args, which never appear
///     as a process env var.</item>
///   <item>The raw <c>ASPNETCORE_ENVIRONMENT</c> process variable — for plain
///     <c>ServiceCollection</c> composition (unit tests, tools).</item>
///   <item>Neither present → Production (fail closed).</item>
/// </list>
/// </summary>
public static class DeploymentEnvironment
{
    public const string EnvVar = "ASPNETCORE_ENVIRONMENT";

    /// <summary>
    /// True only when the host environment for this service collection is
    /// Development. Unknown/unset fails closed (treated as Production).
    /// </summary>
    public static bool IsDevelopment(IServiceCollection services)
    {
        foreach (var descriptor in services)
        {
            if (!descriptor.IsKeyedService
                && descriptor.ServiceType == typeof(IHostEnvironment)
                && descriptor.ImplementationInstance is IHostEnvironment env)
            {
                return IsDevelopmentName(env.EnvironmentName);
            }
        }

        return IsDevelopmentName(Environment.GetEnvironmentVariable(EnvVar));
    }

    /// <summary>Pure classification (testable): only exactly "Development"
    /// (case-insensitive, trimmed) counts; null/empty fails closed.</summary>
    public static bool IsDevelopmentName(string? environmentName) =>
        string.Equals(environmentName?.Trim(), "Development", StringComparison.OrdinalIgnoreCase);

    /// <summary>Human-readable current environment for error messages.</summary>
    public static string Describe(IServiceCollection services)
    {
        foreach (var descriptor in services)
        {
            if (!descriptor.IsKeyedService
                && descriptor.ServiceType == typeof(IHostEnvironment)
                && descriptor.ImplementationInstance is IHostEnvironment env)
            {
                return $"'{env.EnvironmentName}'";
            }
        }

        return Environment.GetEnvironmentVariable(EnvVar) is { Length: > 0 } name
            ? $"'{name}' (from {EnvVar})"
            : "<unset, treated as Production>";
    }
}
