using System.Text.RegularExpressions;
using Networker.ControlPlane.Provisioning;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Tests for the pure tester-create decision logic ported from the Rust
/// dashboard's <c>api/testers.rs</c> (<c>check_rate_limit</c>, body validation,
/// <c>target_triple_for</c>), <c>db/project_testers.rs</c> (COALESCE defaults),
/// and <c>services/cloud_provider.rs</c> (<c>resolve_image</c>,
/// <c>default_ssh_user</c>, <c>generate_vm_name</c>, <c>from_connection</c>
/// config validation). Mirrors the Rust unit tests where they exist.
/// </summary>
public sealed class TesterCreateLogicTests
{
    // ── check_rate_limit (mirrors the Rust unit tests) ─────────────────────

    [Theory]
    [InlineData(0, 0)]
    [InlineData(19, 0)]
    [InlineData(19, 19)]
    [InlineData(0, 19)]
    public void CheckRateLimit_under_both_caps_allows(long total, long lastHour)
    {
        Assert.Null(TesterCreateLogic.CheckRateLimit(total, lastHour));
    }

    [Theory]
    [InlineData(20)]
    [InlineData(21)]
    [InlineData(100)]
    public void CheckRateLimit_at_or_over_total_cap_rejects_with_rust_message(long total)
    {
        var msg = TesterCreateLogic.CheckRateLimit(total, 0);
        Assert.Equal($"project already has {total} testers (max 20)", msg);
    }

    [Theory]
    [InlineData(20)]
    [InlineData(25)]
    public void CheckRateLimit_at_or_over_hourly_cap_rejects_with_rust_message(long lastHour)
    {
        var msg = TesterCreateLogic.CheckRateLimit(5, lastHour);
        Assert.Equal($"project created {lastHour} testers in the last hour (max 20/h)", msg);
    }

    [Fact]
    public void CheckRateLimit_total_cap_wins_when_both_exceeded()
    {
        // Rust checks the total cap first.
        var msg = TesterCreateLogic.CheckRateLimit(20, 20);
        Assert.Equal("project already has 20 testers (max 20)", msg);
    }

