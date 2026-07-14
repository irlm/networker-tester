using System.Security.Cryptography;

namespace Networker.Security;

/// <summary>
/// Byte-for-byte port of the Rust dashboard credential-encryption scheme
/// (<c>crates/networker-dashboard/src/crypto.rs</c>).
///
/// Scheme reproduced EXACTLY so this C# control plane can decrypt cloud-account
/// secrets that the Rust dashboard wrote to the shared Postgres DB:
///
///   - Cipher:  AES-256-GCM (Rust `aes-gcm` crate, RustCrypto).
///   - Key:     32 bytes (256-bit). Sourced as 64 hex chars from
///              DASHBOARD_CREDENTIAL_KEY / credential.key file on the Rust side.
///   - Nonce:   12 bytes, random per encryption.
///   - Tag:     16 bytes (128-bit GCM auth tag).
///   - Layout:  RustCrypto's `aead::encrypt` returns `ciphertext || tag`
///              (tag APPENDED to ciphertext). The Rust dashboard stores that
///              blob and the 12-byte nonce in SEPARATE DB columns
///              (e.g. `credentials_enc` + `credentials_nonce`).
///   - AAD:     none (Rust passes the plaintext slice directly, no associated data).
///   - Rotation: decrypt tries the primary key first, then falls back to an
///               optional "old" key (matches `decrypt_with_fallback`).
///
/// This class deliberately mirrors the Rust storage shape: Encrypt returns the
/// ciphertext-with-tag blob and the nonce as two separate byte arrays, and
/// Decrypt takes them back the same way.
/// </summary>
public sealed class CredentialCipher
{
    /// <summary>AES-256 key size in bytes (Rust <c>[u8; 32]</c>).</summary>
    public const int KeySize = 32;

    /// <summary>GCM nonce size in bytes (Rust <c>[u8; 12]</c>).</summary>
    public const int NonceSize = 12;

    /// <summary>GCM authentication tag size in bytes (128-bit, RustCrypto default).</summary>
    public const int TagSize = 16;

    private readonly byte[] _key;
    private readonly byte[]? _oldKey;

    /// <summary>
    /// Construct a cipher with a primary key and an optional old key for
    /// decrypt-time rotation fallback.
    /// </summary>
    /// <param name="key">32-byte primary key.</param>
    /// <param name="oldKey">Optional 32-byte old key used only for decrypt fallback.</param>
    public CredentialCipher(byte[] key, byte[]? oldKey = null)
    {
        ArgumentNullException.ThrowIfNull(key);
        if (key.Length != KeySize)
            throw new ArgumentException($"Key must be {KeySize} bytes, got {key.Length}.", nameof(key));
        if (oldKey is not null && oldKey.Length != KeySize)
            throw new ArgumentException($"Old key must be {KeySize} bytes, got {oldKey.Length}.", nameof(oldKey));

        _key = (byte[])key.Clone();
        _oldKey = oldKey is null ? null : (byte[])oldKey.Clone();
    }

    /// <summary>
    /// Parse a 64-hex-character string into a 32-byte key, matching the Rust
    /// side (<c>hex::decode</c> of DASHBOARD_CREDENTIAL_KEY, lowercase hex,
    /// exactly 64 chars). Hex is case-insensitive on decode.
    /// </summary>
    public static byte[] KeyFromHex(string hex)
    {
        ArgumentNullException.ThrowIfNull(hex);
        if (hex.Length != KeySize * 2)
            throw new ArgumentException($"Key hex must be {KeySize * 2} characters, got {hex.Length}.", nameof(hex));
        return Convert.FromHexString(hex);
    }

