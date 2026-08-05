using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.Logging.Abstractions;
using Networker.ControlPlane.Startup;
using Npgsql;
using Xunit;

namespace Networker.Tests;

/// <summary>
/// Guards the first-admin bootstrap (<see cref="AdminBootstrap"/>).
///
/// <para>Two opposite properties have to hold at once, and both need a REAL
/// Postgres (the statement is a single <c>INSERT ... WHERE NOT EXISTS</c> —
/// SQLite would prove nothing about it):</para>
/// <list type="number">
///   <item>a fresh install with an empty <c>dash_user</c> gets exactly one
///   usable admin, otherwise the dashboard is unreachable; and</item>
///   <item>an existing deployment is <b>never</b> touched — no update, no
///   upsert, no overwrite. That is the test that stands between this feature
///   and nuking a real install's admin account.</item>
/// </list>
///
/// <para>The shared <see cref="ControlPlaneFixture"/> database already has
/// seeded users, so it is the natural "existing deployment". The empty-table
/// cases run against throwaway databases created inside the same container.</para>
/// </summary>
public sealed class AdminBootstrapTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fx;

    public AdminBootstrapTests(ControlPlaneFixture fx) => _fx = fx;

    private const string TestPassword = "bootstrap-test-pw-9f3a";

    /// <summary>Connection string of the fixture's Postgres container.</summary>
    private string FixtureConnString()
    {
        using var db = _fx.NewDbContext();
        return db.Database.GetConnectionString()
            ?? throw new InvalidOperationException("fixture DbContext has no connection string");
    }

    /// <summary>
    /// The columns the bootstrap writes, as the migration chain
    /// (V002/V004/V008/V010) leaves them. Enough of <c>dash_user</c> to exercise
    /// the real INSERT, including the UNIQUE(email) constraint.
    /// </summary>
    private const string DashUserDdl = """
        CREATE TABLE dash_user (
            user_id              uuid PRIMARY KEY,
            email                varchar(255) NOT NULL UNIQUE,
            password_hash        varchar(255),
            role                 varchar(20)  NOT NULL DEFAULT 'viewer',
            created_at           timestamptz  NOT NULL DEFAULT now(),
            last_login_at        timestamptz,
            must_change_password boolean      NOT NULL DEFAULT false,
            status               varchar(20)  NOT NULL DEFAULT 'pending',
            auth_provider        varchar(20)  NOT NULL DEFAULT 'local',
            sso_only             boolean      NOT NULL DEFAULT false,
            is_platform_admin    boolean      NOT NULL DEFAULT false
        );
        """;

    /// <summary>
    /// Create a throwaway database in the fixture's container holding nothing but
    /// an empty <c>dash_user</c> — a genuinely fresh install, without touching the
    /// shared fixture schema. Returns its connection string.
    /// </summary>
    private async Task<string> NewEmptyInstallDbAsync()
    {
        var baseConn = FixtureConnString();
        var dbName = "bootstrap_" + Guid.NewGuid().ToString("N");

        await using (var admin = new NpgsqlConnection(baseConn))
        {
            await admin.OpenAsync();
            // CREATE DATABASE can't run inside a transaction block — plain command.
            await using var create = new NpgsqlCommand($"CREATE DATABASE \"{dbName}\"", admin);
            await create.ExecuteNonQueryAsync();
        }

        var scratchConn = new NpgsqlConnectionStringBuilder(baseConn) { Database = dbName }.ConnectionString;
        await using (var conn = new NpgsqlConnection(scratchConn))
        {
            await conn.OpenAsync();
            await using var ddl = new NpgsqlCommand(DashUserDdl, conn);
            await ddl.ExecuteNonQueryAsync();
        }

        return scratchConn;
    }

    private static async Task<long> UserCountAsync(string connString)
    {
        await using var conn = new NpgsqlConnection(connString);
        await conn.OpenAsync();
        await using var cmd = new NpgsqlCommand("SELECT COUNT(*) FROM dash_user", conn);
        return (long)(await cmd.ExecuteScalarAsync())!;
    }

    /// <summary>Every user's identity + credential, for before/after comparison.</summary>
    private static async Task<List<(Guid Id, string Email, string? Hash, string Role)>> SnapshotUsersAsync(
        string connString)
    {
        var rows = new List<(Guid, string, string?, string)>();
        await using var conn = new NpgsqlConnection(connString);
        await conn.OpenAsync();
        await using var cmd = new NpgsqlCommand(
            "SELECT user_id, email, password_hash, role FROM dash_user ORDER BY user_id", conn);
        await using var reader = await cmd.ExecuteReaderAsync();
        while (await reader.ReadAsync())
        {
            rows.Add((
                reader.GetGuid(0),
                reader.GetString(1),
                reader.IsDBNull(2) ? null : reader.GetString(2),
                reader.GetString(3)));
        }
        return rows;
    }

    /// <summary>
    /// Run the bootstrap with the given env, restoring whatever was there before.
    /// </summary>
    private static async Task<string> RunBootstrapAsync(string connString, string? password, string? email = null)
    {
        var prevPw = Environment.GetEnvironmentVariable(AdminBootstrap.PasswordEnvVar);
        var prevEmail = Environment.GetEnvironmentVariable(AdminBootstrap.EmailEnvVar);
        try
        {
            Environment.SetEnvironmentVariable(AdminBootstrap.PasswordEnvVar, password);
            Environment.SetEnvironmentVariable(AdminBootstrap.EmailEnvVar, email);
            return await AdminBootstrap.EnsureBootstrapAdminAsync(connString, NullLogger.Instance);
        }
        finally
        {
            Environment.SetEnvironmentVariable(AdminBootstrap.PasswordEnvVar, prevPw);
            Environment.SetEnvironmentVariable(AdminBootstrap.EmailEnvVar, prevEmail);
        }
    }

    // ---------------------------------------------------------------- safety

    [Fact]
    public async Task Existing_deployment_is_never_touched()
    {
        // THE load-bearing test. The fixture DB is a populated install (three
        // seeded users). Give one of them a real credential so "left alone"
        // means something byte-for-byte, then run the bootstrap with a password
        // set — the shape that would overwrite an admin if the guard were wrong.
        var conn = FixtureConnString();
        var existingHash = BCrypt.Net.BCrypt.HashPassword("the-real-operators-password");
        await using (var c = new NpgsqlConnection(conn))
        {
            await c.OpenAsync();
            await using var upd = new NpgsqlCommand(
                "UPDATE dash_user SET password_hash = @h WHERE email = @e", c);
            upd.Parameters.AddWithValue("h", existingHash);
            upd.Parameters.AddWithValue("e", ControlPlaneFixture.SeededAdminEmail);
            Assert.Equal(1, await upd.ExecuteNonQueryAsync());
        }

        var before = await SnapshotUsersAsync(conn);
        Assert.NotEmpty(before);

        var status = await RunBootstrapAsync(conn, TestPassword, "attacker@localhost");

        Assert.Equal("skipped: users exist", status);

        var after = await SnapshotUsersAsync(conn);
        Assert.Equal(before, after);           // no insert, no update, no delete
        Assert.DoesNotContain(after, u => u.Email == "attacker@localhost");

        // And the pre-existing credential still verifies — the account was not
        // re-hashed under a new password.
        var admin = Assert.Single(after, u => u.Email == ControlPlaneFixture.SeededAdminEmail);
        Assert.Equal(existingHash, admin.Hash);
        Assert.NotNull(admin.Hash);
        Assert.True(BCrypt.Net.BCrypt.Verify("the-real-operators-password", admin.Hash!));
        Assert.False(BCrypt.Net.BCrypt.Verify(TestPassword, admin.Hash!));
    }

    [Fact]
    public async Task No_password_means_no_seed_even_on_an_empty_install()
    {
        var conn = await NewEmptyInstallDbAsync();
        Assert.Equal(0, await UserCountAsync(conn));

        var status = await RunBootstrapAsync(conn, password: null);

        Assert.Equal("skipped: no password", status);
        Assert.Equal(0, await UserCountAsync(conn));

        // Whitespace is not a password either.
        Assert.Equal("skipped: no password", await RunBootstrapAsync(conn, password: "   "));
        Assert.Equal(0, await UserCountAsync(conn));
    }

    // ------------------------------------------------------------ fresh install

    [Fact]
    public async Task Empty_install_gets_exactly_one_usable_admin()
    {
        var conn = await NewEmptyInstallDbAsync();

        var status = await RunBootstrapAsync(conn, TestPassword, "Ops.Admin@Example.COM  ");

        Assert.Equal("seeded", status);

        await using var c = new NpgsqlConnection(conn);
        await c.OpenAsync();
        await using var cmd = new NpgsqlCommand(
            """
            SELECT email, password_hash, role, status, auth_provider,
                   sso_only, is_platform_admin, must_change_password
            FROM dash_user
            """, c);
        await using var reader = await cmd.ExecuteReaderAsync();

        Assert.True(await reader.ReadAsync(), "expected exactly one seeded admin");
        var email = reader.GetString(0);
        var hash = reader.GetString(1);
        Assert.Equal("ops.admin@example.com", email);       // trimmed + lowercased
        Assert.Equal("admin", reader.GetString(2));
        Assert.Equal("active", reader.GetString(3));
        Assert.Equal("local", reader.GetString(4));
        Assert.False(reader.GetBoolean(5));                  // sso_only
        Assert.True(reader.GetBoolean(6));                   // is_platform_admin
        Assert.True(reader.GetBoolean(7));                   // must_change_password
        Assert.False(await reader.ReadAsync(), "more than one row was inserted");

        // The stored credential is a real BCrypt hash of the supplied password —
        // i.e. the operator can actually log in with it.
        Assert.True(BCrypt.Net.BCrypt.Verify(TestPassword, hash));
        Assert.False(BCrypt.Net.BCrypt.Verify("not-the-password", hash));
        Assert.NotEqual(TestPassword, hash);                 // never stored in the clear
    }

    [Fact]
    public async Task Defaults_the_email_when_only_a_password_is_supplied()
    {
        var conn = await NewEmptyInstallDbAsync();

        Assert.Equal("seeded", await RunBootstrapAsync(conn, TestPassword, email: null));

        await using var c = new NpgsqlConnection(conn);
        await c.OpenAsync();
        await using var cmd = new NpgsqlCommand("SELECT email FROM dash_user", c);
        Assert.Equal(AdminBootstrap.DefaultEmail, (string)(await cmd.ExecuteScalarAsync())!);
    }

    [Fact]
    public async Task Running_twice_yields_exactly_one_user()
    {
        var conn = await NewEmptyInstallDbAsync();

        Assert.Equal("seeded", await RunBootstrapAsync(conn, TestPassword));
        var afterFirst = await SnapshotUsersAsync(conn);

        // Second boot of the same install — must be a pure no-op, including on
        // the credential (a re-hash would invalidate a password the operator
        // may already have changed).
        Assert.Equal("skipped: users exist", await RunBootstrapAsync(conn, "a-completely-different-pw"));

        var afterSecond = await SnapshotUsersAsync(conn);
        Assert.Single(afterSecond);
        Assert.Equal(afterFirst, afterSecond);
    }

    [Fact]
    public async Task Concurrent_starters_cannot_create_two_admins()
    {
        // Two control-plane instances booting at once against the same empty
        // database: the single-statement INSERT ... WHERE NOT EXISTS under an
        // advisory lock must let exactly one through.
        var conn = await NewEmptyInstallDbAsync();

        var prevPw = Environment.GetEnvironmentVariable(AdminBootstrap.PasswordEnvVar);
        var prevEmail = Environment.GetEnvironmentVariable(AdminBootstrap.EmailEnvVar);
        string[] statuses;
        try
        {
            Environment.SetEnvironmentVariable(AdminBootstrap.PasswordEnvVar, TestPassword);
            Environment.SetEnvironmentVariable(AdminBootstrap.EmailEnvVar, null);
            statuses = await Task.WhenAll(
                Enumerable.Range(0, 4).Select(_ =>
                    AdminBootstrap.EnsureBootstrapAdminAsync(conn, NullLogger.Instance)));
        }
        finally
        {
            Environment.SetEnvironmentVariable(AdminBootstrap.PasswordEnvVar, prevPw);
            Environment.SetEnvironmentVariable(AdminBootstrap.EmailEnvVar, prevEmail);
        }

        Assert.Equal(1, statuses.Count(s => s == "seeded"));
        Assert.Equal(1, await UserCountAsync(conn));
    }

    // -------------------------------------------------------------- fail-safe

    [Fact]
    public async Task Bootstrap_failure_never_blocks_startup()
    {
        // Unreachable database: must degrade to a status string, not an
        // exception that would take the whole control plane down on boot.
        var status = await RunBootstrapAsync(
            "Host=127.0.0.1;Port=1;Database=nope;Username=nope;Password=nope;Timeout=2",
            TestPassword);

        Assert.Equal("skipped: error", status);
    }
}
