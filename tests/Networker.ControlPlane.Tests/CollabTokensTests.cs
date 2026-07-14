using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// Unit tests for the invite/share-link token machinery — the raw-token shape
/// (URL-safe base64 no-pad of 32 CSPRNG bytes) and the SHA-256 storage hash
/// must stay byte-compatible with the Rust dashboard so tokens minted by
/// either backend resolve on the other.
public sealed class CollabTokensTests
{
    [Fact]
    public void Token_is_43_chars()
    {
        // 32 bytes → ceil(32 * 4 / 3) = 44 base64 chars incl. one '=' pad,
        // which URL_SAFE_NO_PAD strips → 43.
        Assert.Equal(43, CollabTokens.Generate().Length);
    }

    [Fact]
    public void Token_is_url_safe()
    {
        // No '+', '/', or '=' — the token is embedded in a path segment
        // (/invite/{token}, /share/{token}) and must never need escaping.
        for (var i = 0; i < 100; i++)
        {
            var token = CollabTokens.Generate();
            Assert.All(token, c => Assert.True(
                char.IsAsciiLetterOrDigit(c) || c is '-' or '_',
                $"unexpected token char '{c}' in {token}"));
        }
    }

    [Fact]
    public void Tokens_are_unique()
    {
        var tokens = Enumerable.Range(0, 1000).Select(_ => CollabTokens.Generate()).ToHashSet();
        Assert.Equal(1000, tokens.Count);
    }

    [Fact]
    public void UrlSafe_encoding_maps_base64_specials()
    {
        // 0xFB 0xEF 0xBE is "++++" in the standard base64 alphabet (all four
        // 6-bit groups are 62) → must become "----" in base64url (RFC 4648 §5),
        // matching the Rust URL_SAFE_NO_PAD engine.
        Assert.Equal("----", CollabTokens.ToUrlSafeBase64NoPad([0xFB, 0xEF, 0xBE]));

        // 0xFF 0xFF 0xFF is "////" (all groups 63) → "____".
        Assert.Equal("____", CollabTokens.ToUrlSafeBase64NoPad([0xFF, 0xFF, 0xFF]));

        // Padding is stripped, not substituted: 1 byte → 2 chars, no '='.
        Assert.Equal("AA", CollabTokens.ToUrlSafeBase64NoPad([0x00]));
    }

    [Fact]
    public void Hash_matches_known_sha256_vector()
    {
        // SHA-256("abc") — FIPS 180-2 appendix B.1. Lowercase hex, like the
        // Rust hex::encode(hasher.finalize()).
        Assert.Equal(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            CollabTokens.Sha256Hex("abc"));
    }

    [Fact]
    public void Hash_is_64_lowercase_hex_chars()
    {
        var hash = CollabTokens.Sha256Hex(CollabTokens.Generate());
        Assert.Equal(64, hash.Length);
        Assert.All(hash, c => Assert.True(char.IsAsciiDigit(c) || (c >= 'a' && c <= 'f')));
    }

    [Fact]
    public void Hash_is_deterministic_and_input_sensitive()
    {
        var token = CollabTokens.Generate();
        Assert.Equal(CollabTokens.Sha256Hex(token), CollabTokens.Sha256Hex(token));
        Assert.NotEqual(CollabTokens.Sha256Hex(token), CollabTokens.Sha256Hex(token + "x"));
    }
}
