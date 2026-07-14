using Microsoft.Extensions.DependencyInjection;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Security;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Locks the fail-closed secret policy: outside Development (including an UNSET
/// ASPNETCORE_ENVIRONMENT, which must be treated as Production), a missing/invalid
/// DASHBOARD_JWT_SECRET or DASHBOARD_CREDENTIAL_KEY must abort startup with an
/// InvalidOperationException instead of silently falling back to the insecure dev
/// secret / an ephemeral key. In Development the dev fallbacks still boot the app.
///
/// All tests live in ONE class so xUnit serializes them — they mutate process
/// environment variables and restore them in finally blocks.
/// </summary>
public class FailClosedSecretsTests
{
    private const string ConnString = "Host=localhost;Database=networker;Username=t;Password=t";

    /// <summary>Run body with the three env vars forced, restoring on exit.</summary>
    private static void WithEnv(string? aspnetEnv, string? jwtSecret, string? credentialKey, Action body)
    {
        var prevEnv = Environment.GetEnvironmentVariable(DeploymentEnvironment.EnvVar);
        var prevSecret = Environment.GetEnvironmentVariable(JwtTokenService.SecretEnvVar);
        var prevKey = Environment.GetEnvironmentVariable(CredentialCipherExtensions.KeyEnvVar);
        try
        {
            Environment.SetEnvironmentVariable(DeploymentEnvironment.EnvVar, aspnetEnv);
            Environment.SetEnvironmentVariable(JwtTokenService.SecretEnvVar, jwtSecret);
            Environment.SetEnvironmentVariable(CredentialCipherExtensions.KeyEnvVar, credentialKey);
            body();
        }
        finally
        {
            Environment.SetEnvironmentVariable(DeploymentEnvironment.EnvVar, prevEnv);
            Environment.SetEnvironmentVariable(JwtTokenService.SecretEnvVar, prevSecret);
            Environment.SetEnvironmentVariable(CredentialCipherExtensions.KeyEnvVar, prevKey);
        }
    }

    // ── DASHBOARD_JWT_SECRET ────────────────────────────────────────────────

    [Fact]
    public void AddNetworkerAuth_Production_WithoutJwtSecret_Throws()
    {
        WithEnv("Production", jwtSecret: null, credentialKey: null, () =>
        {
            var ex = Assert.Throws<InvalidOperationException>(
                () => new ServiceCollection().AddNetworkerAuth(ConnString));
            Assert.Contains(JwtTokenService.SecretEnvVar, ex.Message);
        });
    }

    [Fact]
    public void AddNetworkerAuth_UnsetEnvironment_TreatedAsProduction_Throws()
    {
        WithEnv(aspnetEnv: null, jwtSecret: null, credentialKey: null, () =>
        {
            Assert.Throws<InvalidOperationException>(
                () => new ServiceCollection().AddNetworkerAuth(ConnString));
        });
    }

    [Fact]
    public void AddNetworkerAuth_Development_WithoutJwtSecret_UsesDevFallback()
    {
        WithEnv("Development", jwtSecret: null, credentialKey: null, () =>
        {
            var services = new ServiceCollection().AddNetworkerAuth(ConnString);
            using var provider = services.BuildServiceProvider();
            Assert.NotNull(provider.GetRequiredService<JwtTokenService>());
        });
    }

    [Fact]
    public void AddNetworkerAuth_Production_WithJwtSecret_Succeeds()
    {
        WithEnv("Production", "a-strong-secret-at-least-32-bytes-long!!", credentialKey: null, () =>
        {
            var services = new ServiceCollection().AddNetworkerAuth(ConnString);
            using var provider = services.BuildServiceProvider();
            Assert.NotNull(provider.GetRequiredService<JwtTokenService>());
        });
    }

    // ── DASHBOARD_CREDENTIAL_KEY ────────────────────────────────────────────

    [Fact]
    public void AddCredentialCipher_Production_WithoutKey_Throws()
    {
        WithEnv("Production", jwtSecret: null, credentialKey: null, () =>
        {
            var ex = Assert.Throws<InvalidOperationException>(
                () => new ServiceCollection().AddCredentialCipher());
            Assert.Contains(CredentialCipherExtensions.KeyEnvVar, ex.Message);
        });
    }

    [Fact]
    public void AddCredentialCipher_Production_WithInvalidKey_Throws()
    {
        WithEnv("Production", jwtSecret: null, credentialKey: "not-64-hex-chars", () =>
        {
            var ex = Assert.Throws<InvalidOperationException>(
                () => new ServiceCollection().AddCredentialCipher());
            Assert.Contains(CredentialCipherExtensions.KeyEnvVar, ex.Message);
        });
    }

    [Fact]
    public void AddCredentialCipher_UnsetEnvironment_TreatedAsProduction_Throws()
    {
        WithEnv(aspnetEnv: null, jwtSecret: null, credentialKey: null, () =>
        {
            Assert.Throws<InvalidOperationException>(
                () => new ServiceCollection().AddCredentialCipher());
        });
    }

    [Fact]
    public void AddCredentialCipher_Development_WithoutKey_UsesEphemeralDevKey()
    {
        WithEnv("Development", jwtSecret: null, credentialKey: null, () =>
        {
            var services = new ServiceCollection().AddCredentialCipher();
            Assert.Contains(services, d => d.ServiceType == typeof(Networker.Security.CredentialCipher));
        });
    }

    [Fact]
    public void AddCredentialCipher_Production_WithValidKey_Succeeds()
    {
        WithEnv("Production", jwtSecret: null, credentialKey: new string('a', 64), () =>
        {
            var services = new ServiceCollection().AddCredentialCipher();
            Assert.Contains(services, d => d.ServiceType == typeof(Networker.Security.CredentialCipher));
        });
    }

    // ── host-environment detection (the WebApplicationFactory path) ─────────

    [Fact]
    public void RegisteredHostEnvironment_Development_WinsOverUnsetEnvVar()
    {
        // WebApplicationFactory forces Development via host-config args (never a
        // process env var); AddNetworkerAuth must honor the IHostEnvironment
        // instance already sitting in the service collection.
        WithEnv(aspnetEnv: null, jwtSecret: null, credentialKey: null, () =>
        {
            var services = new ServiceCollection();
            services.AddSingleton<Microsoft.Extensions.Hosting.IHostEnvironment>(
                new FakeHostEnvironment("Development"));

            services.AddNetworkerAuth(ConnString); // must not throw
            services.AddCredentialCipher();        // must not throw
        });
    }

    [Fact]
    public void RegisteredHostEnvironment_Production_FailsClosed()
    {
        WithEnv(aspnetEnv: "Development", jwtSecret: null, credentialKey: null, () =>
        {
            // The registered host environment (Production) must beat the env var.
            var services = new ServiceCollection();
            services.AddSingleton<Microsoft.Extensions.Hosting.IHostEnvironment>(
                new FakeHostEnvironment("Production"));

            Assert.Throws<InvalidOperationException>(() => services.AddNetworkerAuth(ConnString));
        });
    }

    private sealed class FakeHostEnvironment(string name) : Microsoft.Extensions.Hosting.IHostEnvironment
    {
        public string EnvironmentName { get; set; } = name;
        public string ApplicationName { get; set; } = "test";
        public string ContentRootPath { get; set; } = ".";
        public Microsoft.Extensions.FileProviders.IFileProvider ContentRootFileProvider { get; set; } = null!;
    }

    // ── environment classification ──────────────────────────────────────────

    [Theory]
    [InlineData("Development", true)]
    [InlineData("development", true)]
    [InlineData(" Development ", true)]
    [InlineData("Production", false)]
    [InlineData("Staging", false)]
    [InlineData("", false)]   // empty fails closed
    [InlineData(null, false)] // unset fails closed
    public void DeploymentEnvironment_IsDevelopmentName_FailsClosed(string? name, bool expected)
    {
        Assert.Equal(expected, DeploymentEnvironment.IsDevelopmentName(name));
    }
}
