namespace Networker.ControlPlane.Auth;

/// <summary>
/// Global RBAC role, mirroring the Rust dashboard's <c>auth::Role</c> enum.
///
/// Serialized/stored as a lowercase string ("admin" | "operator" | "viewer"),
/// exactly matching Rust's <c>#[serde(rename_all = "lowercase")]</c> so the
/// value carried in a JWT's <c>role</c> claim is interchangeable across the two
/// implementations. Hierarchy: Admin &gt; Operator &gt; Viewer.
/// </summary>
public enum Role
{
    Viewer = 0,
    Operator = 1,
    Admin = 2,
}

/// <summary>
/// Project-scoped role, mirroring the Rust dashboard's <c>auth::ProjectRole</c>.
/// Stored in <c>project_member.role</c> as a lowercase string. Same ordering as
/// <see cref="Role"/>: Admin &gt; Operator &gt; Viewer.
/// </summary>
public enum ProjectRole
{
    Viewer = 0,
    Operator = 1,
    Admin = 2,
}

public static class RoleExtensions
{
    /// <summary>
    /// True when this role has at least the permissions of <paramref name="required"/>.
    /// Matches Rust's <c>Role::has_permission</c> (Admin ⊇ Operator ⊇ Viewer).
    /// </summary>
    public static bool HasPermission(this Role self, Role required) => self >= required;

    /// <summary>Matches Rust's <c>ProjectRole::has_permission</c> (ordinal &gt;=).</summary>
    public static bool HasPermission(this ProjectRole self, ProjectRole required) => self >= required;

    /// <summary>Lowercase wire/DB form ("admin" | "operator" | "viewer").</summary>
    public static string ToWire(this Role self) => self switch
    {
        Role.Admin => "admin",
        Role.Operator => "operator",
        _ => "viewer",
    };

    /// <summary>Lowercase wire/DB form ("admin" | "operator" | "viewer").</summary>
    public static string ToWire(this ProjectRole self) => self switch
    {
        ProjectRole.Admin => "admin",
        ProjectRole.Operator => "operator",
        _ => "viewer",
    };

    /// <summary>
    /// Parse a role string. Unknown values fail closed to <see cref="Role.Viewer"/>,
    /// matching the Rust <c>require_role</c> path which defaults unknown roles to Viewer.
    /// </summary>
    public static Role ParseRoleOrViewer(string? role) => role switch
    {
        "admin" => Role.Admin,
        "operator" => Role.Operator,
        _ => Role.Viewer,
    };

    /// <summary>Parse a project role string; returns null for unknown values (not a member).</summary>
    public static ProjectRole? ParseProjectRole(string? role) => role switch
    {
        "admin" => ProjectRole.Admin,
        "operator" => ProjectRole.Operator,
        "viewer" => ProjectRole.Viewer,
        _ => null,
    };
}
