using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Ports the Rust <c>api/bench_tokens.rs</c> unit tests: token-name parsing and
/// the per-user filtering rule.
/// </summary>
public class Phase3BenchTokensTests
{
    [Fact]
    public void ParseValidTokenName()
    {
        var parsed = BenchTokensEndpoints.ParseTokenName("bench-abc123-vm-east-us-1");
        Assert.NotNull(parsed);
        Assert.Equal("abc123", parsed!.Value.ConfigId);
        Assert.Equal("east-us-1", parsed.Value.TestbedId);
    }

    [Fact]
    public void ParseTokenNameWithHyphens()
    {
        var parsed = BenchTokensEndpoints.ParseTokenName("bench-my-config-vm-my-testbed");
        Assert.NotNull(parsed);
        Assert.Equal("my-config", parsed!.Value.ConfigId);
        Assert.Equal("my-testbed", parsed.Value.TestbedId);
    }

    [Fact]
    public void ParseInvalidPrefix()
    {
        Assert.Null(BenchTokensEndpoints.ParseTokenName("notbench-abc-vm-def"));
    }

    [Fact]
    public void ParseMissingVmSeparator()
    {
        Assert.Null(BenchTokensEndpoints.ParseTokenName("bench-abc-def"));
    }

    [Fact]
    public void ParseEmptyParts()
    {
        Assert.Null(BenchTokensEndpoints.ParseTokenName("bench--vm-def"));
        Assert.Null(BenchTokensEndpoints.ParseTokenName("bench-abc-vm-"));
    }

    [Fact]
    public void AdminSeesAllTokens()
    {
        var admin = new AuthUser(Guid.NewGuid(), "admin@x.com", "admin", IsPlatformAdmin: true);
        var tokens = new List<BenchTokensEndpoints.TokenInfo>
        {
            new() { name = "a", user = "someone@else.com" },
            new() { name = "b", user = "other" },
        };
        Assert.Equal(2, BenchTokensEndpoints.FilterTokensForUser(tokens, admin).Count);
    }

    [Fact]
    public void NonAdminSeesOnlyOwnTokensByEmailOrId()
    {
        var uid = Guid.NewGuid();
        var user = new AuthUser(uid, "me@x.com", "viewer", IsPlatformAdmin: false);
        var tokens = new List<BenchTokensEndpoints.TokenInfo>
        {
            new() { name = "by-email", user = "me@x.com" },
            new() { name = "by-id", user = uid.ToString() },
            new() { name = "not-mine", user = "someone@else.com" },
        };
        var filtered = BenchTokensEndpoints.FilterTokensForUser(tokens, user);
        Assert.Equal(2, filtered.Count);
        Assert.Contains(filtered, t => t.name == "by-email");
        Assert.Contains(filtered, t => t.name == "by-id");
    }
}
