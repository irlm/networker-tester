using Networker.ControlPlane.Provisioning;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Tests for the cloud-init bootstrap generation ported from Rust
/// <c>cloud_init.rs</c> — input validation, <c>agent_ws_url</c>, and placeholder
/// substitution into the verbatim templates.
/// </summary>
public sealed class CloudInitScriptsTests
{
    private const string GoodUrl = "https://alethedash.com";
    private const string GoodKey = "abcdefghijklmnopqrstuvwxyz012345"; // 32 alnum
    private const string GoodTriple = "x86_64-unknown-linux-musl";

    [Theory]
    [InlineData("https://alethedash.com", "wss://alethedash.com/ws/agent")]
    [InlineData("http://localhost:3000", "ws://localhost:3000/ws/agent")]
    [InlineData("https://alethedash.com/api", "wss://alethedash.com/ws/agent")]
    [InlineData("https://alethedash.com/", "wss://alethedash.com/ws/agent")]
    public void AgentWsUrl_maps_scheme_and_drops_path(string input, string expected)
    {
        Assert.Equal(expected, CloudInitScripts.AgentWsUrl(input));
    }

    [Fact]
    public void ValidateInputs_rejects_bad_url()
    {
        var ex = Assert.Throws<ArgumentException>(
            () => CloudInitScripts.ValidateInputs("ftp://x", GoodKey, GoodTriple));
        Assert.Contains("invalid dashboard_url", ex.Message);
    }

    [Fact]
    public void ValidateInputs_rejects_short_key()
    {
        var ex = Assert.Throws<ArgumentException>(
            () => CloudInitScripts.ValidateInputs(GoodUrl, "tooshort", GoodTriple));
        Assert.Contains("invalid api_key", ex.Message);
    }

    [Fact]
    public void ValidateInputs_rejects_bad_triple()
    {
        var ex = Assert.Throws<ArgumentException>(
            () => CloudInitScripts.ValidateInputs(GoodUrl, GoodKey, "bad triple!"));
        Assert.Contains("invalid target_triple", ex.Message);
    }

    [Fact]
    public void RenderLinux_substitutes_all_placeholders()
    {
        var script = CloudInitScripts.RenderLinuxBootstrap(GoodUrl, GoodKey, GoodTriple);

        Assert.DoesNotContain("__TARGET_TRIPLE__", script);
        Assert.DoesNotContain("__DASHBOARD_URL__", script);
        Assert.DoesNotContain("__API_KEY__", script);
        Assert.Contains($"Environment=AGENT_DASHBOARD_URL={GoodUrl}", script);
        Assert.Contains($"Environment=AGENT_API_KEY={GoodKey}", script);
        // System-level unit (distinct from the user-level tester_install unit).
        Assert.Contains("WantedBy=multi-user.target", script);
        Assert.Contains("/etc/systemd/system/networker-agent.service", script);
        // Downloads the right asset for the target.
        Assert.Contains($"networker-tester-{GoodTriple}.tar.gz", script.Replace("${TARGET}", GoodTriple));
    }

    [Fact]
    public void RenderWindows_is_ascii_only_and_substitutes()
    {
        var script = CloudInitScripts.RenderWindowsBootstrap(GoodUrl, GoodKey, GoodTriple);

        Assert.DoesNotContain("__TARGET_TRIPLE__", script);
        Assert.All(script, c => Assert.True(c < 128, $"non-ASCII char U+{(int)c:X4}"));
        Assert.Contains("sc.exe create NetworkerAgent", script);
        Assert.Contains($"'{GoodUrl}'", script);
    }
}
