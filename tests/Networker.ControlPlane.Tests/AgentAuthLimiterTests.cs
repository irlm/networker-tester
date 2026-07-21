using Networker.ControlPlane.Realtime.RawWs;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// V044 brute-force mitigation: the per-IP failed-auth limiter that both agent
/// auth transports consult before the WS upgrade. Pins the block threshold,
/// per-IP isolation, success-clears-history semantics, and the null-IP escape.
/// </summary>
public sealed class AgentAuthLimiterTests
{
    [Fact]
    public void Ip_is_not_blocked_until_it_exceeds_the_failure_cap()
    {
        var limiter = new AgentAuthLimiter(maxFailures: 3, window: TimeSpan.FromMinutes(5));
        const string ip = "203.0.113.10";

        limiter.RecordFailure(ip);
        limiter.RecordFailure(ip);
        Assert.False(limiter.IsBlocked(ip)); // 2 < 3

        limiter.RecordFailure(ip);
        Assert.True(limiter.IsBlocked(ip)); // 3 >= 3
    }

    [Fact]
    public void Blocking_is_isolated_per_ip()
    {
        var limiter = new AgentAuthLimiter(maxFailures: 2, window: TimeSpan.FromMinutes(5));

        limiter.RecordFailure("198.51.100.1");
        limiter.RecordFailure("198.51.100.1");
        Assert.True(limiter.IsBlocked("198.51.100.1"));

        // A different IP is unaffected by the first IP's failures.
        Assert.False(limiter.IsBlocked("198.51.100.2"));
    }

    [Fact]
    public void Success_clears_an_ip_failure_history()
    {
        var limiter = new AgentAuthLimiter(maxFailures: 2, window: TimeSpan.FromMinutes(5));
        const string ip = "203.0.113.11";

        limiter.RecordFailure(ip);
        limiter.RecordFailure(ip);
        Assert.True(limiter.IsBlocked(ip));

        // A legitimate agent that finally presents a good key is not held back.
        limiter.RecordSuccess(ip);
        Assert.False(limiter.IsBlocked(ip));
    }

    [Fact]
    public void Failures_outside_the_window_do_not_count()
    {
        // A 0-length window means every recorded failure is already "expired"
        // by the time it is counted, so an IP is never blocked.
        var limiter = new AgentAuthLimiter(maxFailures: 1, window: TimeSpan.Zero);
        limiter.RecordFailure("203.0.113.12");
        Assert.False(limiter.IsBlocked("203.0.113.12"));
    }

    [Fact]
    public void Null_or_empty_ip_is_never_blocked()
    {
        var limiter = new AgentAuthLimiter(maxFailures: 1, window: TimeSpan.FromMinutes(5));
        // Unattributable failures cannot block (the hash lookup still gates auth).
        limiter.RecordFailure(null);
        limiter.RecordFailure("");
        Assert.False(limiter.IsBlocked(null));
        Assert.False(limiter.IsBlocked(""));
    }

    [Fact]
    public void Default_cap_is_ten()
    {
        Assert.Equal(10, AgentAuthLimiter.DefaultMaxFailures);
        Assert.Equal(10, new AgentAuthLimiter().MaxFailures);
    }
}
