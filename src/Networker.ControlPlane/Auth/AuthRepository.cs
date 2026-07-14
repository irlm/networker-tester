using Npgsql;

namespace Networker.ControlPlane.Auth;

/// <summary>
/// Raw-SQL reads of <c>dash_user</c> and <c>project_member</c> via Npgsql.
///
/// Deliberately EF-free: this stays decoupled from the parallel EF-model work by
/// using an <see cref="NpgsqlDataSource"/> built from the same connection string
/// the ControlPlane already resolves (see Program.cs <c>connString</c>). It
/// depends on no <c>Networker.Data</c> entities, so it neither drives nor is
/// broken by schema-model changes there.
///
/// The queries are the same the Rust dashboard issues (<c>db::users::authenticate</c>,
/// the <c>require_auth</c> status query, and <c>db::projects::get_member_role</c>).
/// </summary>
public sealed class AuthRepository(NpgsqlDataSource dataSource)
{
    public sealed record AuthCandidate(
        Guid UserId,
        string Email,
        string? PasswordHash,
        string Role,
        string Status,
        bool MustChangePassword,
        bool SsoOnly,
        bool IsPlatformAdmin);

    public sealed record UserStatusRow(
        string Status,
        string Role,
        bool MustChangePassword,
        bool IsPlatformAdmin);

    public sealed record ProfileRow(string Email, string Role, string Status);

    /// <summary>
    /// Fetch the login candidate by email. Mirrors the SELECT in Rust
    /// <c>db::users::authenticate</c>. Password verification (bcrypt) and the
    /// active/sso_only gating happen in the login endpoint, matching Rust.
    /// </summary>
    public async Task<AuthCandidate?> FindByEmailForLoginAsync(string email, CancellationToken ct = default)
    {
        const string sql = """
            SELECT user_id, email, password_hash, role, status,
                   must_change_password, sso_only, is_platform_admin
            FROM dash_user WHERE email = @email
            """;

        await using var cmd = dataSource.CreateCommand(sql);
        cmd.Parameters.AddWithValue("email", email);
        await using var reader = await cmd.ExecuteReaderAsync(ct);
        if (!await reader.ReadAsync(ct))
        {
            return null;
        }

        return new AuthCandidate(
            UserId: reader.GetGuid(0),
            Email: reader.GetString(1),
            PasswordHash: reader.IsDBNull(2) ? null : reader.GetString(2),
            Role: reader.GetString(3),
            Status: reader.GetString(4),
            MustChangePassword: !reader.IsDBNull(5) && reader.GetBoolean(5),
            SsoOnly: !reader.IsDBNull(6) && reader.GetBoolean(6),
            IsPlatformAdmin: !reader.IsDBNull(7) && reader.GetBoolean(7));
    }

    /// <summary>Record a successful login timestamp (Rust does this in authenticate()).</summary>
    public async Task TouchLastLoginAsync(Guid userId, CancellationToken ct = default)
    {
        await using var cmd = dataSource.CreateCommand(
            "UPDATE dash_user SET last_login_at = now() WHERE user_id = @id");
        cmd.Parameters.AddWithValue("id", userId);
        await cmd.ExecuteNonQueryAsync(ct);
    }

    /// <summary>
    /// Re-read status/role/must_change_password/is_platform_admin for a user.
    /// Mirrors the single query the Rust <c>require_auth</c> middleware runs on
    /// every request. Returns null if the user no longer exists.
    /// </summary>
    public async Task<UserStatusRow?> GetUserStatusAsync(Guid userId, CancellationToken ct = default)
    {
        const string sql = """
            SELECT status, role, must_change_password, is_platform_admin
            FROM dash_user WHERE user_id = @id
            """;

        await using var cmd = dataSource.CreateCommand(sql);
        cmd.Parameters.AddWithValue("id", userId);
        await using var reader = await cmd.ExecuteReaderAsync(ct);
        if (!await reader.ReadAsync(ct))
        {
            return null;
        }

        return new UserStatusRow(
            Status: reader.GetString(0),
            Role: reader.GetString(1),
            MustChangePassword: !reader.IsDBNull(2) && reader.GetBoolean(2),
            IsPlatformAdmin: !reader.IsDBNull(3) && reader.GetBoolean(3));
    }

    /// <summary>Profile info for GET /auth/profile (email + status).</summary>
    public async Task<ProfileRow?> GetProfileAsync(Guid userId, CancellationToken ct = default)
    {
        const string sql = "SELECT email, role, status FROM dash_user WHERE user_id = @id";
        await using var cmd = dataSource.CreateCommand(sql);
        cmd.Parameters.AddWithValue("id", userId);
        await using var reader = await cmd.ExecuteReaderAsync(ct);
        if (!await reader.ReadAsync(ct))
        {
            return null;
        }

        return new ProfileRow(reader.GetString(0), reader.GetString(1), reader.GetString(2));
    }

    /// <summary>
    /// Resolve the caller's project role from <c>project_member</c>. Mirrors Rust
    /// <c>db::projects::get_member_role</c>. Returns null when the user is not a member.
    /// </summary>
    public async Task<ProjectRole?> GetMemberRoleAsync(string projectId, Guid userId, CancellationToken ct = default)
    {
        const string sql =
            "SELECT role FROM project_member WHERE project_id = @pid AND user_id = @uid";
        await using var cmd = dataSource.CreateCommand(sql);
        cmd.Parameters.AddWithValue("pid", projectId);
        cmd.Parameters.AddWithValue("uid", userId);
        var role = await cmd.ExecuteScalarAsync(ct) as string;
        return RoleExtensions.ParseProjectRole(role);
    }

    /// <summary>
    /// Whether a project exists and is not soft-deleted. Platform admins may bypass
    /// the soft-delete gate (matches the Rust require_project logic).
    /// </summary>
    public async Task<(bool Exists, bool Deleted)> GetProjectStateAsync(string projectId, CancellationToken ct = default)
    {
        const string sql = "SELECT deleted_at FROM project WHERE project_id = @pid";
        await using var cmd = dataSource.CreateCommand(sql);
        cmd.Parameters.AddWithValue("pid", projectId);
        await using var reader = await cmd.ExecuteReaderAsync(ct);
        if (!await reader.ReadAsync(ct))
        {
            return (false, false);
        }

        var deleted = !reader.IsDBNull(0);
        return (true, deleted);
    }
}
