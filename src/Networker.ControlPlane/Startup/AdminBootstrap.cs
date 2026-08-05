using Npgsql;

namespace Networker.ControlPlane.Startup;

/// <summary>
/// First-admin bootstrap for fresh self-hosted installs.
///
/// <para>The control plane has no signup route: <c>/api/auth/login</c> verifies a
/// BCrypt hash out of <c>dash_user</c>, and every production admin row was
/// inherited from the Rust-era database. A brand-new install therefore came up
/// with an EMPTY <c>dash_user</c> and no possible way in — the dashboard was
/// unusable until somebody hand-crafted a row with psql.</para>
///
/// <para>This closes that hole with the narrowest safe rule: seed exactly one
/// admin, and only when <c>dash_user</c> is <b>completely empty</b>. That single
/// condition is the whole safety story — an existing deployment (prod has users)
/// is a permanent no-op, so this can never update, overwrite, or resurrect an
/// account. There is no upsert path here by design.</para>
///
/// <para>Opt-in: nothing happens unless <c>DASHBOARD_ADMIN_PASSWORD</c> is set.
/// A password is never invented, never defaulted, and never logged.</para>
///
/// <para>Deliberately raw Npgsql (matching <c>Auth/AuthRepository</c>) with
/// parameterised values — the table is owned by the V0NN migration chain
/// (<c>V002</c>/<c>V008</c>/<c>V010</c>), so no EF migration is involved.</para>
/// </summary>
public static class AdminBootstrap
{
    /// <summary>Env var holding the initial admin password. Unset ⇒ no-op.</summary>
    public const string PasswordEnvVar = "DASHBOARD_ADMIN_PASSWORD";

    /// <summary>Env var holding the initial admin email.</summary>
    public const string EmailEnvVar = "DASHBOARD_ADMIN_EMAIL";

    /// <summary>Email used when <see cref="EmailEnvVar"/> is unset.</summary>
    public const string DefaultEmail = "admin@localhost";

    /// <summary>
    /// Advisory-lock key so two control-plane instances booting simultaneously
    /// against the same database can't both pass the emptiness check. Arbitrary
    /// but stable — only this routine takes it.
    /// </summary>
    private const long BootstrapLockKey = 823_141_001L;

    /// <summary>
    /// Seed the first admin if — and only if — <c>dash_user</c> is empty and an
    /// initial password was supplied.
    /// </summary>
    /// <returns>
    /// A short status for the startup log: <c>"seeded"</c>,
    /// <c>"skipped: no password"</c>, <c>"skipped: users exist"</c>, or
    /// <c>"skipped: error"</c>. Never throws — a bootstrap problem must not stop
    /// the control plane from starting.
    /// </returns>
    public static async Task<string> EnsureBootstrapAdminAsync(
        string connString,
        ILogger logger,
        CancellationToken ct = default)
    {
        try
        {
            var password = Environment.GetEnvironmentVariable(PasswordEnvVar);
            if (string.IsNullOrWhiteSpace(password))
            {
                // No password ⇒ nothing to do. We never invent one: a guessable
                // default admin credential would be worse than no admin at all.
                return "skipped: no password";
            }

            var email = Environment.GetEnvironmentVariable(EmailEnvVar);
            email = string.IsNullOrWhiteSpace(email) ? DefaultEmail : email;
            email = email.Trim().ToLowerInvariant();

            var hash = BCrypt.Net.BCrypt.HashPassword(password);

            await using var conn = new NpgsqlConnection(connString);
            await conn.OpenAsync(ct);
            await using var tx = await conn.BeginTransactionAsync(ct);

            // Serialize concurrent starters; released with the transaction.
            await using (var lockCmd = new NpgsqlCommand(
                "SELECT pg_advisory_xact_lock(@key)", conn, tx))
            {
                lockCmd.Parameters.AddWithValue("key", BootstrapLockKey);
                await lockCmd.ExecuteNonQueryAsync(ct);
            }

            // Single-statement guard: the emptiness check and the insert are the
            // same statement, so there is no window between them. Rowcount 0
            // means the table already had a user — i.e. a real deployment.
            const string sql = """
                INSERT INTO dash_user (
                    user_id, email, password_hash, role, status, auth_provider,
                    sso_only, is_platform_admin, must_change_password, created_at)
                SELECT @id, @email, @hash, 'admin', 'active', 'local',
                       FALSE, TRUE, TRUE, NOW()
                WHERE NOT EXISTS (SELECT 1 FROM dash_user)
                """;

            int inserted;
            await using (var cmd = new NpgsqlCommand(sql, conn, tx))
            {
                cmd.Parameters.AddWithValue("id", Guid.NewGuid());
                cmd.Parameters.AddWithValue("email", email);
                cmd.Parameters.AddWithValue("hash", hash);
                inserted = await cmd.ExecuteNonQueryAsync(ct);
            }

            await tx.CommitAsync(ct);

            if (inserted == 0)
            {
                return "skipped: users exist";
            }

            // Email only — the password is never logged, anywhere.
            logger.LogInformation(
                "Bootstrap admin seeded: {Email} (must change password on first login). "
                + "dash_user was empty; this runs only on a fresh install.",
                email);
            return "seeded";
        }
        catch (Exception ex)
        {
            // Fail-safe: a bootstrap failure degrades to "no admin seeded", it
            // never blocks startup.
            logger.LogWarning(ex, "Bootstrap admin check failed; continuing startup without seeding");
            return "skipped: error";
        }
    }
}
