using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// Unit tests for the share-link/invite expiry rules — the pure guards behind
/// POST/PUT share-links (expires_in_days window) and the env-var parsing that
/// mirrors the Rust config.rs defaults (invite 7 days, share cap 365).
public sealed class CollabExpiryRulesTests
{
    [Theory]
    [InlineData(0, 365, false)]  // Rust: expires_in_days == 0 → reject
    [InlineData(1, 365, true)]   // lower bound inclusive
    [InlineData(30, 365, true)]  // the PUT-extend default
    [InlineData(365, 365, true)] // upper bound inclusive
    [InlineData(366, 365, false)] // Rust: > share_max_days → reject
    [InlineData(-5, 365, false)]
    [InlineData(2, 1, false)]    // respects a lowered cap
    public void Share_expiry_window(int days, int max, bool expected)
    {
        Assert.Equal(expected, ShareLinkRules.IsValidExpiryDays(days, max));
    }

    [Theory]
    [InlineData(null, 7, 7)]      // unset → default (invite)
    [InlineData("", 7, 7)]        // empty → default
    [InlineData("banana", 365, 365)] // non-numeric → default (Rust parse().ok())
    [InlineData("14", 7, 14)]     // valid override wins
    [InlineData("0", 7, 7)]       // non-positive → default
    [InlineData("-3", 365, 365)]  // negative → default
    public void Env_days_parsing(string? raw, int fallback, int expected)
    {
        Assert.Equal(expected, CollabConfig.ParseDays(raw, fallback));
    }

    [Fact]
    public void Defaults_match_rust_config()
    {
        // config.rs: invite_expiry_days default 7, share_max_days default 365.
        Assert.Equal(7, CollabConfig.DefaultInviteExpiryDays);
        Assert.Equal(365, CollabConfig.DefaultShareMaxDays);
    }
}
