using Microsoft.Extensions.Caching.Memory;
using Networker.ControlPlane.Sso;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// The exchange-code cache is the seam between the SSO callback (which mints a
/// code) and POST /auth/sso/exchange (which redeems it for a JWT). These tests
/// pin the Rust-matched semantics: single use, 2-minute lifetime, burn-then-
/// check-expiry ordering.
/// </summary>
public sealed class SsoExchangeCodeCacheTests
{
    private sealed class FakeTime : TimeProvider
    {
        public DateTimeOffset Now { get; set; } = new(2026, 7, 14, 12, 0, 0, TimeSpan.Zero);
        public override DateTimeOffset GetUtcNow() => Now;
    }

    private static (SsoExchangeCodeCache Cache, FakeTime Time) Create()
    {
        var time = new FakeTime();
        var cache = new SsoExchangeCodeCache(new MemoryCache(new MemoryCacheOptions()), time);
        return (cache, time);
    }

    [Fact]
    public void Issue_ProducesUrlSafeCodeOfRustLength()
    {
        var (cache, _) = Create();
        var code = cache.Issue(Guid.NewGuid(), "a@b.com", "viewer");

        Assert.Equal(SsoExchangeCodeCache.CodeLength, code.Length); // 48, like Rust
        Assert.All(code, c => Assert.True(char.IsAsciiLetterOrDigit(c)));
    }

    [Fact]
    public void Redeem_ReturnsEntryOnce_ThenNull()
    {
        var (cache, _) = Create();
        var userId = Guid.NewGuid();
        var code = cache.Issue(userId, "user@example.com", "operator");

        var entry = cache.Redeem(code);
        Assert.NotNull(entry);
        Assert.Equal(userId, entry.UserId);
        Assert.Equal("user@example.com", entry.Email);
        Assert.Equal("operator", entry.Role);

        // Single use: a second redemption of the same code must fail.
        Assert.Null(cache.Redeem(code));
    }

    [Fact]
    public void Redeem_UnknownOrEmptyCode_ReturnsNull()
    {
        var (cache, _) = Create();
        Assert.Null(cache.Redeem("never-issued"));
        Assert.Null(cache.Redeem(string.Empty));
    }

    [Fact]
    public void Redeem_AfterLifetime_ReturnsNull()
    {
        var (cache, time) = Create();
        var code = cache.Issue(Guid.NewGuid(), "a@b.com", "viewer");

        time.Now += SsoExchangeCodeCache.CodeLifetime + TimeSpan.FromSeconds(1);

        Assert.Null(cache.Redeem(code));
    }

    [Fact]
    public void Redeem_JustBeforeLifetime_StillValid()
    {
        var (cache, time) = Create();
        var code = cache.Issue(Guid.NewGuid(), "a@b.com", "viewer");

        time.Now += SsoExchangeCodeCache.CodeLifetime - TimeSpan.FromSeconds(1);

        Assert.NotNull(cache.Redeem(code));
    }

    [Fact]
    public void ExpiredCode_IsBurnedByTheFailedRedeem()
    {
        // Rust removes the entry before checking expiry — an expired code is
        // consumed by the attempt and can't be probed repeatedly.
        var (cache, time) = Create();
        var code = cache.Issue(Guid.NewGuid(), "a@b.com", "viewer");

        time.Now += SsoExchangeCodeCache.CodeLifetime + TimeSpan.FromSeconds(1);
        Assert.Null(cache.Redeem(code));

        // Even if time rolled back (clock skew), the code is gone.
        time.Now -= TimeSpan.FromMinutes(10);
        Assert.Null(cache.Redeem(code));
    }

    [Fact]
    public void Issue_ProducesUniqueCodes()
    {
        var (cache, _) = Create();
        var codes = Enumerable.Range(0, 100)
            .Select(_ => cache.Issue(Guid.NewGuid(), "a@b.com", "viewer"))
            .ToHashSet();
        Assert.Equal(100, codes.Count);
    }
}
