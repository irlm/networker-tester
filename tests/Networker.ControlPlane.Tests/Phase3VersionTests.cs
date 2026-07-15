using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pure-logic parity for the <c>GET /api/version</c> port — the semver
/// comparison that drives <c>update_available</c>. Mirrors the Rust dashboard's
/// <c>api/version.rs</c> <c>version_newer</c> unit tests one-for-one.
/// </summary>
public class Phase3VersionTests
{
    [Theory]
    // newer_patch_detected / same / older_patch
    [InlineData("0.13.37", "0.13.36", true)]
    [InlineData("0.13.36", "0.13.36", false)]
    [InlineData("0.13.35", "0.13.36", false)]
    // minor / major
    [InlineData("0.14.0", "0.13.99", true)]
    [InlineData("1.0.0", "0.99.99", true)]
    // two-part versions
    [InlineData("1.1", "1.0", true)]
    [InlineData("1.0", "1.1", false)]
    // missing patch treated as zero
    [InlineData("1.0", "1.0.0", false)]
    [InlineData("1.0.0", "1.0", false)]
    // empty strings handled safely
    [InlineData("", "", false)]
    [InlineData("", "1.0.0", false)]
    [InlineData("1.0.0", "", true)]
    // non-numeric segments ignored (parse skips them)
    [InlineData("1.0.beta", "1.0.1", false)]
    // caller strips 'v'; if not, the prefixed segment fails to parse gracefully
    [InlineData("v1.0.0", "0.9.0", false)]
    public void VersionNewer_MatchesRustSemantics(string a, string b, bool expected)
    {
        Assert.Equal(expected, VersionEndpoints.VersionNewer(a, b));
    }
}
