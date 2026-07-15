using Microsoft.Extensions.Logging.Abstractions;
using Networker.ControlPlane.Background;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Tests for the version-refresh semver logic — the C# port of the Rust
/// <c>version_refresh.rs</c> <c>parse_semver</c> / <c>pick_higher_semver</c>.
/// </summary>
public sealed class VersionRefreshSemverTests
{
    [Theory]
    [InlineData("0.25.0", 0u, 25u, 0u)]
    [InlineData("v0.25.0", 0u, 25u, 0u)]
    [InlineData("1.2.3", 1u, 2u, 3u)]
    [InlineData("0.25.0-rc.1", 0u, 25u, 0u)] // patch takes leading digits only
    [InlineData("10.20.30", 10u, 20u, 30u)]
    public void ParseSemver_parses_valid(string input, uint major, uint minor, uint patch)
    {
        var parsed = VersionRefreshService.ParseSemver(input);
        Assert.NotNull(parsed);
        Assert.Equal((major, minor, patch), parsed!.Value);
    }

    [Theory]
    [InlineData("0.25")]     // too few parts
    [InlineData("abc")]      // not numeric
    [InlineData("0.x.0")]    // minor not numeric
    [InlineData("0.0.rc")]   // patch has no leading digit
    public void ParseSemver_rejects_invalid(string input)
    {
        Assert.Null(VersionRefreshService.ParseSemver(input));
    }

    [Fact]
    public void PickHigher_returns_a_when_b_null()
    {
        Assert.Equal("0.24.0", VersionRefreshService.PickHigherSemver("0.24.0", null));
    }

    [Fact]
    public void PickHigher_prefers_greater_and_preserves_string()
    {
        // Remote higher: returns the b string verbatim (v prefix preserved).
        Assert.Equal("v0.25.0", VersionRefreshService.PickHigherSemver("0.24.0", "v0.25.0"));
        // Floor higher: returns a.
        Assert.Equal("0.26.0", VersionRefreshService.PickHigherSemver("0.26.0", "0.25.0"));
        // Equal: returns a (b not strictly greater).
        Assert.Equal("0.25.0", VersionRefreshService.PickHigherSemver("0.25.0", "0.25.0"));
    }

    [Fact]
    public void PickHigher_falls_back_to_parseable_side()
    {
        // Only a parses.
        Assert.Equal("0.25.0", VersionRefreshService.PickHigherSemver("0.25.0", "garbage"));
        // Only b parses.
        Assert.Equal("0.25.0", VersionRefreshService.PickHigherSemver("garbage", "0.25.0"));
        // Neither parses → a.
        Assert.Equal("x", VersionRefreshService.PickHigherSemver("x", "y"));
    }

    [Fact]
    public void Cache_seeds_with_floor()
    {
        var cache = new LatestVersionCache("0.28.13");
        Assert.Equal("0.28.13", cache.Current);
    }

    [Fact]
    public async Task RefreshNow_falls_back_to_floor_on_fetch_failure()
    {
        // No IHttpClientFactory + real HTTP to GitHub is not made deterministic in
        // unit tests; but RefreshNow must ALWAYS succeed and write a value. With a
        // bogus network the fetch throws internally and the floor is cached.
        var cache = new LatestVersionCache("0.28.13");
        var svc = new VersionRefreshService(
            cache,
            "0.28.13",
            NullLogger<VersionRefreshService>.Instance,
            new FailingHttpClientFactory());

        var resolved = await svc.RefreshNowAsync();

        // Fetch failed => floor is used and cached (Rust: always Ok, floor fallback).
        Assert.Equal("0.28.13", resolved);
        Assert.Equal("0.28.13", cache.Current);
    }

    private sealed class FailingHttpClientFactory : IHttpClientFactory
    {
        public HttpClient CreateClient(string name) =>
            new(new FailingHandler());

        private sealed class FailingHandler : HttpMessageHandler
        {
            protected override Task<HttpResponseMessage> SendAsync(
                HttpRequestMessage request, CancellationToken cancellationToken) =>
                throw new HttpRequestException("network down");
        }
    }
}
