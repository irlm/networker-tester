using System.Text.Json;
using Networker.ControlPlane.Alerting;
using Networker.Security;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Encryption-at-rest for the alert-webhook HMAC secret (secrets audit 2026-07).
/// Pins the round-trip, the legacy-plaintext passthrough (back-compat for
/// pre-encryption rows), and the config-field rewrite the create/patch handlers
/// apply so a plaintext secret never reaches the database.
/// </summary>
public sealed class AlertSecretCipherTests
{
    private static CredentialCipher Cipher() =>
        new(Enumerable.Range(0, CredentialCipher.KeySize).Select(i => (byte)i).ToArray());

    [Fact]
    public void Protect_then_unprotect_round_trips()
    {
        var cipher = Cipher();
        var protectedSecret = AlertSecretCipher.Protect(cipher, "s3cr3t-hmac-key");

        Assert.True(AlertSecretCipher.IsProtected(protectedSecret));
        Assert.StartsWith(AlertSecretCipher.Marker, protectedSecret);
        Assert.DoesNotContain("s3cr3t-hmac-key", protectedSecret);
        Assert.Equal("s3cr3t-hmac-key", AlertSecretCipher.Unprotect(cipher, protectedSecret));
    }

    [Fact]
    public void Each_protect_uses_a_fresh_nonce_but_decrypts_to_the_same_plaintext()
    {
        var cipher = Cipher();
        var a = AlertSecretCipher.Protect(cipher, "same");
        var b = AlertSecretCipher.Protect(cipher, "same");

        Assert.NotEqual(a, b); // random nonce → different ciphertext
        Assert.Equal("same", AlertSecretCipher.Unprotect(cipher, a));
        Assert.Equal("same", AlertSecretCipher.Unprotect(cipher, b));
    }

    [Fact]
    public void Unprotect_passes_through_legacy_plaintext_and_null()
    {
        var cipher = Cipher();
        Assert.Equal("legacy-plaintext", AlertSecretCipher.Unprotect(cipher, "legacy-plaintext"));
        Assert.Null(AlertSecretCipher.Unprotect(cipher, null));
        Assert.Equal("", AlertSecretCipher.Unprotect(cipher, ""));
    }

    [Fact]
    public void Unprotect_returns_null_when_a_marked_value_cannot_be_decrypted()
    {
        // Marked but corrupt → cannot sign; caller omits the signature.
        Assert.Null(AlertSecretCipher.Unprotect(Cipher(), AlertSecretCipher.Marker + "not-base64$nope"));
    }

    [Fact]
    public void ProtectConfigSecret_encrypts_a_webhook_secret_and_preserves_url()
    {
        var cipher = Cipher();
        var config = JsonSerializer.Serialize(new { url = "https://hooks.example.com/x", secret = "top" });

        var result = AlertSecretCipher.ProtectConfigSecret(cipher, config);

        using var doc = JsonDocument.Parse(result);
        Assert.Equal("https://hooks.example.com/x", doc.RootElement.GetProperty("url").GetString());
        var stored = doc.RootElement.GetProperty("secret").GetString();
        Assert.True(AlertSecretCipher.IsProtected(stored));
        Assert.Equal("top", AlertSecretCipher.Unprotect(cipher, stored));
    }

    [Fact]
    public void ProtectConfigSecret_is_a_noop_for_email_secretless_and_already_protected()
    {
        var cipher = Cipher();

        var email = JsonSerializer.Serialize(new { to = new[] { "sre@example.com" } });
        Assert.Same(email, AlertSecretCipher.ProtectConfigSecret(cipher, email));

        var noSecret = JsonSerializer.Serialize(new { url = "https://h/x" });
        Assert.Same(noSecret, AlertSecretCipher.ProtectConfigSecret(cipher, noSecret));

        // Already-protected → returned unchanged (idempotent; no double-encryption).
        var once = AlertSecretCipher.ProtectConfigSecret(
            cipher, JsonSerializer.Serialize(new { url = "https://h/x", secret = "k" }));
        Assert.Same(once, AlertSecretCipher.ProtectConfigSecret(cipher, once));
    }
}
