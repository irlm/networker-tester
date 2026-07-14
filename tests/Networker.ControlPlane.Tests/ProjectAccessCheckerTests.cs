using Networker.ControlPlane.Auth;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Locks the pure decision core of <see cref="ProjectAccessChecker"/> — the
/// row-level authorization rules shared by the {projectId} policy handler and
/// the flat v2 routes (GET /api/v2/test-runs/{id} etc.). A regression here is a
/// cross-project IDOR.
/// </summary>
public class ProjectAccessCheckerTests
{
    [Fact]
    public void MissingProject_YieldsNoRole_EvenForPlatformAdmin()
    {
        Assert.Null(ProjectAccessChecker.ResolveEffectiveRole(
            projectExists: false, projectDeleted: false, isPlatformAdmin: true, memberRole: null));
        Assert.Null(ProjectAccessChecker.ResolveEffectiveRole(
            projectExists: false, projectDeleted: false, isPlatformAdmin: false, memberRole: ProjectRole.Admin));
    }

    [Fact]
    public void SoftDeletedProject_DeniesNonAdmins_AllowsPlatformAdmin()
    {
        // Member of a soft-deleted project → no access.
        Assert.Null(ProjectAccessChecker.ResolveEffectiveRole(
            projectExists: true, projectDeleted: true, isPlatformAdmin: false, memberRole: ProjectRole.Admin));

        // Platform admin bypasses the soft-delete gate with implicit Admin.
        Assert.Equal(ProjectRole.Admin, ProjectAccessChecker.ResolveEffectiveRole(
            projectExists: true, projectDeleted: true, isPlatformAdmin: true, memberRole: null));
    }

    [Fact]
    public void PlatformAdmin_GetsImplicitAdmin_WithoutMembership()
    {
        Assert.Equal(ProjectRole.Admin, ProjectAccessChecker.ResolveEffectiveRole(
            projectExists: true, projectDeleted: false, isPlatformAdmin: true, memberRole: null));
    }

    [Fact]
    public void NonMember_GetsNoRole()
    {
        Assert.Null(ProjectAccessChecker.ResolveEffectiveRole(
            projectExists: true, projectDeleted: false, isPlatformAdmin: false, memberRole: null));
    }

    [Theory]
    [InlineData(ProjectRole.Viewer)]
    [InlineData(ProjectRole.Operator)]
    [InlineData(ProjectRole.Admin)]
    public void Member_GetsTheirMembershipRole(ProjectRole memberRole)
    {
        Assert.Equal(memberRole, ProjectAccessChecker.ResolveEffectiveRole(
            projectExists: true, projectDeleted: false, isPlatformAdmin: false, memberRole: memberRole));
    }

    [Fact]
    public void RoleHierarchy_ViewerCannotOperate_OperatorCannotAdmin()
    {
        // The endpoints combine ResolveEffectiveRole with HasPermission — pin
        // the hierarchy the flat routes rely on (Viewer for reads, Operator for
        // launch).
        Assert.True(ProjectRole.Viewer.HasPermission(ProjectRole.Viewer));
        Assert.False(ProjectRole.Viewer.HasPermission(ProjectRole.Operator));
        Assert.True(ProjectRole.Operator.HasPermission(ProjectRole.Viewer));
        Assert.False(ProjectRole.Operator.HasPermission(ProjectRole.Admin));
        Assert.True(ProjectRole.Admin.HasPermission(ProjectRole.Operator));
    }
}
