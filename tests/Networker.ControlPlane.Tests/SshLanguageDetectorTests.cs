using Networker.ControlPlane.Provisioning;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Tests for the pure parts of the SSH language probe ported from the Rust
/// dashboard's <c>ssh_detect_languages</c> (api/benchmark_catalog.rs) — the
/// probe command table, the ssh argument vector, and the csharp-net* sweep
/// output parsing. The Process/ssh execution itself is exercised only against
/// live VMs (deploy E2E), matching how the Rust original was verified.
/// </summary>
public sealed class SshLanguageDetectorTests
{
    [Fact]
    public void LanguageChecks_match_rust_probe_table_exactly()
    {
        // Byte-for-byte the Rust `checks` vec — order included (results are
        // persisted in detection order).
        var expected = new (string Language, string Command)[]
        {
            ("rust", "test -f /opt/bench/rust-server"),
            ("go", "test -f /opt/bench/go-server"),
            ("cpp", "test -f /opt/bench/cpp-build/server"),
            ("nodejs", "test -f /opt/bench/nodejs/server.js"),
            ("python", "test -f /opt/bench/python/server.py"),
            ("ruby", "test -f /opt/bench/ruby/config.ru"),
            ("php", "test -f /opt/bench/php/server.php"),
            ("java", "test -f /opt/bench/java/server.jar"),
            ("nginx", "which nginx > /dev/null 2>&1"),
        };

        Assert.Equal(expected, SshLanguageDetector.LanguageChecks);
    }

    [Fact]
    public void CsharpProbe_matches_rust_sweep_command()
    {
        Assert.Equal(
            "ls -d /opt/bench/csharp-net* 2>/dev/null | sed 's|/opt/bench/||'",
            SshLanguageDetector.CsharpProbeCommand);
    }

    [Fact]
    public void BuildSshArgs_uses_batchmode_key_auth_and_rust_options()
    {
        var args = SshLanguageDetector.BuildSshArgs("azureuser", "10.1.2.3", "test -f /opt/bench/rust-server");

        string[] expected =
        [
            "-o", "StrictHostKeyChecking=no",
            "-o", "ConnectTimeout=10",
            "-o", "BatchMode=yes",
            "azureuser@10.1.2.3",
            "test -f /opt/bench/rust-server",
        ];
        Assert.Equal(expected, args);
    }

    [Fact]
    public void ParseCsharpVariants_keeps_trimmed_csharp_net_lines()
    {
        var stdout = "csharp-net8\n  csharp-net9-aot  \n\ncsharp-net48\n";

        string[] expected = ["csharp-net8", "csharp-net9-aot", "csharp-net48"];
        Assert.Equal(expected, SshLanguageDetector.ParseCsharpVariants(stdout));
    }

    [Theory]
    [InlineData("")]                       // empty sweep (no csharp installs)
    [InlineData("\n\n")]                   // blank lines only
    [InlineData("ls: cannot access\n")]    // stray non-matching output
    [InlineData("net8\ncsharp\n")]         // lines not starting with csharp-net
    public void ParseCsharpVariants_ignores_non_matching_output(string stdout)
    {
        Assert.Empty(SshLanguageDetector.ParseCsharpVariants(stdout));
    }
}
