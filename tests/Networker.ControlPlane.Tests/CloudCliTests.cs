using Networker.ControlPlane.Provisioning;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Fidelity-audit F3/F12 regression tests: cloud-CLI binary resolution
/// (AZ_CMD / AWS_CMD / GCLOUD_CMD overrides), the diagnosable launch-failure
/// message, and home-directory resolution that survives a systemd unit
/// without <c>$HOME</c> (the old <c>?? ""</c> pattern silently probed the
/// relative path <c>.ssh/id_rsa.pub</c> under cwd <c>/</c>).
/// </summary>
public class CloudCliTests
{
    // ── binary resolution ───────────────────────────────────────────────

    [Theory]
    [InlineData("az", "AZ_CMD")]
    [InlineData("aws", "AWS_CMD")]
    [InlineData("gcloud", "GCLOUD_CMD")]
    public void Resolve_returns_the_default_when_the_override_is_unset_or_empty(string bin, string var)
    {
        Assert.Equal(bin, CloudCli.Resolve(bin, var, _ => null));
        Assert.Equal(bin, CloudCli.Resolve(bin, var, _ => string.Empty));
    }

    [Fact]
    public void Resolve_honours_the_override_env_var()
    {
        var resolved = CloudCli.Resolve("gcloud", "GCLOUD_CMD",
            v => v == "GCLOUD_CMD" ? "/snap/bin/gcloud" : null);
        Assert.Equal("/snap/bin/gcloud", resolved);
    }

    [Theory]
    [InlineData("az", "AZ_CMD")]
    [InlineData("aws", "AWS_CMD")]
    [InlineData("gcloud", "GCLOUD_CMD")]
    [InlineData("/snap/bin/gcloud", "GCLOUD_CMD")] // absolute override still maps
    [InlineData("/usr/local/bin/aws", "AWS_CMD")]
    [InlineData("az.exe", "AZ_CMD")] // Windows-style name
    public void OverrideVarFor_maps_each_cloud_cli(string file, string expectedVar) =>
        Assert.Equal(expectedVar, CloudCli.OverrideVarFor(file));

    [Fact]
    public void OverrideVarFor_returns_null_for_binaries_it_does_not_own() =>
        Assert.Null(CloudCli.OverrideVarFor("bash"));

    // ── launch-failure message (audit F12: no silent soft-fail) ─────────

    [Fact]
    public void LaunchFailureMessage_names_the_binary_and_its_override_var()
    {
        var msg = CloudCli.LaunchFailureMessage("gcloud", "No such file or directory");

        Assert.Contains("gcloud", msg);
        Assert.Contains("No such file or directory", msg);
        Assert.Contains("GCLOUD_CMD", msg);
        Assert.Contains("/snap/bin", msg); // the systemd-PATH hint
    }

    [Fact]
    public void LaunchFailureMessage_for_an_unowned_binary_still_names_it()
    {
        var msg = CloudCli.LaunchFailureMessage("bash", "boom");

        Assert.Contains("bash", msg);
        Assert.Contains("boom", msg);
        Assert.DoesNotContain("_CMD", msg); // no bogus override hint
    }

    // ── home resolution (audit F3) ──────────────────────────────────────

    [Fact]
    public void HomeDirectory_prefers_the_HOME_env_var() =>
        Assert.Equal("/home/azureuser",
            CloudCli.HomeDirectory(v => v == "HOME" ? "/home/azureuser" : null, () => "/wrong"));

    [Fact]
    public void HomeDirectory_falls_back_to_the_passwd_backed_profile()
    {
        // The systemd case: no $HOME in the unit's environment.
        Assert.Equal("/home/svc",
            CloudCli.HomeDirectory(_ => null, () => "/home/svc"));
        // Whitespace HOME is as good as unset.
        Assert.Equal("/home/svc",
            CloudCli.HomeDirectory(v => v == "HOME" ? "  " : null, () => "/home/svc"));
    }

    [Fact]
    public void HomeDirectory_never_degrades_to_a_relative_path_on_unix()
    {
        // Both sources empty — the old `?? ""` bug made Path.Combine produce
        // the RELATIVE ".ssh/id_rsa.pub" (cwd "/" under systemd).
        var home = CloudCli.HomeDirectory(_ => null, () => null);

        if (OperatingSystem.IsWindows())
        {
            Assert.Equal(string.Empty, home);
        }
        else
        {
            Assert.Equal("/root", home);
            Assert.True(Path.IsPathRooted(Path.Combine(home, ".ssh", "id_rsa.pub")));
        }
    }

    [Fact]
    public void Default_bin_resolvers_return_a_launchable_name()
    {
        // Ambient-env smoke test: whatever the overrides say, the resolvers
        // never return null/empty (RunAsync would throw on an empty FileName).
        Assert.False(string.IsNullOrWhiteSpace(CloudCli.AzBin()));
        Assert.False(string.IsNullOrWhiteSpace(CloudCli.AwsBin()));
        Assert.False(string.IsNullOrWhiteSpace(CloudCli.GcloudBin()));
    }
}
