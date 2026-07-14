using System.IdentityModel.Tokens.Jwt;
using System.Security.Claims;
using System.Text;
using Microsoft.IdentityModel.Tokens;

namespace Networker.ControlPlane.Auth;

/// <summary>
/// Mints and validates JWTs that are byte-compatible with the Rust dashboard's
/// <c>auth::create_token</c> / <c>auth::validate_token</c> (the <c>jsonwebtoken</c>
/// crate). A token minted here validates on the Rust side and vice-versa.
///
/// Scheme matched exactly:
/// <list type="bullet">
///   <item>Algorithm: HS256 (Rust <c>Header::default()</c>).</item>
///   <item>Header: <c>{ "alg": "HS256", "typ": "JWT" }</c>.</item>
///   <item>Secret: raw UTF-8 bytes of <c>DASHBOARD_JWT_SECRET</c> (Rust
///         <c>EncodingKey::from_secret(secret.as_bytes())</c> — NOT base64-decoded).</item>
///   <item>Claims: <c>sub</c> (UUID string), <c>email</c> (string),
///         <c>role</c> (lowercase string), <c>is_platform_admin</c> (JSON bool),
///         <c>exp</c> (unix seconds), <c>iat</c> (unix seconds).</item>
///   <item>TTL: 24 hours (exp = iat + 86400).</item>
/// </list>
/// </summary>
public sealed class JwtTokenService
{
    public const string SecretEnvVar = "DASHBOARD_JWT_SECRET";
    public const int TokenTtlSeconds = 24 * 3600; // 24h, matches Rust create_token
    public const string SubClaim = "sub";
    public const string EmailClaim = "email";
    public const string RoleClaim = "role";
    public const string PlatformAdminClaim = "is_platform_admin";

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
        // bytes of the string, with no base64 decode. Match that exactly.
        _secretBytes = Encoding.UTF8.GetBytes(secret);
    }

    public SymmetricSecurityKey SigningKey => new(_secretBytes);

    /// <summary>
    /// Mint a token with the same claim set + HS256 signature the Rust side produces.
    /// The <c>is_platform_admin</c> claim is emitted as a raw JSON boolean (not the
    /// string "true"/"false") so serde on the Rust side deserializes it into a bool.
    /// </summary>
    public string CreateToken(Guid userId, string email, string role, bool isPlatformAdmin)
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        var exp = now + TokenTtlSeconds;

        var handler = new JwtSecurityTokenHandler();
        // Preserve exact claim names/types — do not remap sub → nameidentifier etc.
        handler.SetDefaultTimesOnTokenCreation = false;

        var creds = new SigningCredentials(SigningKey, SecurityAlgorithms.HmacSha256);

        var token = new JwtSecurityToken(
            issuer: null,
            audience: null,
            claims: null,
            notBefore: null,
            expires: null,
            signingCredentials: creds);

        // Build the payload by hand so numeric/boolean claims serialize as JSON
        // numbers/booleans (matching serde), not JSON strings.
        var payload = token.Payload;
        payload[SubClaim] = userId.ToString();          // UUID string, matches Rust Uuid serde
        payload[EmailClaim] = email;
        payload[RoleClaim] = role;
        payload[PlatformAdminClaim] = isPlatformAdmin;   // JSON bool
        payload["exp"] = exp;                            // JSON number (unix seconds)
        payload["iat"] = now;                            // JSON number (unix seconds)

        return handler.WriteToken(token);
    }

    /// <summary>
    /// TokenValidationParameters mirroring Rust's <c>Validation::default()</c>:
    /// HS256 only, validate signature + lifetime (60s clock skew, same as the
    /// jsonwebtoken crate's default leeway), and do NOT validate issuer/audience
    /// (Rust default validation checks neither). Claim names are left untouched.
    /// </summary>
    public TokenValidationParameters ValidationParameters => new()
    {
        ValidateIssuer = false,
        ValidateAudience = false,
        ValidateIssuerSigningKey = true,
        IssuerSigningKey = SigningKey,
        ValidAlgorithms = new[] { SecurityAlgorithms.HmacSha256 },
        ValidateLifetime = true,
        RequireExpirationTime = true,
        ClockSkew = TimeSpan.FromSeconds(60),
        NameClaimType = EmailClaim,
        RoleClaimType = RoleClaim,
    };

    /// <summary>
    /// Validate a token and return the resolved principal, or null when invalid
    /// (bad signature, wrong alg, expired, malformed). Mirrors the boolean-ish
    /// outcome of Rust's <c>validate_token</c>.
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
            var principal = handler.ValidateToken(token, ValidationParameters, out _);
            return principal;
        }
        catch (Exception)
        {
            return null;
        }
    }
}
