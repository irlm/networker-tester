using Networker.ControlPlane.Auth;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// The human-login brute-force throttle (websec audit 2026-07, P1-2). A separate
/// per-IP bucket from the agent-key limiter, so an agent-key flood and a
/// login-password flood from the same IP don't cross-penalise. Mirrors the
/// <c>AgentAuthLimiterTests</c> contract since the mechanics are reused by
/// composition.
/// </summary>
public sealed class LoginRateLimiterTests
{
    [Fact]
    public void Ip_is_not_blocked_until_it_exceeds_the_failure_cap()
    {
        var limiter = new LoginRateLimiter(maxFailures: 3, window: TimeSpan.FromMinutes(15));
        const string ip = "203.0.113.20";

        limiter.RecordFailure(ip);
        limiter.RecordFailure(ip);
        Assert.False(limiter.IsBlocked(ip)); // 2 < 3

        limiter.RecordFailure(ip);
        Assert.True(limiter.IsBlocked(ip)); // 3 >= 3
    }

    [Fact]
    public void Success_clears_an_ip_failure_history()
    {
        var limiter = new LoginRateLimiter(maxFailures: 2, window: TimeSpan.FromMinutes(15));
        const string ip = "203.0.113.21";

        limiter.RecordFailure(ip);
        limiter.RecordFailure(ip);
        Assert.True(limiter.IsBlocked(ip));

        // A user who finally types the right password is not held back.
        limiter.RecordSuccess(ip);
        Assert.False(limiter.IsBlocked(ip));
    }

    [Fact]
    public void Blocking_is_isolated_per_ip()
    {
        var limiter = new LoginRateLimiter(maxFailures: 2, window: TimeSpan.FromMinutes(15));

        limiter.RecordFailure("198.51.100.20");
        limiter.RecordFailure("198.51.100.20");
        Assert.True(limiter.IsBlocked("198.51.100.20"));
        Assert.False(limiter.IsBlocked("198.51.100.21"));
    }

    [Fact]
    public void Null_or_empty_ip_is_never_blocked()
    {
        var limiter = new LoginRateLimiter(maxFailures: 1, window: TimeSpan.FromMinutes(15));
        limiter.RecordFailure(null);
        limiter.RecordFailure("");
        Assert.False(limiter.IsBlocked(null));
        Assert.False(limiter.IsBlocked(""));
    }

    [Fact]
    public void Default_cap_is_ten()
    {
        Assert.Equal(10, LoginRateLimiter.DefaultMaxFailures);
        Assert.Equal(10, new LoginRateLimiter().MaxFailures);
    }
}
