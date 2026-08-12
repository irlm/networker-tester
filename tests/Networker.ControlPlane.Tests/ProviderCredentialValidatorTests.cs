using Networker.ControlPlane.Provisioning;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// The pure parts of the real credential validator (2026-08-11: validation
/// had been a field-presence stub since the phase-3 port — an expired AWS
/// key showed "active"). Cloud/CLI round-trips are exercised in prod, not
/// CI; these pin the fast-fail paths and the Rust-parity AADSTS mapping.
/// </summary>
public sealed class ProviderCredentialValidatorTests
{
    private static ProviderCredentialValidator NewValidator()
    {
        var services = new Microsoft.Extensions.DependencyInjection.ServiceCollection();
        Microsoft.Extensions.DependencyInjection.HttpClientFactoryServiceCollectionExtensions.AddHttpClient(services);
        var sp = Microsoft.Extensions.DependencyInjection.ServiceCollectionContainerBuilderExtensions.BuildServiceProvider(services);
        return new ProviderCredentialValidator(
            Microsoft.Extensions.DependencyInjection.ServiceProviderServiceExtensions.GetRequiredService<System.Net.Http.IHttpClientFactory>(sp),
            Microsoft.Extensions.Logging.Abstractions.NullLogger<ProviderCredentialValidator>.Instance);
    }

    [Theory]
    [InlineData("azure", "Missing client_id, client_secret, or tenant_id")]
    [InlineData("aws", "Missing access_key_id or secret_access_key")]
    [InlineData("gcp", "Missing json_key")]
    public async Task Missing_fields_fast_fail_without_any_provider_call(string provider, string expected)
    {
        var v = NewValidator();
        var (status, error) = await v.ValidateAsync(provider, new Dictionary<string, string>(), CancellationToken.None);
        Assert.Equal("error", status);
        Assert.Equal(expected, error);
    }

    [Fact]
    public async Task Unknown_provider_is_an_error()
    {
        var v = NewValidator();
        var (status, error) = await v.ValidateAsync("digitalocean", new Dictionary<string, string>(), CancellationToken.None);
        Assert.Equal("error", status);
        Assert.Contains("Unknown provider", error);
    }

    [Theory]
    [InlineData("invalid_client", "", "Invalid client secret")]
    [InlineData("", "AADSTS7000215: Invalid client secret provided", "Invalid client secret")]
    [InlineData("", "AADSTS700016: Application not found in the directory", "not found in tenant")]
    [InlineData("", "AADSTS90002: Tenant 'x' not found", "Tenant 'tid' not found")]
    [InlineData("", "Some other error\rwith CRLF tail", "Azure: Some other error")]
    public void Azure_error_mapping_matches_rust(string code, string desc, string expectedFragment)
    {
        var msg = ProviderCredentialValidator.MapAzureError(code, desc, "cid", "tid", 401);
        Assert.Contains(expectedFragment, msg);
    }

    [Fact]
    public void Azure_error_mapping_falls_back_to_http_status()
    {
        var msg = ProviderCredentialValidator.MapAzureError("", "", "cid", "tid", 503);
        Assert.Equal("Azure token request failed (HTTP 503)", msg);
    }
    // ── stderr cleaners (2026-08-12: the raw aws CLI message was three
    // layers of boilerplate and the UI truncated exactly where the error
    // code started — the visible fragment carried zero signal) ──────────

    [Theory]
    [InlineData(
        "AWS validation failed: aws: [ERROR]: An error occurred (InvalidClientTokenId) when calling the GetCallerIdentity operation: The security token included in the request is invalid.",
        "Invalid access key ID — it does not exist or is deactivated (InvalidClientTokenId).")]
    [InlineData(
        "AWS validation failed: An error occurred (SignatureDoesNotMatch) when calling the GetCallerIdentity operation: Signature mismatch",
        "Secret access key does not match the access key ID (SignatureDoesNotMatch).")]
    [InlineData(
        "An error occurred (ExpiredToken) when calling the GetCallerIdentity operation: token expired",
        "Temporary credentials have expired — generate a fresh session token (ExpiredToken).")]
    [InlineData(
        "An error occurred (SomethingNew) when calling the GetCallerIdentity operation: unusual failure.",
        "SomethingNew: unusual failure.")]
    public void Aws_errors_lead_with_the_human_reason(string raw, string expected)
    {
        Assert.Equal(expected, ProviderCredentialValidator.CleanAwsError(raw));
    }

    [Fact]
    public void Aws_unrecognized_stderr_passes_through()
    {
        var raw = "aws CLI not available";
        Assert.Equal(raw, ProviderCredentialValidator.CleanAwsError(raw));
    }

    [Fact]
    public void Gcloud_prefix_is_stripped()
    {
        var raw = "GCP validation failed: ERROR: (gcloud.auth.activate-service-account) There was a problem refreshing auth tokens: invalid_grant";
        var cleaned = ProviderCredentialValidator.CleanGcloudError(raw);
        Assert.DoesNotContain("(gcloud.auth", cleaned);
        Assert.Contains("invalid_grant", cleaned);
    }
}
