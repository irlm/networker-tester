using System.Security.Cryptography;
using System.Text;

namespace Networker.ControlPlane.Sso;

/// <summary>
/// Pure helpers behind the account/password endpoints — ported from
/// <c>crates/networker-dashboard/src/db/users.rs</c> so the token scheme is
/// interoperable with rows the Rust dashboard wrote:
/// <list type="bullet">
///   <item>Reset tokens: 64 random alphanumeric chars (URL-safe by construction),
///         stored as lowercase SHA-256 hex (never plaintext), valid 1 hour.</item>
///   <item>Password policy: minimum 8 characters; change-password additionally
///         requires the new password to differ from the current one.</item>
/// </list>
/// Static and side-effect-free (except the CSPRNG) so tests hit them directly.
/// </summary>
public static class AccountSecurity
{
    /// <summary>Alphabet of Rust's <c>rand::distr::Alphanumeric</c>.</summary>
    private const string AlphanumericChars =
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

    /// <summary>Reset-token length (Rust takes 64 Alphanumeric samples).</summary>
    public const int ResetTokenLength = 64;

    /// <summary>Reset-token validity (Rust <c>chrono::Duration::hours(1)</c>).</summary>
    public static readonly TimeSpan ResetTokenLifetime = TimeSpan.FromHours(1);

    /// <summary>Minimum password length (Rust checks <c>len() &lt; 8</c>).</summary>
    public const int MinPasswordLength = 8;

    /// <summary>
    /// Cryptographically random alphanumeric token — same alphabet and entropy
    /// source class as the Rust <c>rand::rng().sample_iter(&amp;Alphanumeric)</c>.
    /// Alphanumeric-only means it is URL-safe without encoding.
    /// </summary>
    public static string GenerateAlphanumericToken(int length)
        => RandomNumberGenerator.GetString(AlphanumericChars, length);

    /// <summary>
    /// Lowercase SHA-256 hex of a token — matches Rust <c>hash_token</c>
    /// (<c>hex::encode(Sha256::digest(token))</c>), so a token minted by either
    /// implementation validates against a row written by the other.
    /// </summary>
    public static string HashToken(string token)
        => Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(token)));

    /// <summary>
    /// True when a stored reset expiry is missing or in the past. A null expiry
    /// is invalid (Rust treats a token row without an expiry as a bad link).
    /// </summary>
    public static bool IsResetTokenExpired(DateTime? expiresUtc, DateTime nowUtc)
        => expiresUtc is null || expiresUtc.Value < nowUtc;

    /// <summary>
    /// Validate a new password for the reset flow. Returns the user-facing error
    /// message (Rust's exact string) or null when acceptable.
    /// </summary>
    public static string? ValidateResetPassword(string newPassword)
        => newPassword.Length < MinPasswordLength
            ? "Password must be at least 8 characters"
            : null;

    /// <summary>
    /// Validate a new password for the change-password flow (length + must
    /// differ from current). Returns Rust's exact error strings or null.
    /// </summary>
    public static string? ValidateChangedPassword(string currentPassword, string newPassword)
    {
        if (newPassword.Length < MinPasswordLength)
        {
            return "New password must be at least 8 characters";
        }

        if (currentPassword == newPassword)
        {
            return "New password must be different from current password";
        }

        return null;
    }
}
