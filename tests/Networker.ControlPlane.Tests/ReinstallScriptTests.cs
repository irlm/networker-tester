using Networker.ControlPlane.Provisioning;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// The tester in-place upgrade reinstall script (run over Azure
/// <c>vm run-command</c>). The endpoint was an honest 501 (F23) until
/// 2026-07-31. Azure latin-1-encodes run-command payloads, so a stray
/// non-ASCII byte aborts the whole invocation (the v0.28.26 em-dash incident) —
/// the ASCII invariant is load-bearing and pinned here.
/// </summary>
public class ReinstallScriptTests
{
    [Fact]
    public void Script_is_ascii_only()
    {
        var script = TesterInstallScripts.ReinstallScript("v0.28.118", "x86_64-unknown-linux-musl");
        Assert.True(TesterInstallScripts.IsAsciiOnly(script), "reinstall script must be pure ASCII for Azure run-command");
    }

    [Fact]
    public void Script_installs_both_binaries_at_the_tag_and_restarts_the_agent()
    {
        var script = TesterInstallScripts.ReinstallScript("v0.28.118", "x86_64-unknown-linux-musl");

        Assert.Contains("TAG=v0.28.118", script);
        Assert.Contains("TARGET=x86_64-unknown-linux-musl", script);
        Assert.Contains("networker-tester-${TARGET}.tar.gz", script);
        // C# agent asset first, Rust-agent (${TARGET}) fallback.
        Assert.Contains("networker-agent-cs-linux-x64.tar.gz", script);
        Assert.Contains("networker-agent-${TARGET}.tar.gz", script);
        Assert.Contains("install -m 0755 networker-tester /usr/local/bin/networker-tester", script);
        Assert.Contains("systemctl restart networker-agent", script);
        // Carries the v0.28.118 ping sysctl so a re-imaged runner gets it too.
        Assert.Contains("ping_group_range = 0 2147483647", script);
    }

    [Fact]
    public void IsAsciiOnly_rejects_a_non_ascii_char()
    {
        Assert.False(TesterInstallScripts.IsAsciiOnly("echo — dash")); // em dash
        Assert.True(TesterInstallScripts.IsAsciiOnly("echo - plain ascii ~"));
    }

    [Theory]
    [InlineData("x86_64", "x86_64-unknown-linux-musl")]
    [InlineData("aarch64", "aarch64-unknown-linux-musl")]
    [InlineData("weird", "x86_64-unknown-linux-musl")] // unknown → x86_64 default
    public void Release_target_maps_arch(string arch, string expected)
        => Assert.Equal(expected, TesterInstallScripts.ReleaseTarget(arch));
}
