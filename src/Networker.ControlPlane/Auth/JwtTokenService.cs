using System.IdentityModel.Tokens.Jwt;
using System.Security.Claims;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using Microsoft.IdentityModel.Tokens;

namespace Networker.ControlPlane.Auth;

/// <summary>
/// Mints and validates JWTs that are byte-compatible with the Rust dashboard's
/// <c>auth::create_token</c> / <c>auth::validate_token</c> (the <c>jsonwebtoken</c>
/// crate). A token minted here validates on the Rust side and vice-versa.
///
/// Scheme matched exactly: HS256; header <c>{ "alg": "HS256", "typ": "JWT" }</c>;
/// secret = raw UTF-8 bytes of <c>DASHBOARD_JWT_SECRET</c> (Rust
/// <c>EncodingKey::from_secret(secret.as_bytes())</c>, NOT base64-decoded); claims
/// <c>sub</c>/<c>email</c>/<c>role</c>/<c>is_platform_admin</c> (JSON bool) /
/// <c>exp</c>/<c>iat</c> (unix seconds); 24h TTL.
///
/// <para><b>Raw-HMAC signing/validation (not <c>SymmetricSecurityKey</c>):</b> Rust's
/// <c>jsonwebtoken</c> does a plain HMAC-SHA256 over <c>header.payload</c> with the raw
/// secret bytes and accepts ANY key length. Microsoft.IdentityModel enforces a ≥256-bit
/// key for HS256 and rejects shorter secrets (IDX10517). Real deployments carry secrets
/// shorter than 32 bytes, so we sign and verify with <see cref="HMACSHA256"/> directly —
/// byte-identical to <c>jsonwebtoken</c> for every key length (the ≥32-byte path is
/// unchanged). Found during the live cutover: a Rust-minted token with the 29-byte prod
/// secret validated on Rust but 401'd on C# until this landed.</para>
/// </summary>
public sealed class JwtTokenService
{
    public const string SecretEnvVar = "DASHBOARD_JWT_SECRET";
    public const int TokenTtlSeconds = 24 * 3600; // 24h, matches Rust create_token
    public const string SubClaim = "sub";
    public const string EmailClaim = "email";
    public const string RoleClaim = "role";
    public const string PlatformAdminClaim = "is_platform_admin";

    private const int ClockSkewSeconds = 60; // jsonwebtoken default leeway

    private readonly byte[] _secretBytes;

    public JwtTokenService(string secret)
    {
        if (string.IsNullOrEmpty(secret))
        {
            throw new ArgumentException(
                $"{SecretEnvVar} must be set (raw UTF-8 secret; generate with: openssl rand -base64 32)",
                nameof(secret));
        }

        // Rust does EncodingKey::from_secret(secret.as_bytes()) — the raw UTF-8
        // bytes of the string, no base64 decode. Match that exactly.
        _secretBytes = Encoding.UTF8.GetBytes(secret);
    }

    /// <summary>Kept for compatibility; not used for HS256 signing of short keys
    /// (that path re-enters M.IdentityModel's key-size enforcement).</summary>
    public SymmetricSecurityKey SigningKey => new(_secretBytes);

    /// <summary>Raw HMAC-SHA256 of a signing input, base64url-encoded — the
    /// jsonwebtoken signing primitive. Works for any secret length.</summary>
    public string SignHs256(string signingInput)
    {
        var mac = HMACSHA256.HashData(_secretBytes, Encoding.ASCII.GetBytes(signingInput));
        return Base64UrlEncode(mac);
    }

    /// <summary>
    /// Mint a token with the same claim set + HS256 signature the Rust side produces,
    /// via raw HMAC so it works for any secret length. <c>is_platform_admin</c> is a
    /// JSON boolean; <c>exp</c>/<c>iat</c> are JSON numbers (matching serde).
    /// </summary>
    public string CreateToken(Guid userId, string email, string role, bool isPlatformAdmin)
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        var exp = now + TokenTtlSeconds;

        const string headerJson = "{\"alg\":\"HS256\",\"typ\":\"JWT\"}";
        var payload = new Dictionary<string, object>
        {
            [SubClaim] = userId.ToString(),
            [EmailClaim] = email,
            [RoleClaim] = role,
            [PlatformAdminClaim] = isPlatformAdmin,
            ["exp"] = exp,
            ["iat"] = now,
        };
        var payloadJson = JsonSerializer.Serialize(payload);

        var signingInput =
            Base64UrlEncode(Encoding.UTF8.GetBytes(headerJson)) + "." +
            Base64UrlEncode(Encoding.UTF8.GetBytes(payloadJson));

        return signingInput + "." + SignHs256(signingInput);
    }

    /// <summary>
    /// TokenValidationParameters mirroring Rust's <c>Validation::default()</c>: HS256,
    /// signature via the raw-HMAC <see cref="ValidateSignatureRaw"/>, lifetime with 60s
    /// skew, no issuer/audience checks, claim names untouched.
    /// </summary>
    public TokenValidationParameters ValidationParameters => new()
    {
        ValidateIssuer = false,
        ValidateAudience = false,
        ValidateIssuerSigningKey = false,
        RequireSignedTokens = true,
        ValidAlgorithms = new[] { SecurityAlgorithms.HmacSha256 },
        ValidateLifetime = true,
        RequireExpirationTime = true,
        ClockSkew = TimeSpan.FromSeconds(ClockSkewSeconds),
        NameClaimType = EmailClaim,
        RoleClaimType = RoleClaim,
        // Replace M.IdentityModel's key-size-enforcing signature check with a raw
        // HMAC-SHA256 compare — byte-identical to Rust jsonwebtoken for any key length.
        SignatureValidator = ValidateSignatureRaw,
    };

    private SecurityToken ValidateSignatureRaw(string token, TokenValidationParameters _)
    {
        var parts = token.Split('.');
        if (parts.Length != 3)
        {
            throw new SecurityTokenMalformedException("JWT must have three parts");
        }

        var signingInput = parts[0] + "." + parts[1];
        var expected = SignHs256(signingInput);

        var a = Encoding.ASCII.GetBytes(expected);
        var b = Encoding.ASCII.GetBytes(parts[2]);
        if (a.Length != b.Length || !CryptographicOperations.FixedTimeEquals(a, b))
        {
            throw new SecurityTokenInvalidSignatureException("HMAC-SHA256 signature mismatch");
        }

        return new JwtSecurityToken(token);
    }

    /// <summary>
    /// Validate a token and return the resolved principal, or null when invalid
    /// (bad signature, wrong alg, expired, malformed). Mirrors Rust <c>validate_token</c>.
    /// </summary>
    public ClaimsPrincipal? Validate(string token)
    {
        if (string.IsNullOrEmpty(token))
        {
            return null;
        }

        try
        {
            var handler = new JwtSecurityTokenHandler();
            handler.InboundClaimTypeMap.Clear(); // keep sub/email/role names verbatim
            return handler.ValidateToken(token, ValidationParameters, out _);
        }
        catch (Exception)
        {
            return null;
        }
    }

    private static string Base64UrlEncode(byte[] bytes) =>
        Convert.ToBase64String(bytes).TrimEnd('=').Replace('+', '-').Replace('/', '_');
}
