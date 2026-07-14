using Networker.ControlPlane.Sso;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pure parts of the account/password endpoints: reset-token generation +
/// SHA-256 storage hashing (must interoperate with rows the Rust dashboard
/// wrote), the 1-hour expiry rule, the password policies, and the bcrypt
/// verify/hash path that change-password rides on.
/// </summary>
public sealed class AccountSecurityTests
{
    // ── Reset tokens ──────────────────────────────────────────────────────────

    [Fact]
    public void GenerateAlphanumericToken_LengthAndUrlSafeCharset()
    {
        var token = AccountSecurity.GenerateAlphanumericToken(AccountSecurity.ResetTokenLength);

        Assert.Equal(64, token.Length); // Rust takes 64 Alphanumeric samples
        Assert.All(token, c => Assert.True(char.IsAsciiLetterOrDigit(c)));
        // Alphanumeric-only ⇒ the reset link needs no URL encoding.
        Assert.Equal(token, Uri.EscapeDataString(token));
    }

    [Fact]
    public void GenerateAlphanumericToken_IsNotDeterministic()
    {
        var a = AccountSecurity.GenerateAlphanumericToken(64);
        var b = AccountSecurity.GenerateAlphanumericToken(64);
        Assert.NotEqual(a, b);
    }

    [Fact]
    public void HashToken_MatchesRustSha256HexScheme()
    {
        // Rust: hex::encode(Sha256::digest(token)) — lowercase hex. Known
        // vector: SHA-256("abc").
        Assert.Equal(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            AccountSecurity.HashToken("abc"));
    }

    [Fact]
    public void HashToken_NeverEqualsThePlaintext()
    {
        var token = AccountSecurity.GenerateAlphanumericToken(64);
        var hash = AccountSecurity.HashToken(token);
        Assert.NotEqual(token, hash);
        Assert.Equal(64, hash.Length); // 32 bytes → 64 hex chars fits the varchar(128) column
    }

    // ── Expiry (1 hour, Rust chrono::Duration::hours(1)) ─────────────────────

    [Fact]
    public void ResetTokenLifetime_IsOneHour()
    {
        Assert.Equal(TimeSpan.FromHours(1), AccountSecurity.ResetTokenLifetime);
    }

    [Fact]
    public void IsResetTokenExpired_ExpiryLogic()
    {
        var now = new DateTime(2026, 7, 14, 12, 0, 0, DateTimeKind.Utc);

        // Freshly issued token: expires now + 1h → valid for the whole window.
        var expires = now + AccountSecurity.ResetTokenLifetime;
        Assert.False(AccountSecurity.IsResetTokenExpired(expires, now));
        Assert.False(AccountSecurity.IsResetTokenExpired(expires, now.AddMinutes(59)));

        // One second past the expiry → dead.
        Assert.True(AccountSecurity.IsResetTokenExpired(expires, expires.AddSeconds(1)));

        // Boundary: Rust checks `exp < now`, so exactly-at-expiry is still valid.
        Assert.False(AccountSecurity.IsResetTokenExpired(expires, expires));

        // Missing expiry column ⇒ invalid link (Rust's None arm).
        Assert.True(AccountSecurity.IsResetTokenExpired(null, now));
    }

    // ── Password policies (Rust's exact messages) ────────────────────────────

    [Theory]
    [InlineData("short7!", "Password must be at least 8 characters")]
    [InlineData("", "Password must be at least 8 characters")]
    [InlineData("longenough", null)]
    public void ValidateResetPassword_MinimumLength(string password, string? expected)
    {
        Assert.Equal(expected, AccountSecurity.ValidateResetPassword(password));
    }

    [Fact]
    public void ValidateChangedPassword_RejectsShortAndUnchanged()
    {
        Assert.Equal(
            "New password must be at least 8 characters",
            AccountSecurity.ValidateChangedPassword("current-pass", "tiny"));
        Assert.Equal(
            "New password must be different from current password",
            AccountSecurity.ValidateChangedPassword("same-password", "same-password"));
        Assert.Null(AccountSecurity.ValidateChangedPassword("old-password", "new-password"));
    }

    // ── bcrypt path (what change-password / reset-password store + verify) ───

    [Fact]
    public void Bcrypt_HashAndVerify_RoundTrip()
    {
        var hash = BCrypt.Net.BCrypt.HashPassword("correct horse battery staple");

        Assert.True(BCrypt.Net.BCrypt.Verify("correct horse battery staple", hash));
        Assert.False(BCrypt.Net.BCrypt.Verify("wrong password", hash));
        Assert.StartsWith("$2", hash); // modular-crypt bcrypt prefix, same family Rust bcrypt writes
    }

    [Fact]
    public void Bcrypt_VerifiesHashesTheRustSideWrote()
    {
        // The Rust dashboard writes bcrypt modular-crypt hashes; BCrypt.Net must
        // verify rows produced by other implementations for shared-DB cutover.
        // Fixed externally generated vector: bcrypt("networker-test", cost 10).
        const string rustStyleHash = "$2y$10$SiDOPfui8.KpQzH05r7wrenOwKwxYCDFUK1DT20ZpTvPYpS1ZXhli";
        Assert.True(BCrypt.Net.BCrypt.Verify("networker-test", rustStyleHash));
        Assert.False(BCrypt.Net.BCrypt.Verify("networker-wrong", rustStyleHash));
    }
}
