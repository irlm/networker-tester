using Microsoft.Extensions.Caching.Memory;

namespace Networker.ControlPlane.Sso;

/// <summary>
/// What an SSO exchange code redeems for — mirrors the Rust <c>SsoCodeEntry</c>
/// (email, role, user_id, expires_at) stashed in <c>AppState.sso_codes</c>.
/// </summary>
public sealed record SsoCodeEntry(Guid UserId, string Email, string Role, DateTimeOffset ExpiresAt);

/// <summary>
/// Single-use, short-lived exchange codes minted at the end of the SSO callback
/// and redeemed by <c>POST /auth/sso/exchange</c> for a session JWT — so the JWT
/// never rides in a URL query parameter. Port of the Rust
/// <c>Mutex&lt;HashMap&lt;String, SsoCodeEntry&gt;&gt;</c> on <c>AppState</c>,
/// re-based on <see cref="IMemoryCache"/> (which handles the pruning the Rust
/// code did by hand on every insert).
///
/// Semantics matched to Rust <c>sso_callback</c>/<c>sso_exchange</c>:
/// <list type="bullet">
///   <item>48-char alphanumeric code.</item>
///   <item>2-minute lifetime (<c>chrono::Duration::minutes(2)</c>).</item>
///   <item>Redeem removes the entry first, then checks expiry — a code is
///         burned on first use even if it turns out to be expired.</item>
/// </list>
///
/// Time is injected (<see cref="TimeProvider"/>) so the expiry path is
/// unit-testable without sleeping; the IMemoryCache absolute expiration acts as
/// the memory backstop.
/// </summary>
public sealed class SsoExchangeCodeCache(IMemoryCache cache, TimeProvider? time = null)
{
    public const int CodeLength = 48;
    public static readonly TimeSpan CodeLifetime = TimeSpan.FromMinutes(2);

    private const string KeyPrefix = "sso_exchange_code:";
    private readonly TimeProvider _time = time ?? TimeProvider.System;

    /// <summary>
    /// Mint a fresh single-use code for the given user and stash it. Returns the
    /// raw code (goes into the <c>/sso-complete?code=</c> redirect).
    /// </summary>
    public string Issue(Guid userId, string email, string role)
    {
        var code = AccountSecurity.GenerateAlphanumericToken(CodeLength);
        var entry = new SsoCodeEntry(userId, email, role, _time.GetUtcNow() + CodeLifetime);

        cache.Set(KeyPrefix + code, entry, new MemoryCacheEntryOptions
        {
            // Backstop eviction slightly after the logical expiry; Redeem
            // enforces the exact ExpiresAt (matching the Rust explicit check).
            AbsoluteExpirationRelativeToNow = CodeLifetime + TimeSpan.FromSeconds(30),
            Size = 1,
        });

        return code;
    }

    /// <summary>
    /// Redeem a code: removes it (single use), then returns the entry only when
    /// it hasn't expired. Null = unknown, already used, or expired — the caller
    /// answers 401 for all three, exactly like the Rust <c>sso_exchange</c>.
    /// </summary>
    public SsoCodeEntry? Redeem(string code)
    {
        if (string.IsNullOrEmpty(code))
        {
            return null;
        }

        var key = KeyPrefix + code;
        if (!cache.TryGetValue(key, out SsoCodeEntry? entry) || entry is null)
        {
            return null;
        }

        // Burn first, check expiry second — same order as Rust (remove, then
        // compare expires_at).
        cache.Remove(key);

        return _time.GetUtcNow() > entry.ExpiresAt ? null : entry;
    }
}
