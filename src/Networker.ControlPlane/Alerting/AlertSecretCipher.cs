using System.Text;
using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.Data;
using Networker.Security;

namespace Networker.ControlPlane.Alerting;

/// <summary>
/// Encrypts the alert-webhook HMAC secret at rest (secrets audit 2026-07, P1 —
/// it was the only user-supplied shared secret still stored plaintext, inside
/// <c>alert_channel.config</c>). The ciphertext is stored IN PLACE of the
/// plaintext in the same JSON <c>secret</c> field (no schema change), tagged with
/// a marker prefix so reads can tell an encrypted value from a legacy plaintext
/// one and migrate transparently. Uses the same AES-256-GCM
/// <see cref="CredentialCipher"/> as cloud-account credentials and SDK tokens.
///
/// <para>Wire format of a protected secret:
/// <c>lhenc$1$&lt;base64(nonce)&gt;$&lt;base64(ciphertext+tag)&gt;</c>. A value
/// without the marker is treated as legacy plaintext on read (back-compat) and
/// re-encrypted on the next write / by <see cref="BackfillAsync"/>.</para>
/// </summary>
public static class AlertSecretCipher
{
    /// <summary>Prefix marking a value that has been AES-256-GCM encrypted by
    /// this helper. Versioned so the scheme can evolve.</summary>
    public const string Marker = "lhenc$1$";

    /// <summary>True when <paramref name="value"/> is an encrypted secret.</summary>
    public static bool IsProtected(string? value) =>
        value is not null && value.StartsWith(Marker, StringComparison.Ordinal);

    /// <summary>Encrypt a plaintext secret to the <see cref="Marker"/> wire form.</summary>
    public static string Protect(CredentialCipher cipher, string plaintext)
    {
        var (ciphertext, nonce) = cipher.Encrypt(Encoding.UTF8.GetBytes(plaintext));
        return Marker + Convert.ToBase64String(nonce) + "$" + Convert.ToBase64String(ciphertext);
    }

    /// <summary>
    /// Recover the plaintext secret for signing. Null/empty passes through; a
    /// value WITHOUT the marker is returned verbatim (legacy plaintext, pre-
    /// encryption rows). A marked value is decrypted; a decrypt failure (e.g. a
    /// key rotated without the old key configured) returns null so the caller
    /// simply omits the signature rather than signing with garbage.
    /// </summary>
    public static string? Unprotect(CredentialCipher cipher, string? stored)
    {
        if (string.IsNullOrEmpty(stored) || !IsProtected(stored))
        {
            return stored;
        }

        try
        {
            var body = stored[Marker.Length..];
            var sep = body.IndexOf('$', StringComparison.Ordinal);
            if (sep < 0)
            {
                return null;
            }
            var nonce = Convert.FromBase64String(body[..sep]);
            var ciphertext = Convert.FromBase64String(body[(sep + 1)..]);
            return Encoding.UTF8.GetString(cipher.Decrypt(ciphertext, nonce));
        }
        catch
        {
            return null;
        }
    }

    /// <summary>
    /// Return <paramref name="configJson"/> with its webhook <c>secret</c> field
    /// encrypted, or the original string unchanged when there is nothing to do
    /// (no object / no string secret / empty / already protected). Called by the
    /// channel create + patch handlers AFTER validation, so the plaintext secret
    /// never reaches the database. Preserves all other fields (e.g. <c>url</c>).
    /// </summary>
    public static string ProtectConfigSecret(CredentialCipher cipher, string configJson)
    {
        using var doc = JsonDocument.Parse(configJson);
        if (doc.RootElement.ValueKind != JsonValueKind.Object
            || !doc.RootElement.TryGetProperty("secret", out var secretEl)
            || secretEl.ValueKind != JsonValueKind.String)
        {
            return configJson;
        }

        var secret = secretEl.GetString();
        if (string.IsNullOrEmpty(secret) || IsProtected(secret))
        {
            return configJson;
        }

        var protectedSecret = Protect(cipher, secret);
        var rewritten = doc.RootElement.EnumerateObject().ToDictionary(
            p => p.Name,
            p => p.Name == "secret"
                ? JsonSerializer.SerializeToElement(protectedSecret)
                : p.Value.Clone());
        return JsonSerializer.Serialize(rewritten);
    }

    /// <summary>
    /// One-shot idempotent backfill: encrypt any legacy plaintext webhook secret
    /// still sitting in <c>alert_channel.config</c>. Best-effort — a failure is
    /// logged and swallowed so it can never block startup. Returns the number of
    /// rows migrated (0 once every secret is protected).
    /// </summary>
    public static async Task<int> BackfillAsync(
        NetworkerDbContext db, CredentialCipher cipher, ILogger logger, CancellationToken ct = default)
    {
        var migrated = 0;
        try
        {
            var channels = await db.AlertChannels.Where(c => c.Kind == "webhook").ToListAsync(ct);
            foreach (var ch in channels)
            {
                var updated = ProtectConfigSecret(cipher, ch.Config);
                if (!ReferenceEquals(updated, ch.Config))
                {
                    ch.Config = updated;
                    migrated++;
                }
            }
            if (migrated > 0)
            {
                await db.SaveChangesAsync(ct);
            }
        }
        catch (Exception ex)
        {
            logger.LogWarning(ex, "Alert-webhook secret backfill skipped (non-fatal)");
        }
        return migrated;
    }
}
