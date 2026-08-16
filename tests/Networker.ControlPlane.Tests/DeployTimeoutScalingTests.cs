using Networker.ControlPlane.Provisioning;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pins the workload-scaled install.sh budget (v0.28.208): a 3-stack +
/// 8-language Windows deploy was killed by the flat 30m ceiling mid-install
/// (field, 2026-08-16). Base 30m + 8m per requested language, capped at 2h;
/// unparseable configs get the base (install.sh validation rejects them).
/// </summary>
public class DeployTimeoutScalingTests
{
    [Fact]
    public void stack_only_deploy_keeps_the_base_budget()
    {
        var t = DeployRunner.DeployTimeoutFor("""
            { "version": 1, "endpoints": [ { "provider": "azure", "http_stacks": ["iis", "caddy"] } ] }
            """);
        Assert.Equal(TimeSpan.FromMinutes(30), t);
    }

    [Fact]
    public void each_language_adds_budget()
    {
        var t = DeployRunner.DeployTimeoutFor("""
            { "endpoints": [ { "languages": ["csharp-net48", "go", "nodejs"] } ] }
            """);
        Assert.Equal(TimeSpan.FromMinutes(30 + 3 * 8), t);
    }

    [Fact]
    public void the_field_incident_config_now_fits()
    {
        // 8 languages → 30 + 64 = 94m (the deploy died at ~20 steps in 30m).
        var t = DeployRunner.DeployTimeoutFor("""
            { "endpoints": [ { "languages": ["go","csharp-net48","csharp-net8","csharp-net9","csharp-net10","java","nodejs","python"] } ] }
            """);
        Assert.Equal(TimeSpan.FromMinutes(94), t);
    }

    [Fact]
    public void budget_is_capped_and_multi_endpoint_languages_sum()
    {
        var t = DeployRunner.DeployTimeoutFor("""
            { "endpoints": [
                { "languages": ["a","b","c","d","e","f","g","h"] },
                { "languages": ["i","j","k","l","m","n","o","p"] }
            ] }
            """);
        Assert.Equal(TimeSpan.FromMinutes(120), t);
    }

    [Theory]
    [InlineData("not json at all")]
    [InlineData("{}")]
    [InlineData("""{ "endpoints": "wrong-shape" }""")]
    public void malformed_configs_get_the_base(string json)
    {
        Assert.Equal(TimeSpan.FromMinutes(30), DeployRunner.DeployTimeoutFor(json));
    }
}
