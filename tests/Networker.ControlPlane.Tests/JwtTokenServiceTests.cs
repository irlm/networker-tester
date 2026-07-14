using System.IdentityModel.Tokens.Jwt;
using Networker.ControlPlane.Auth;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Locks the C# JWT scheme to the Rust dashboard's (jsonwebtoken crate):
/// HS256, raw-UTF-8 DASHBOARD_JWT_SECRET signing key, claims
/// { sub, email, role, is_platform_admin, exp, iat }, 24h TTL. A drift here
/// means Rust-minted and C#-minted tokens stop being interchangeable.
/// </summary>
public class JwtTokenServiceTests
{
    // Same length/shape as the Rust unit-test secret.
    private const string Secret = "test-secret-at-least-32-bytes-long!!";

    private static JwtTokenService NewService() => new(Secret);

    [Fact]
    public void Roundtrip_MintThenValidate_PreservesClaims()
    {
        var svc = NewService();
        var uid = Guid.NewGuid();

        var token = svc.CreateToken(uid, "alice@test.com", "operator", isPlatformAdmin: false);
        var principal = svc.Validate(token);

        Assert.NotNull(principal);
        Assert.Equal(uid.ToString(), principal!.FindFirst(JwtTokenService.SubClaim)?.Value);
        Assert.Equal("alice@test.com", principal.FindFirst(JwtTokenService.EmailClaim)?.Value);
        Assert.Equal("operator", principal.FindFirst(JwtTokenService.RoleClaim)?.Value);

        var authUser = AuthUser.FromPrincipal(principal);
        Assert.NotNull(authUser);
        Assert.Equal(uid, authUser!.UserId);
        Assert.Equal("operator", authUser.Role);
        Assert.False(authUser.IsPlatformAdmin);
    }

    [Fact]
    public void Header_Algorithm_IsHs256_TypJwt()
    {
        var svc = NewService();
        var token = svc.CreateToken(Guid.NewGuid(), "a@b.com", "admin", true);

        var jwt = new JwtSecurityTokenHandler().ReadJwtToken(token);
        Assert.Equal("HS256", jwt.Header.Alg);
        Assert.Equal("JWT", jwt.Header.Typ);
    }

    [Fact]
    public void Claims_MatchRustScheme_TypesAndNames()
    {
        var svc = NewService();
        var uid = Guid.NewGuid();
        var token = svc.CreateToken(uid, "admin@test.com", "admin", isPlatformAdmin: true);

        var jwt = new JwtSecurityTokenHandler().ReadJwtToken(token);

        // sub is the UUID as a string (Rust Uuid serde form).
        Assert.Equal(uid.ToString(), jwt.Payload[JwtTokenService.SubClaim]);
        Assert.Equal("admin@test.com", jwt.Payload[JwtTokenService.EmailClaim]);
        Assert.Equal("admin", jwt.Payload[JwtTokenService.RoleClaim]);

        // is_platform_admin must be a JSON boolean, not the string "true".
        Assert.Equal(true, jwt.Payload[JwtTokenService.PlatformAdminClaim]);

        // exp/iat present as numeric unix seconds; exp = iat + 24h.
        var iat = Convert.ToInt64(jwt.Payload["iat"]);
        var exp = Convert.ToInt64(jwt.Payload["exp"]);
        Assert.Equal(JwtTokenService.TokenTtlSeconds, exp - iat);
        Assert.Equal(24 * 3600, exp - iat);

        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        Assert.True(Math.Abs(now - iat) < 5, "iat should be within 5s of now");
    }

    [Fact]
    public void PlatformAdmin_Flag_Roundtrips()
    {
        var svc = NewService();
        var token = svc.CreateToken(Guid.NewGuid(), "admin@test.com", "admin", isPlatformAdmin: true);

        var principal = svc.Validate(token);
        var authUser = AuthUser.FromPrincipal(principal);

        Assert.NotNull(authUser);
        Assert.True(authUser!.IsPlatformAdmin);
    }

    [Fact]
    public void Validate_RejectsWrongSecret()
    {
        var token = NewService().CreateToken(Guid.NewGuid(), "a@b.com", "viewer", false);
        var other = new JwtTokenService("wrong-secret-xxxxxxxxxxxxxxxxxxxxxxx");

        Assert.Null(other.Validate(token));
    }

    [Theory]
    [InlineData("not.a.jwt")]
    [InlineData("")]
    public void Validate_RejectsGarbageAndEmpty(string bad)
    {
        Assert.Null(NewService().Validate(bad));
    }

    [Fact]
    public void Validate_RejectsExpiredToken()
    {
        // Sign a token that expired well beyond the 60s clock skew.
        var svc = NewService();
        var handler = new JwtSecurityTokenHandler();
        var creds = new Microsoft.IdentityModel.Tokens.SigningCredentials(
            svc.SigningKey, Microsoft.IdentityModel.Tokens.SecurityAlgorithms.HmacSha256);
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        var token = new JwtSecurityToken(signingCredentials: creds);
        token.Payload[JwtTokenService.SubClaim] = Guid.NewGuid().ToString();
        token.Payload[JwtTokenService.EmailClaim] = "expired@test.com";
        token.Payload[JwtTokenService.RoleClaim] = "admin";
        token.Payload["iat"] = now - 7200;
        token.Payload["exp"] = now - 3600; // expired 1h ago
        var written = handler.WriteToken(token);

        Assert.Null(svc.Validate(written));
    }

    [Fact]
    public void RoleHierarchy_MatchesRust()
    {
        Assert.True(Role.Admin.HasPermission(Role.Admin));
        Assert.True(Role.Admin.HasPermission(Role.Operator));
        Assert.True(Role.Admin.HasPermission(Role.Viewer));

        Assert.False(Role.Operator.HasPermission(Role.Admin));
        Assert.True(Role.Operator.HasPermission(Role.Operator));
        Assert.True(Role.Operator.HasPermission(Role.Viewer));

        Assert.False(Role.Viewer.HasPermission(Role.Admin));
        Assert.False(Role.Viewer.HasPermission(Role.Operator));
        Assert.True(Role.Viewer.HasPermission(Role.Viewer));

        // Unknown role fails closed to Viewer, matching Rust require_role.
        Assert.Equal(Role.Viewer, RoleExtensions.ParseRoleOrViewer("superadmin"));
        Assert.Equal(Role.Viewer, RoleExtensions.ParseRoleOrViewer(""));
        Assert.Equal(Role.Admin, RoleExtensions.ParseRoleOrViewer("admin"));
    }

    [Fact]
    public void ProjectRoleHierarchy_MatchesRust()
    {
        Assert.True(ProjectRole.Admin.HasPermission(ProjectRole.Operator));
        Assert.False(ProjectRole.Operator.HasPermission(ProjectRole.Admin));
        Assert.True(ProjectRole.Operator.HasPermission(ProjectRole.Viewer));
        Assert.Null(RoleExtensions.ParseProjectRole("nonsense"));
        Assert.Equal(ProjectRole.Operator, RoleExtensions.ParseProjectRole("operator"));
    }

    [Fact]
    public void Role_WireForm_IsLowercase()
    {
        Assert.Equal("admin", Role.Admin.ToWire());
        Assert.Equal("operator", Role.Operator.ToWire());
        Assert.Equal("viewer", Role.Viewer.ToWire());
    }
}
