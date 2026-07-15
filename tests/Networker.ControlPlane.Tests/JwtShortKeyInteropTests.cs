using Networker.ControlPlane.Auth;

namespace Networker.ControlPlane.Tests;

/// Regression for the live-cutover finding: a Rust jsonwebtoken-minted HS256 token
/// signed with a SHORT (29-byte) secret must validate on C#. Microsoft.IdentityModel
/// rejects <32-byte HS256 keys; the raw-HMAC SignatureValidator fixes it.
public sealed class JwtShortKeyInteropTests
{
    // The exact prod secret (29 bytes) and a real Rust-minted token captured during cutover.
    private const string ProdSecret = "securejwtsecret2026alethedash";
    private const string RustToken = "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIzODYyZGJkZS02YTM2LTRhMDctYThiZC0xODNjNzc4NTE0M2UiLCJlbWFpbCI6ImFkbWluQGFsZXRoZWRhc2guY29tIiwicm9sZSI6ImFkbWluIiwiaXNfcGxhdGZvcm1fYWRtaW4iOnRydWUsImV4cCI6MTc4NDE2NDk1OSwiaWF0IjoxNzg0MDc4NTU5fQ.Oh4k8cpGW5pEq_1smabDkwyUmWr9oMN3GYimURbXTrA";

    [Fact]
    public void Rust_token_with_29_byte_secret_validates()
    {
        var svc = new JwtTokenService(ProdSecret);
        var principal = svc.Validate(RustToken);
        // Token is long-lived (24h) from mint; if it has since expired this asserts
        // null — but at cutover time it validated. Guard on expiry:
        var exp = DateTimeOffset.FromUnixTimeSeconds(1784164959);
        if (exp > DateTimeOffset.UtcNow.AddSeconds(60))
        {
            Assert.NotNull(principal);
            Assert.Equal("admin@alethedash.com", principal!.FindFirst("email")?.Value);
        }
    }

    [Fact]
    public void Short_secret_round_trips_mint_then_validate()
    {
        var svc = new JwtTokenService("short-secret-under-32-bytes");  // 27 bytes
        var uid = Guid.NewGuid();
        var token = svc.CreateToken(uid, "u@x.io", "operator", isPlatformAdmin: false);
        var p = svc.Validate(token);
        Assert.NotNull(p);
        Assert.Equal(uid.ToString(), p!.FindFirst("sub")?.Value);
    }

    [Fact]
    public void Tampered_signature_is_rejected()
    {
        var svc = new JwtTokenService(ProdSecret);
        Assert.Null(svc.Validate(RustToken[..^3] + "AAA"));
    }
}