    /// <summary>
    /// Encrypt <paramref name="plaintext"/> with AES-256-GCM under the primary key.
    /// Returns the ciphertext-with-appended-tag blob (matching RustCrypto's
    /// <c>ciphertext || tag</c> output) and the freshly generated 12-byte nonce.
    /// Store both exactly as the Rust dashboard does (separate columns).
    /// </summary>
    public (byte[] ciphertext, byte[] nonce) Encrypt(byte[] plaintext)
    {
        ArgumentNullException.ThrowIfNull(plaintext);

        byte[] nonce = new byte[NonceSize];
        RandomNumberGenerator.Fill(nonce);

        byte[] blob = EncryptWith(_key, plaintext, nonce);
        return (blob, nonce);
    }

    /// <summary>
    /// Encrypt with an explicit nonce. Exposed for known-answer/cross-check
    /// testing where the nonce must be fixed to reproduce a Rust vector.
    /// The returned blob is <c>ciphertext || tag</c>.
    /// </summary>
    public byte[] EncryptWithNonce(byte[] plaintext, byte[] nonce)
    {
        ArgumentNullException.ThrowIfNull(plaintext);
        ArgumentNullException.ThrowIfNull(nonce);
        if (nonce.Length != NonceSize)
            throw new ArgumentException($"Nonce must be {NonceSize} bytes, got {nonce.Length}.", nameof(nonce));
        return EncryptWith(_key, plaintext, nonce);
    }

    /// <summary>
    /// Decrypt a ciphertext-with-tag blob + nonce. Tries the primary key first,
    /// then the old key if one was provided (matches Rust
    /// <c>decrypt_with_fallback</c>). Throws <see cref="CryptographicException"/>
    /// if authentication fails under all available keys.
    /// </summary>
    public byte[] Decrypt(byte[] ciphertextWithTag, byte[] nonce)
    {
        ArgumentNullException.ThrowIfNull(ciphertextWithTag);
        ArgumentNullException.ThrowIfNull(nonce);
        if (nonce.Length != NonceSize)
            throw new ArgumentException($"Nonce must be {NonceSize} bytes, got {nonce.Length}.", nameof(nonce));
        if (ciphertextWithTag.Length < TagSize)
            throw new ArgumentException(
                $"Ciphertext blob must be at least {TagSize} bytes (the GCM tag).",
                nameof(ciphertextWithTag));

        try
        {
            return DecryptWith(_key, ciphertextWithTag, nonce);
        }
        catch (CryptographicException) when (_oldKey is not null)
        {
            // Rotation fallback: identical semantics to the Rust
            // `Err(_) if old_key.is_some() => decrypt(.., old_key)` arm.
            return DecryptWith(_oldKey, ciphertextWithTag, nonce);
        }
    }

    private static byte[] EncryptWith(byte[] key, byte[] plaintext, byte[] nonce)
    {
        byte[] cipherOnly = new byte[plaintext.Length];
        byte[] tag = new byte[TagSize];

        using var gcm = new AesGcm(key, TagSize);
        // No associated data (AAD) — the Rust side passes none.
        gcm.Encrypt(nonce, plaintext, cipherOnly, tag);

        // RustCrypto layout: ciphertext || tag.
        byte[] blob = new byte[cipherOnly.Length + TagSize];
        Buffer.BlockCopy(cipherOnly, 0, blob, 0, cipherOnly.Length);
        Buffer.BlockCopy(tag, 0, blob, cipherOnly.Length, TagSize);
        return blob;
    }

    private static byte[] DecryptWith(byte[] key, byte[] ciphertextWithTag, byte[] nonce)
    {
        int cipherLen = ciphertextWithTag.Length - TagSize;

        byte[] cipherOnly = new byte[cipherLen];
        byte[] tag = new byte[TagSize];
        Buffer.BlockCopy(ciphertextWithTag, 0, cipherOnly, 0, cipherLen);
        Buffer.BlockCopy(ciphertextWithTag, cipherLen, tag, 0, TagSize);

        byte[] plaintext = new byte[cipherLen];
        using var gcm = new AesGcm(key, TagSize);
        gcm.Decrypt(nonce, cipherOnly, tag, plaintext);
        return plaintext;
    }
}
