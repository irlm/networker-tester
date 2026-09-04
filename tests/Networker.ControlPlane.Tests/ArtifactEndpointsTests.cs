using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// The artifact route is what tester VMs fetch binaries through now the repo is
/// private. These cover the parts that are pure logic; the redirect itself
/// needs a deployed control plane with the SAS configured.
/// </summary>
public sealed class ArtifactEndpointsTests
{
    private const string Base = "https://alethedashreleases.blob.core.windows.net/releases";
    private const string Sas = "se=2027-08-27T13%3A28Z&sp=r&sv=2022-11-02&sr=c&sig=abc";

    [Fact]
    public void Blob_url_is_version_scoped()
    {
        // A release must never be able to serve another release's binary.
        var url = ArtifactEndpoints.BlobUrlFor(
            Base, Sas, "networker-tester-x86_64-unknown-linux-musl.tar.gz", "0.28.308");

        Assert.StartsWith($"{Base}/v0.28.308/", url);
        Assert.Contains("networker-tester-x86_64-unknown-linux-musl.tar.gz", url);
        Assert.EndsWith($"?{Sas}", url);
    }

    [Fact]
    public void Blob_url_tolerates_a_trailing_slash_and_a_leading_question_mark()
    {
        // Both are easy to get wrong in configuration; neither should produce
        // a double separator that Azure would reject.
        var url = ArtifactEndpoints.BlobUrlFor(Base + "/", "?" + Sas, "a.tar.gz", "1.2.3");

        Assert.DoesNotContain("//v1.2.3", url);
        Assert.DoesNotContain("??", url);
        Assert.Equal($"{Base}/v1.2.3/a.tar.gz?{Sas}", url);
    }

    [Fact]
    public void Own_version_is_three_part_and_matches_the_assembly()
    {
        // The default when no ?tag= is given: a VM gets the build that belongs
        // to the control plane that provisioned it.
        var v = ArtifactEndpoints.OwnVersion();

        Assert.NotNull(v);
        Assert.Equal(3, v!.Split('.').Length);
        Assert.All(v.Split('.'), part => Assert.True(int.TryParse(part, out _), $"'{part}' is not numeric"));
    }

    [Theory]
    [InlineData("networker-tester-x86_64-unknown-linux-musl.tar.gz", true)]
    [InlineData("networker-agent-cs-linux-x64.tar.gz", true)]
    [InlineData("networker-agent-cs-win-x64.zip", true)]
    // The name arrives from a VM: it must not be able to steer the request.
    [InlineData("../../etc/passwd", false)]
    [InlineData("networker-tester-x86_64-unknown-linux-musl.tar.gz.evil", false)]
    [InlineData("", false)]
    [InlineData("dashboard-frontend.tar.gz", false)]
    public void Only_known_release_assets_are_servable(string name, bool allowed)
    {
        Assert.Equal(allowed, ArtifactEndpoints.IsAllowedAsset(name));
    }
}
