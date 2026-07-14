using System.Security.Cryptography;
using System.Text;
using Networker.Security;

namespace Networker.Security.Tests;

public class CredentialCipherTests
{
    private static byte[] Key(byte fill) => Enumerable.Repeat(fill, CredentialCipher.KeySize).ToArray();

    // ---- Round-trip -------------------------------------------------------

    public static IEnumerable<object[]> RoundTripInputs()
    {
        yield return new object[] { Array.Empty<byte>() };                          // empty
        yield return new object[] { Encoding.UTF8.GetBytes("x") };                  // 1 byte
        yield return new object[] { Encoding.UTF8.GetBytes("hello world") };        // short text
        yield return new object[] { Encoding.UTF8.GetBytes("{\"k\":\"secret\"}") }; // JSON-like
        yield return new object[] { Enumerable.Range(0, 1024).Select(i => (byte)i).ToArray() }; // 1KB binary
        yield return new object[] { new byte[] { 0x00, 0xFF, 0x00, 0xFF, 0x10, 0x80 } };        // binary
    }

    [Theory]
    [MemberData(nameof(RoundTripInputs))]
    public void Encrypt_Then_Decrypt_Returns_Original(byte[] plaintext)
    {
        var cipher = new CredentialCipher(Key(0x42));
        var (blob, nonce) = cipher.Encrypt(plaintext);

        Assert.Equal(CredentialCipher.NonceSize, nonce.Length);
        // RustCrypto layout: blob = ciphertext || 16-byte tag; ciphertext len == plaintext len.
        Assert.Equal(plaintext.Length + CredentialCipher.TagSize, blob.Length);

        var recovered = cipher.Decrypt(blob, nonce);
        Assert.Equal(plaintext, recovered);
    }

    [Fact]
    public void Nonce_Is_Random_Per_Encryption()
    {
        var cipher = new CredentialCipher(Key(0x11));
        var (_, n1) = cipher.Encrypt(Encoding.UTF8.GetBytes("data"));
        var (_, n2) = cipher.Encrypt(Encoding.UTF8.GetBytes("data"));
        Assert.NotEqual(n1, n2);
    }

    [Fact]
    public void Wrong_Key_Fails_Authentication()
    {
        var enc = new CredentialCipher(Key(0x42));
        var (blob, nonce) = enc.Encrypt(Encoding.UTF8.GetBytes("secret"));

        var dec = new CredentialCipher(Key(0x43));
        Assert.ThrowsAny<CryptographicException>(() => dec.Decrypt(blob, nonce));
    }

    [Fact]
    public void Tampered_Ciphertext_Fails_Authentication()
    {
        var cipher = new CredentialCipher(Key(0x42));
        var (blob, nonce) = cipher.Encrypt(Encoding.UTF8.GetBytes("secret"));
        blob[0] ^= 0x01; // flip a bit
        Assert.ThrowsAny<CryptographicException>(() => cipher.Decrypt(blob, nonce));
    }

    // ---- Key rotation fallback -------------------------------------------

    [Fact]
    public void OldKey_Fallback_Decrypts_Value_Encrypted_Under_OldKey()
    {
        var oldKey = Key(0x42);
        var newKey = Key(0x43);

        // Encrypt under the OLD key (as the Rust dashboard would have, pre-rotation).
        var producer = new CredentialCipher(oldKey);
        var (blob, nonce) = producer.Encrypt(Encoding.UTF8.GetBytes("data"));

        // Consumer has new primary + old as fallback → primary fails, fallback succeeds.
        var consumer = new CredentialCipher(newKey, oldKey);
        var recovered = consumer.Decrypt(blob, nonce);
        Assert.Equal("data", Encoding.UTF8.GetString(recovered));
    }

    [Fact]
    public void No_Fallback_Configured_Throws_When_Primary_Wrong()
    {
        var oldKey = Key(0x42);
        var newKey = Key(0x43);

        var producer = new CredentialCipher(oldKey);
        var (blob, nonce) = producer.Encrypt(Encoding.UTF8.GetBytes("data"));

        var consumer = new CredentialCipher(newKey); // no old key
        Assert.ThrowsAny<CryptographicException>(() => consumer.Decrypt(blob, nonce));
    }

    [Fact]
    public void Primary_Key_Takes_Precedence_Over_Fallback()
    {
        var primary = Key(0x43);
        var old = Key(0x42);

        var producer = new CredentialCipher(primary);
        var (blob, nonce) = producer.Encrypt(Encoding.UTF8.GetBytes("current"));

        var consumer = new CredentialCipher(primary, old);
        Assert.Equal("current", Encoding.UTF8.GetString(consumer.Decrypt(blob, nonce)));
    }

    // ---- KNOWN-ANSWER cross-check vs REAL Rust output --------------------
    //
    // Ground-truth vector produced by the ACTUAL Rust dashboard crate
    // (aes-gcm 0.10.3, RustCrypto) via a throwaway test added to
    // crates/networker-dashboard/src/crypto.rs, run with:
    //   cargo test -p networker-dashboard --bin networker-dashboard vector_gen -- --nocapture
    // then reverted. Inputs:
    //   key       = bytes 0x00..0x1f  (32 bytes, ascending)
    //   nonce     = bytes 0x00..0x0b  (12 bytes, ascending)
    //   plaintext = "networker-credential-vector" (27 bytes UTF-8)
    // Rust emitted this exact ciphertext||tag blob (43 bytes = 27 ct + 16 tag):
    private const string RustCiphertextWithTagHex =
        "2967a26caa97a97eff6cf4f9d48d1d03f7bfe658dd0d3a1f4c089794314910ba922348f77cb16dae920330";

    private static byte[] AscendingBytes(int count) =>
        Enumerable.Range(0, count).Select(i => (byte)i).ToArray();

    [Fact]
    public void CSharp_Encrypt_Matches_Rust_Known_Vector_Byte_For_Byte()
    {
        var key = AscendingBytes(32);
        var nonce = AscendingBytes(12);
        var plaintext = Encoding.UTF8.GetBytes("networker-credential-vector");

        var cipher = new CredentialCipher(key);
        var blob = cipher.EncryptWithNonce(plaintext, nonce);

        var expected = Convert.FromHexString(RustCiphertextWithTagHex);
        // This assertion FAILS if the C# scheme drifts from Rust in any way:
        // cipher choice, key/nonce/tag size, tag placement, or AAD.
        Assert.Equal(expected, blob);
    }

    [Fact]
    public void CSharp_Decrypt_Recovers_Plaintext_From_Rust_Blob()
    {
        var key = AscendingBytes(32);
        var nonce = AscendingBytes(12);
        var blob = Convert.FromHexString(RustCiphertextWithTagHex);

        var cipher = new CredentialCipher(key);
        var recovered = cipher.Decrypt(blob, nonce);

        Assert.Equal("networker-credential-vector", Encoding.UTF8.GetString(recovered));
    }

    // ---- Key hex parsing --------------------------------------------------

    [Fact]
    public void KeyFromHex_Parses_64_Char_Key()
    {
        var hex = string.Concat(Enumerable.Range(0, 32).Select(i => i.ToString("x2")));
        var key = CredentialCipher.KeyFromHex(hex);
        Assert.Equal(AscendingBytes(32), key);
    }

    [Theory]
    [InlineData("")]
    [InlineData("abcd")]
    public void KeyFromHex_Rejects_Wrong_Length(string hex)
    {
        Assert.Throws<ArgumentException>(() => CredentialCipher.KeyFromHex(hex));
    }
}
