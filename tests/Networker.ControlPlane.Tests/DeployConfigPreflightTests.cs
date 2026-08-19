using System.Text.Json.Nodes;
using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pins the create-time deploy-config pre-flight (v0.28.200) — the
/// OS↔stack/language contradictions install.sh would reject after the
/// deployment row already existed (user-caught 2026-08-12: an invisible
/// nginx selection rode a Windows config through the wizard and failed
/// post-create). The pre-flight mirrors install.sh's
/// <c>_deploy_validate_config</c> rules — including the NESTED os location
/// (<c>endpoints[i].{provider}.os</c>) — and must stay a mirror, never a
/// superset: what install.sh accepts must pass here.
/// </summary>
public class DeployConfigPreflightTests
{
    private static JsonNode Config(string endpointJson) =>
        JsonNode.Parse($$"""
            { "version": 1, "tester": { "provider": "local" }, "endpoints": [ {{endpointJson}} ] }
            """)!;

    [Fact]
    public void nginx_on_windows_is_rejected()
    {
        var errors = DeployConfigPreflight.Validate(Config("""
            { "provider": "azure", "http_stacks": ["nginx", "iis"],
              "azure": { "region": "eastus", "os": "windows" } }
            """));
        Assert.Contains(errors, e => e.Contains("nginx requires Linux"));
    }

    [Fact]
    public void iis_on_linux_is_rejected()
    {
        var errors = DeployConfigPreflight.Validate(Config("""
            { "provider": "azure", "http_stacks": ["iis"],
              "azure": { "region": "eastus", "os": "linux" } }
            """));
        Assert.Contains(errors, e => e.Contains("IIS requires Windows"));
    }

    [Fact]
    public void windows_viable_languages_pass_on_windows()
    {
        // v0.28.204: Windows endpoints deploy languages via install.ps1
        // -BenchmarkServer — the windows-viable set validates clean.
        var errors = DeployConfigPreflight.Validate(Config("""
            { "provider": "azure", "http_stacks": ["iis"],
              "languages": ["csharp-net48", "go", "nodejs", "python", "java", "csharp-net10"],
              "azure": { "region": "eastus", "os": "windows" } }
            """));
        Assert.Empty(errors);
    }

    [Theory]
    [InlineData("php")]  // swoole is Linux-only
    [InlineData("ruby")] // devkit gem builds
    [InlineData("cpp")]  // MSVC+boost build
    [InlineData("csharp-net8-aot")]  // needs the VS C++ toolchain
    [InlineData("csharp-net9-aot")]  // needs the VS C++ toolchain
    [InlineData("csharp-net10-aot")] // needs the VS C++ toolchain
    public void linux_only_languages_are_rejected_on_windows(string lang)
    {
        var errors = DeployConfigPreflight.Validate(Config($$"""
            { "provider": "azure", "http_stacks": ["iis"], "languages": ["{{lang}}"],
              "azure": { "region": "eastus", "os": "windows" } }
            """));
        Assert.Contains(errors, e => e.Contains($"'{lang}' is not deployable on a windows endpoint"));
    }

    [Fact]
    public void net48_stays_rejected_on_linux()
    {
        var errors = DeployConfigPreflight.Validate(Config("""
            { "provider": "azure", "http_stacks": ["nginx"], "languages": ["csharp-net48"],
              "azure": { "region": "eastus", "os": "linux" } }
            """));
        Assert.Contains(errors, e => e.Contains("'csharp-net48' is not deployable on a linux endpoint"));
    }

    [Theory]
    [InlineData("csharp-net8-aot")]
    [InlineData("csharp-net9-aot")]
    [InlineData("csharp-net10-aot")]
    public void all_dotnet_aot_variants_pass_on_linux(string lang)
    {
        // Issue #801 pattern A: net9-aot/net10-aot were missing from the
        // Linux set (only net8-aot was listed), so every net9-aot@linux
        // matrix cell failed provisioning at install.sh's validator.
        var errors = DeployConfigPreflight.Validate(Config($$"""
            { "provider": "azure", "http_stacks": ["nginx"], "languages": ["{{lang}}"],
              "azure": { "region": "eastus", "os": "linux" } }
            """));
        Assert.Empty(errors);
    }

    [Fact]
    public void unknown_language_is_named_as_unknown_not_os_mismatched()
    {
        var errors = DeployConfigPreflight.Validate(Config("""
            { "provider": "azure", "http_stacks": ["nginx"], "languages": ["cobol"],
              "azure": { "region": "eastus", "os": "linux" } }
            """));
        Assert.Contains(errors, e => e.Contains("unknown language 'cobol'"));
    }

    [Fact]
    public void valid_linux_config_with_languages_passes()
    {
        var errors = DeployConfigPreflight.Validate(Config("""
            { "provider": "azure", "http_stacks": ["nginx", "caddy"], "languages": ["go", "rust"],
              "azure": { "region": "eastus", "os": "linux" } }
            """));
        Assert.Empty(errors);
    }

    [Fact]
    public void valid_windows_config_passes()
    {
        var errors = DeployConfigPreflight.Validate(Config("""
            { "provider": "azure", "http_stacks": ["iis", "caddy", "traefik"],
              "azure": { "region": "eastus", "os": "windows" } }
            """));
        Assert.Empty(errors);
    }

    [Fact]
    public void lan_endpoint_without_os_defaults_to_linux()
    {
        // lan upgrades carry no os key — they are Linux hosts; nginx is fine,
        // iis is not.
        var ok = DeployConfigPreflight.Validate(Config("""
            { "provider": "lan", "lan": { "ip": "10.0.0.5" }, "http_stacks": ["nginx"] }
            """));
        Assert.Empty(ok);

        var bad = DeployConfigPreflight.Validate(Config("""
            { "provider": "lan", "lan": { "ip": "10.0.0.5" }, "http_stacks": ["iis"] }
            """));
        Assert.Contains(bad, e => e.Contains("IIS requires Windows"));
    }

    [Fact]
    public void missing_or_malformed_sections_produce_no_errors()
    {
        // The pre-flight is a mirror of install.sh's OS rules, not a schema
        // validator — everything else stays install.sh's job.
        Assert.Empty(DeployConfigPreflight.Validate(JsonNode.Parse("""{ "version": 1 }""")!));
        Assert.Empty(DeployConfigPreflight.Validate(Config("""{ "provider": "azure" }""")));
        Assert.Empty(DeployConfigPreflight.Validate(null));
    }
}
