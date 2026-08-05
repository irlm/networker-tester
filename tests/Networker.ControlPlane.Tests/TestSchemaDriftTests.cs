using Microsoft.Data.Sqlite;
using Microsoft.EntityFrameworkCore;
using Networker.Data;
using Xunit;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Audit P2: several SQLite-backed suites build their tables from HAND-WRITTEN
/// DDL. That DDL is a copy of the EF model, and copies drift — add a mapped
/// column and the copy silently lacks it.
///
/// <para>The audit suggested replacing the DDL with
/// <c>Database.GenerateCreateScript()</c>. That does not work here, and it is
/// worth recording why rather than leaving it as an open suggestion: the model
/// declares Postgres sequences, and the SQLite provider throws
/// <c>NotSupportedException: SQLite does not support sequences</c> while
/// generating. Verified, not assumed.</para>
///
/// <para>So the drift is caught directly instead. Each suite's schema is
/// created for real, and every column EF maps for those entities must exist in
/// the created table — for the tables that suite actually exercises through EF.
/// A table that exists only as a one-column foreign-key stub is deliberately
/// out of scope; comparing a stub against the full model reports drift that
/// isn't there. The failure this prevents is nasty because it is not a
/// clean error: EF's INSERT names every mapped column, so a missing one fails
/// the seed with a raw SQLite error inside an unrelated test, and a missing
/// column that is only READ produces wrong results with no error at all.</para>
/// </summary>
public sealed class TestSchemaDriftTests
{
    private static NetworkerDbContext Model(SqliteConnection conn) =>
        new(new DbContextOptionsBuilder<NetworkerDbContext>().UseSqlite(conn).Options);

    /// <summary>Column names SQLite reports for a table it actually created.</summary>
    private static HashSet<string> ActualColumns(SqliteConnection conn, string table)
    {
        var cols = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        using var cmd = conn.CreateCommand();
        cmd.CommandText = $"PRAGMA table_info({table})";
        using var reader = cmd.ExecuteReader();
        while (reader.Read())
        {
            cols.Add(reader.GetString(1));
        }
        return cols;
    }

    /// <summary>Column names EF maps for the entity behind <paramref name="table"/>.</summary>
    private static List<string> ModelColumns(NetworkerDbContext db, string table)
    {
        var entity = db.Model.GetEntityTypes()
            .FirstOrDefault(e => string.Equals(e.GetTableName(), table, StringComparison.OrdinalIgnoreCase));
        Assert.True(entity is not null,
            $"no EF entity maps to table '{table}' — the test schema builds a table the model "
            + "does not know about, or the table was renamed in the model");

        return entity!.GetProperties()
            .Select(p => p.GetColumnName())
            .Where(c => !string.IsNullOrEmpty(c))
            .ToList();
    }

    private static void AssertNoDrift(Action<SqliteConnection> createSchema, string suite, params string[] tables)
    {
        using var conn = new SqliteConnection("DataSource=:memory:");
        conn.Open();
        createSchema(conn);

        using var db = Model(conn);
        foreach (var table in tables)
        {
            var actual = ActualColumns(conn, table);
            Assert.True(actual.Count > 0,
                $"{suite}: table '{table}' was not created at all by the suite's schema builder");

            var missing = ModelColumns(db, table).Where(c => !actual.Contains(c)).ToList();
            Assert.True(missing.Count == 0,
                $"{suite}: hand-written DDL for '{table}' is missing column(s) EF maps: "
                + $"{string.Join(", ", missing)}.\n"
                + "EF's INSERT names every mapped column, so this surfaces as a raw SQLite error "
                + "in whichever test seeds that entity — and a column that is only READ drifts "
                + "silently, with no error at all. Add the column to the suite's CREATE TABLE.");
        }
    }

    [Fact]
    public void Members_suite_schema_matches_the_model()
        => AssertNoDrift(
            MembersQueryTranslationTests.CreateSchema,
            nameof(MembersQueryTranslationTests),
            "project", "dash_user", "project_member");

    [Fact]
    public void Vm_lifecycle_suite_schema_matches_the_model()
        // Only `vm_lifecycle` — that suite's `project` table is a deliberate
        // one-column FK stub, seeded with raw SQL and never materialized as an
        // EF Project, so the model's other columns are correctly absent.
        // Checking a stub against the full model would report drift that isn't.
        => AssertNoDrift(
            VmLifecycleRecorderTests.CreateSchema,
            nameof(VmLifecycleRecorderTests),
            "vm_lifecycle");

    [Fact]
    public void The_drift_check_can_actually_fail()
    {
        // Guard the guard: if ModelColumns ever returned nothing, every check
        // above would pass vacuously. Prove a deliberately incomplete table is
        // detected.
        using var conn = new SqliteConnection("DataSource=:memory:");
        conn.Open();
        using (var cmd = conn.CreateCommand())
        {
            cmd.CommandText = "CREATE TABLE project (project_id TEXT PRIMARY KEY)";
            cmd.ExecuteNonQuery();
        }

        using var db = Model(conn);
        var actual = ActualColumns(conn, "project");
        var missing = ModelColumns(db, "project").Where(c => !actual.Contains(c)).ToList();

        Assert.True(missing.Count > 0,
            "a table with a single column was reported as matching the full model — "
            + "the drift check is not looking at anything");
    }
}
