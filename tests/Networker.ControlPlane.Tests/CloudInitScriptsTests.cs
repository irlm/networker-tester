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

    [Theory]
    [InlineData("https://alethedash.com", "https://alethedash.com")]
    [InlineData("wss://laghound.com/ws/agent", "https://laghound.com")]
    [InlineData("http://localhost:3000", "http://localhost:3000")]
    [InlineData("ws://localhost:5030/ws/agent", "http://localhost:5030")]
    [InlineData("https://alethedash.com/api", "https://alethedash.com")]
    public void ArtifactBase_maps_ws_to_http_and_drops_path(string input, string expected)
    {
        // One configured value drives both the agent socket and the artifact
        // fetch, so a VM can never end up with the two pointing at different
        // hosts.
        Assert.Equal(expected, CloudInitScripts.ArtifactBaseFrom(input));
    }

    [Fact]
    public void Linux_bootstrap_pulls_artifacts_from_the_control_plane_not_github()
    {
        // The repo is private: releases/download/ 404s for every caller, and a
        // GitHub token must NOT be planted in user-data (readable on the VM and
        // through the instance metadata service).
        var script = CloudInitScripts.RenderLinuxBootstrap(GoodUrl, GoodKey, GoodTriple);

        Assert.DoesNotContain("releases/download", script);
        Assert.DoesNotContain("api.github.com/repos", script);
        // Base and path are not adjacent literals: the script holds the base
        // in a variable and interpolates the asset name at the call site.
        Assert.Contains("ARTIFACT_BASE=\"https://alethedash.com\"", script);
        Assert.Contains("/api/artifacts/", script);
        Assert.Contains("X-Agent-Key: " + GoodKey, script);
        Assert.Contains("networker-tester-" + GoodTriple + ".tar.gz", script);
        Assert.Contains("networker-agent-cs-linux-x64.tar.gz", script);
    }

    [Fact]
    public void Windows_bootstrap_pulls_artifacts_from_the_control_plane_not_github()
    {
        var script = CloudInitScripts.RenderWindowsBootstrap(
            GoodUrl, GoodKey, "x86_64-pc-windows-msvc");

        Assert.DoesNotContain("releases/download", script);
        Assert.DoesNotContain("api.github.com/repos", script);
        Assert.Contains("$ArtifactBase = 'https://alethedash.com'", script);
        Assert.Contains("/api/artifacts/", script);
        Assert.Contains("'X-Agent-Key' = '" + GoodKey + "'", script);
        Assert.Contains("networker-agent-cs-win-x64.zip", script);
    }

    [Fact]
    public void No_placeholder_survives_rendering()
    {
        // A missed __PLACEHOLDER__ would ship a VM that fetches from a literal
        // string; assert every one is substituted on both platforms.
        foreach (var script in new[]
                 {
                     CloudInitScripts.RenderLinuxBootstrap(GoodUrl, GoodKey, GoodTriple),
                     CloudInitScripts.RenderWindowsBootstrap(GoodUrl, GoodKey, "x86_64-pc-windows-msvc"),
                 })
        {
            Assert.DoesNotContain("__ARTIFACT_BASE__", script);
            Assert.DoesNotContain("__TARGET_TRIPLE__", script);
            Assert.DoesNotContain("__DASHBOARD_URL__", script);
            Assert.DoesNotContain("__API_KEY__", script);
        }
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
    public void RenderLinux_installs_the_csharp_agent_with_rust_fallback()
    {
        var script = CloudInitScripts.RenderLinuxBootstrap(GoodUrl, GoodKey, GoodTriple);

        // v0.28.26: the agent is the self-contained C# Networker.Agent asset...
        Assert.Contains("networker-agent-cs-linux-x64.tar.gz", script);
        // ...with the legacy Rust asset as the fallback for older releases.
        Assert.Contains("falling back to legacy Rust agent", script);
        Assert.Contains("download_bin networker-agent", script);
        // The tester download is unchanged (Rust tester stays).
        Assert.Contains("download_bin networker-tester", script);
        // RUST_LOG is gone from the unit (the C# agent doesn't read it).
        Assert.DoesNotContain("RUST_LOG", script);
    }

    [Fact]
    public void RenderWindows_installs_the_csharp_agent_with_rust_fallback()
    {
        var script = CloudInitScripts.RenderWindowsBootstrap(GoodUrl, GoodKey, GoodTriple);

        Assert.Contains("networker-agent-cs-win-x64.zip", script);
        Assert.Contains($"networker-agent-{GoodTriple}.zip", script.Replace("$TARGET", GoodTriple));
        Assert.Contains($"networker-tester-{GoodTriple}.zip", script.Replace("$TARGET", GoodTriple));
        Assert.DoesNotContain("RUST_LOG", script);
    }

    [Fact]
    public void RenderLinux_is_ascii_only()
    {
        // Both cloud vendors' user-data paths choke on non-ASCII: az CLI
        // latin-1-encodes --custom-data, so a single U+2014 em-dash makes
        // `az vm create` exit 1 in seconds ('latin-1' codec can't encode…).
        // This guard existed for the Windows template only — the gap let an
        // em-dash into the Linux template and broke tester provisioning on
        // prod (v0.28.26). Never remove this test.
        var script = CloudInitScripts.RenderLinuxBootstrap(GoodUrl, GoodKey, GoodTriple);
        Assert.All(script, c => Assert.True(c < 128, $"non-ASCII char U+{(int)c:X4}"));
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
