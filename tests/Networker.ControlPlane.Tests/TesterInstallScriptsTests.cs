using Networker.ControlPlane.Provisioning;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Tests for the SSH-install command generation ported from Rust
/// <c>tester_install.rs</c> — the inverted SKIP_CHROME semantics, SSH-wait
/// clamp, OS-target/label mapping, systemd unit (user-level), and the download
/// command.
/// </summary>
public sealed class TesterInstallScriptsTests
{
    [Theory]
    [InlineData(null, true)]     // unset => install
    [InlineData("0", true)]      // "0" => install
    [InlineData("false", true)]  // "false" => install
    [InlineData("FALSE", true)]  // case-insensitive
    [InlineData("1", false)]     // any other truthy => skip
    [InlineData("yes", false)]
    public void ShouldInstallChrome_inverted_semantics(string? env, bool expected)
    {
        Assert.Equal(expected, TesterInstallScripts.ShouldInstallChrome(env));
    }

    [Theory]
    [InlineData(null, 300u)]
    [InlineData("500", 500u)]
    [InlineData("10", 60u)]    // clamp low
    [InlineData("5000", 900u)] // clamp high
    [InlineData("garbage", 300u)]
    public void SshWaitSecs_parses_and_clamps(string? env, uint expected)
    {
        Assert.Equal(expected, TesterInstallScripts.SshWaitSecs(env));
    }

    [Theory]
    [InlineData("x86_64", "x86_64-unknown-linux-musl")]
    [InlineData("aarch64", "aarch64-unknown-linux-musl")]
    [InlineData("weird", "x86_64-unknown-linux-musl")]
    public void ReleaseTarget_maps_arch(string arch, string expected)
    {
        Assert.Equal(expected, TesterInstallScripts.ReleaseTarget(arch));
    }

    [Fact]
    public void OsLabel_formats_pretty()
    {
        Assert.Equal("Ubuntu 24.04 Server (x86_64)",
            TesterInstallScripts.OsLabel("ubuntu", "24.04", "server", "x86_64"));
        Assert.Equal("Amazon Linux 2023 (aarch64)",
            TesterInstallScripts.OsLabel("amzn", "2023", "minimal", "aarch64"));
    }

    [Theory]
    [InlineData("ubuntu", "apt")]
    [InlineData("debian", "apt")]
    [InlineData("amzn", "dnf")]
    [InlineData("rhel", "dnf")]
    [InlineData("unknown", "apt")]
    public void PackageManager_selects(string distro, string expected)
    {
        Assert.Equal(expected, TesterInstallScripts.PackageManager(distro));
    }

    [Fact]
    public void SystemdUnit_is_user_level_and_emits_env_when_configured()
    {
        var unit = TesterInstallScripts.BuildSystemdUnit("mykey123", "wss://host/ws/agent");
        // User-level unit (distinct from cloud_init's system-level one).
        Assert.Contains("WantedBy=default.target", unit);
        Assert.Contains("Environment=AGENT_API_KEY=mykey123", unit);
        Assert.Contains("Environment=AGENT_DASHBOARD_URL=wss://host/ws/agent", unit);
    }

    [Fact]
    public void SystemdUnit_omits_env_when_unconfigured()
    {
        var unit = TesterInstallScripts.BuildSystemdUnit(null, null);
        Assert.DoesNotContain("AGENT_API_KEY", unit);
        Assert.Contains("Environment=RUST_LOG=info", unit);
    }

    [Fact]
    public void SystemdUnit_rejects_unsafe_env_values()
    {
        Assert.Throws<ArgumentException>(
            () => TesterInstallScripts.BuildSystemdUnit("key with spaces", "wss://host/ws/agent"));
    }

    [Fact]
    public void StartCommands_differ_by_config()
    {
        Assert.Contains("enable --now",
            TesterInstallScripts.SystemdStartCommands("k", "u"));
        Assert.DoesNotContain("enable --now",
            TesterInstallScripts.SystemdStartCommands(null, null));
    }

    [Fact]
    public void DownloadBinary_builds_release_url()
    {
        var cmd = TesterInstallScripts.DownloadBinaryCommand(
            "networker-agent", "v0.28.13", "x86_64-unknown-linux-musl");
        Assert.Contains(
            "https://github.com/irlm/networker-tester/releases/download/v0.28.13/networker-agent-x86_64-unknown-linux-musl.tar.gz",
            cmd);
        Assert.Contains("sudo install -m 0755 /tmp/networker-agent /usr/local/bin/networker-agent", cmd);
    }

    [Fact]
    public void IsSafeAgentEnvValue_allows_url_chars_rejects_meta()
    {
        Assert.True(TesterInstallScripts.IsSafeAgentEnvValue("wss://host:3000/ws/agent"));
        Assert.True(TesterInstallScripts.IsSafeAgentEnvValue("key-1_2.3"));
        Assert.False(TesterInstallScripts.IsSafeAgentEnvValue("has space"));
        Assert.False(TesterInstallScripts.IsSafeAgentEnvValue("bad;rm"));
    }
}
