using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// The role whitelist shared by invite / approve / set-role — mirrors the Rust
/// <c>api::users</c> role_validation test module.
/// </summary>
public class UsersRoleValidationTests
{
    [Theory]
    [InlineData("admin")]
    [InlineData("operator")]
    [InlineData("viewer")]
    public void Whitelist_contains_expected_roles(string role)
    {
        Assert.True(UsersEndpoints.IsValidRole(role));
    }

    [Fact]
    public void Whitelist_has_exactly_three_roles()
    {
        Assert.Equal(3, UsersEndpoints.ValidRoles.Length);
    }

    [Theory]
    [InlineData("Admin")]
    [InlineData("ADMIN")]
    [InlineData("superadmin")]
    [InlineData("root")]
    [InlineData("")]
    [InlineData(" ")]
    [InlineData("moderator")]
    [InlineData(" admin")]
    [InlineData(null)]
    public void Invalid_roles_are_rejected(string? role)
    {
        Assert.False(UsersEndpoints.IsValidRole(role));
    }
}