    // ── body validation ─────────────────────────────────────────────────────

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("   ")]
    public void ValidateCreateBody_empty_name_rejected(string? name)
    {
        Assert.Equal(
            "name must not be empty",
            TesterCreateLogic.ValidateCreateBody(name, "azure", "eastus"));
    }

    [Theory]
    [InlineData("", "eastus")]
    [InlineData("azure", "")]
    [InlineData("  ", "eastus")]
    [InlineData("azure", "  ")]
    public void ValidateCreateBody_empty_cloud_or_region_rejected(string cloud, string region)
    {
        Assert.Equal(
            "cloud and region are required",
            TesterCreateLogic.ValidateCreateBody("my-tester", cloud, region));
    }

    [Fact]
    public void ValidateCreateBody_valid_input_passes()
    {
        Assert.Null(TesterCreateLogic.ValidateCreateBody("my-tester", "azure", "eastus"));
    }

    // ── COALESCE defaults (db/project_testers.rs insert) ───────────────────

    [Fact]
    public void ApplyDefaults_all_null_uses_rust_insert_defaults()
    {
        var d = TesterCreateLogic.ApplyDefaults(null, null, null, null, null);
        Assert.Equal("Standard_B2s", d.VmSize);
        Assert.Equal(23, d.AutoShutdownLocalHour);
        Assert.False(d.AutoProbeEnabled);
        Assert.Equal("ubuntu-24.04", d.RequestedOs);
        Assert.Equal("server", d.RequestedVariant);
    }

    [Fact]
    public void ApplyDefaults_explicit_values_pass_through()
    {
        var d = TesterCreateLogic.ApplyDefaults("Standard_D4s_v3", 8, true, "windows-2022", "desktop");
        Assert.Equal("Standard_D4s_v3", d.VmSize);
        Assert.Equal(8, d.AutoShutdownLocalHour);
        Assert.True(d.AutoProbeEnabled);
        Assert.Equal("windows-2022", d.RequestedOs);
        Assert.Equal("desktop", d.RequestedVariant);
    }

    [Fact]
    public void ApplyDefaults_hour_zero_is_respected_not_defaulted()
    {
        // COALESCE only replaces NULL — a midnight shutdown hour must survive.
        var d = TesterCreateLogic.ApplyDefaults(null, 0, false, null, null);
        Assert.Equal(0, d.AutoShutdownLocalHour);
        Assert.False(d.AutoProbeEnabled);
    }

    // ── connection config validation (from_connection / from_config) ───────

    [Fact]
    public void ValidateConnectionConfig_unknown_provider_rejected()
    {
        Assert.Equal(
            "unsupported cloud provider: digitalocean",
            TesterCreateLogic.ValidateConnectionConfig("digitalocean", "{}"));
    }

    [Fact]
    public void ValidateConnectionConfig_azure_requires_subscription_and_rg()
    {
        Assert.Equal(
            "azure config: missing subscription_id",
            TesterCreateLogic.ValidateConnectionConfig("azure", "{}"));
        Assert.Equal(
            "azure config: missing resource_group",
            TesterCreateLogic.ValidateConnectionConfig("azure", "{\"subscription_id\":\"sub\"}"));
        Assert.Null(TesterCreateLogic.ValidateConnectionConfig(
            "azure", "{\"subscription_id\":\"sub\",\"resource_group\":\"rg\"}"));
    }

    [Fact]
    public void ValidateConnectionConfig_aws_requires_key_material()
    {
        Assert.Equal(
            "aws config: missing access_key_id or secret_access_key",
            TesterCreateLogic.ValidateConnectionConfig("aws", "{}"));
        Assert.Equal(
            "aws config: missing access_key_id or secret_access_key",
            TesterCreateLogic.ValidateConnectionConfig("aws", "{\"access_key_id\":\"AKIA\"}"));
        Assert.Null(TesterCreateLogic.ValidateConnectionConfig(
            "aws", "{\"access_key_id\":\"AKIA\",\"secret_access_key\":\"s3cr3t\"}"));
    }

    [Fact]
    public void ValidateConnectionConfig_gcp_requires_valid_json_key()
    {
        Assert.Equal(
            "gcp config: missing json_key",
            TesterCreateLogic.ValidateConnectionConfig("gcp", "{}"));
        Assert.Equal(
            "gcp config: json_key is not valid JSON",
            TesterCreateLogic.ValidateConnectionConfig("gcp", "{\"json_key\":\"not-json\"}"));
        Assert.Equal(
            "gcp json_key: missing project_id",
            TesterCreateLogic.ValidateConnectionConfig("gcp", "{\"json_key\":\"{}\"}"));
        Assert.Null(TesterCreateLogic.ValidateConnectionConfig(
            "gcp", "{\"json_key\":\"{\\\"project_id\\\":\\\"my-proj\\\"}\"}"));
    }

    // ── target_triple_for (mirrors the Rust unit test) ──────────────────────

    [Theory]
    [InlineData("ubuntu-24.04", "x86_64-unknown-linux-musl")]
    [InlineData("rhel-9", "x86_64-unknown-linux-musl")]
    [InlineData("debian-12", "x86_64-unknown-linux-musl")]
    [InlineData("windows-2022", "x86_64-pc-windows-msvc")]
    [InlineData("windows-server-2019", "x86_64-pc-windows-msvc")]
    public void TargetTripleFor_maps_os_family(string os, string expected)
    {
        Assert.Equal(expected, TesterCreateLogic.TargetTripleFor(os));
    }

    // ── resolve_image ────────────────────────────────────────────────────────

    [Theory]
    [InlineData("azure", "ubuntu-24.04", "server", "Canonical:ubuntu-24_04-lts:server:latest")]
    [InlineData("azure", "ubuntu-24.04", "desktop", "Canonical:ubuntu-24_04-lts:server:latest")]
    [InlineData("azure", "ubuntu-22.04", "server", "Canonical:0001-com-ubuntu-server-jammy:22_04-lts-gen2:latest")]
    [InlineData("azure", "windows-2022", "server", "MicrosoftWindowsServer:WindowsServer:2022-datacenter-azure-edition:latest")]
    [InlineData("azure", "windows-11", "desktop", "MicrosoftWindowsDesktop:windows-11:win11-24h2-pro:latest")]
    [InlineData("azure", "debian-12", "server", "Debian:debian-12:12:latest")]
    [InlineData("aws", "ubuntu-24.04", "server", "aws:ubuntu-24.04-server")]
    [InlineData("aws", "windows-2022", "server", "aws:windows-2022-server")]
    [InlineData("gcp", "ubuntu-24.04", "server", "ubuntu-2404-lts-amd64")]
    [InlineData("gcp", "debian-12", "server", "debian-12")]
    // Fallbacks
    [InlineData("azure", "something-else", "server", "Canonical:ubuntu-24_04-lts:server:latest")]
    [InlineData("aws", "something-else", "server", "aws:ubuntu-24.04-server")]
    [InlineData("gcp", "something-else", "server", "ubuntu-2404-lts-amd64")]
    [InlineData("unknown-cloud", "ubuntu-24.04", "server", "ubuntu-24.04-server")]
    public void ResolveImage_matches_rust_catalog(string cloud, string os, string variant, string expected)
    {
        Assert.Equal(expected, TesterCreateLogic.ResolveImage(cloud, os, variant));
    }

    // ── default_ssh_user ─────────────────────────────────────────────────────

    [Theory]
    [InlineData("azure", "windows-2022", "azureadmin")]
    [InlineData("aws", "windows-2022", "azureadmin")]
    [InlineData("azure", "ubuntu-24.04", "azureuser")]
    [InlineData("azure", "debian-12", "azureuser")]
    [InlineData("aws", "debian-12", "admin")]
    [InlineData("gcp", "debian-12", "admin")]
    [InlineData("aws", "ubuntu-24.04", "ubuntu")]
    [InlineData("gcp", "ubuntu-24.04", "ubuntu")]
    [InlineData("other", "ubuntu-24.04", "ubuntu")]
    public void DefaultSshUser_matches_rust(string cloud, string os, string expected)
    {
        Assert.Equal(expected, TesterCreateLogic.DefaultSshUser(cloud, os));
    }

    // ── generate_vm_name / generate_agent_api_key ────────────────────────────

    [Fact]
    public void GenerateVmName_is_dns_safe_with_5_hex_suffix()
    {
        var name = TesterCreateLogic.GenerateVmName("eastus");
        Assert.Matches(new Regex("^tester-eastus-[0-9a-f]{5}$"), name);
    }

    [Fact]
    public void GenerateAgentApiKey_is_48_alnum_and_unique()
    {
        var a = TesterCreateLogic.GenerateAgentApiKey();
        var b = TesterCreateLogic.GenerateAgentApiKey();
        Assert.Equal(48, a.Length);
        Assert.Matches(new Regex("^[A-Za-z0-9]{48}$"), a);
        Assert.NotEqual(a, b);

        // Must pass the bootstrap renderer's whitelist (32-128 alnum).
        CloudInitScripts.ValidateInputs("https://dash.example.com", a, "x86_64-unknown-linux-musl");
    }
}
