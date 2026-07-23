using Microsoft.Data.Sqlite;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Networker.ControlPlane.Endpoints;
using Networker.Data;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Regression guard for the 2026-07 perf-sweep bug: the members list and the
/// non-admin projects list ordered by a property of a client-constructed record
/// (<c>MemberRow</c> / <c>ProjectListItem</c>) AFTER the Join projection, which
/// EF Core cannot translate — every load threw InvalidOperationException → 500.
///
/// These execute the extracted query builders against a real (SQLite) EF
/// provider: the untranslatable ordering throws at query-translation time on any
/// provider, so before the fix these tests would throw; after (OrderBy on the
/// entity column before the projection) they return rows in the right order.
/// </summary>
public sealed class MembersQueryTranslationTests
{
    private const string ProjectId = "proj-memq-01";

    private static ServiceProvider BuildHost()
    {
        var conn = new SqliteConnection("DataSource=:memory:");
        conn.Open();
        var services = new ServiceCollection();
        services.AddSingleton(conn);
        services.AddDbContext<NetworkerDbContext>(o => o.UseSqlite(conn));
        var sp = services.BuildServiceProvider();
        CreateSchema(conn);
        return sp;
    }

    private static void CreateSchema(SqliteConnection conn)
    {
        Exec(conn, """
            CREATE TABLE project (
                project_id TEXT PRIMARY KEY, name TEXT NOT NULL, slug TEXT NOT NULL,
                description TEXT, created_by TEXT, created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL, settings TEXT NOT NULL,
                deleted_at TEXT, delete_protection INTEGER NOT NULL DEFAULT 0
            );
            """);
        // All DashUser-mapped columns — EF's INSERT writes every mapped column,
        // so an omitted one (e.g. avatar_url) fails the seed.
        Exec(conn, """
            CREATE TABLE dash_user (
                user_id TEXT PRIMARY KEY, auth_provider TEXT, avatar_url TEXT,
                created_at TEXT NOT NULL, display_name TEXT, email TEXT,
                is_platform_admin INTEGER NOT NULL DEFAULT 0, last_login_at TEXT,
                must_change_password INTEGER NOT NULL DEFAULT 0, password_hash TEXT,
                password_reset_expires TEXT, password_reset_token TEXT,
                role TEXT NOT NULL, sso_only INTEGER NOT NULL DEFAULT 0,
                sso_subject_id TEXT, status TEXT NOT NULL
            );
            """);
        Exec(conn, """
            CREATE TABLE project_member (
                project_id TEXT NOT NULL, user_id TEXT NOT NULL, role TEXT NOT NULL,
                status TEXT NOT NULL, joined_at TEXT NOT NULL, invited_by TEXT,
                invite_sent_at TEXT, link_id TEXT,
                PRIMARY KEY (project_id, user_id)
            );
            """);
    }

    private static void Exec(SqliteConnection conn, string sql)
    {
        using var cmd = conn.CreateCommand();
        cmd.CommandText = sql;
        cmd.ExecuteNonQuery();
    }

    // Seed with raw SQL, NOT EF entities: several columns are mapped
    // ValueGenerated.OnAdd (HasDefaultValueSql), so an EF insert reads the value
    // back and throws "data is NULL at ordinal" against the minimal SQLite
    // schema. Raw INSERTs make the SELECT-under-test the only EF operation.
    private static (Guid u1, Guid u2) Seed(IServiceProvider sp)
    {
        var conn = sp.GetRequiredService<SqliteConnection>();
        var u1 = Guid.NewGuid();
        var u2 = Guid.NewGuid();
        Exec(conn, $"""
            INSERT INTO project (project_id, name, slug, settings, created_at, updated_at, delete_protection)
            VALUES ('{ProjectId}', 'Memq', 'memq', '[]', '2026-07-22 12:00:00', '2026-07-22 12:00:00', 0);
            INSERT INTO dash_user (user_id, email, display_name, role, status, created_at, is_platform_admin, must_change_password, sso_only)
            VALUES ('{u1}', 'a@x.io', 'Alice', 'operator', 'active', '2026-07-22 12:00:00', 0, 0, 0);
            INSERT INTO dash_user (user_id, email, display_name, role, status, created_at, is_platform_admin, must_change_password, sso_only)
            VALUES ('{u2}', 'b@x.io', 'Bob', 'viewer', 'active', '2026-07-22 12:00:00', 0, 0, 0);
            INSERT INTO project_member (project_id, user_id, role, status, joined_at)
            VALUES ('{ProjectId}', '{u2}', 'viewer', 'active', '2026-07-22 11:50:00');
            INSERT INTO project_member (project_id, user_id, role, status, joined_at)
            VALUES ('{ProjectId}', '{u1}', 'operator', 'active', '2026-07-22 12:00:00');
            """);
        return (u1, u2);
    }

    [Fact]
    public async Task Members_query_translates_and_orders_by_joined_at()
    {
        var sp = BuildHost();
        var (u1, u2) = Seed(sp);
        using var scope = sp.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();

        // Before the fix this throws InvalidOperationException at translation.
        var rows = await MembersEndpoints.BuildMembersQuery(db, ProjectId).ToListAsync();

        Assert.Equal(2, rows.Count);
        Assert.Equal(u2, rows[0].user_id); // earlier joined_at first
        Assert.Equal(u1, rows[1].user_id);
        Assert.Equal("Bob", rows[0].display_name);
        Assert.Equal("a@x.io", rows[1].email);
    }

    [Fact]
    public async Task Member_projects_query_translates_and_orders_by_created_at()
    {
        var sp = BuildHost();
        var (u1, _) = Seed(sp);
        using var scope = sp.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();

        // The non-admin projects branch — same bug class; must not throw.
        var rows = await ProjectsEndpoints.BuildMemberProjectsQuery(db, u1).ToListAsync();

        var row = Assert.Single(rows);
        Assert.Equal(ProjectId, row.project_id);
        Assert.Equal("operator", row.role);
    }
}
