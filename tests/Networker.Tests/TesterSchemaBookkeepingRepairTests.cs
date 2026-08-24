using Networker.ControlPlane.Realtime.RawWs;
using Npgsql;
using Testcontainers.PostgreSql;

namespace Networker.Tests;

/// <summary>
/// The tester probe schema has TWO legitimate writers — the Rust tester's
/// <c>PostgresBackend::migrate()</c> and <see cref="AttemptPersister"/> — and
/// both record progress in <c>_schema_versions(version VARCHAR)</c> as rows
/// <c>'V001'</c>.. .
///
/// <para>Until v0.28.292 there was a THIRD, hand-maintained copy of the DDL
/// inside install.sh, and it created that table as <c>version INTEGER</c> with
/// the row <c>1</c>. On any host installed that way the bookkeeping INSERT threw
/// 22P02 ("invalid input syntax for type integer: V001"), the caller's catch
/// latched <c>_schemaState = -1</c>, and the control plane silently stopped
/// ensuring the tester schema for the rest of the process — degrading every
/// streamed attempt. install.sh no longer carries that copy; these tests pin the
/// repair that heals databases it already created.</para>
///
/// <para>Deliberately exercises <see cref="AttemptPersister.RepairVersionBookkeepingAsync"/>
/// directly rather than through the ensure path: that path latches a static
/// probed once per process, so a test driving it would depend on which other
/// test class xUnit happened to run first.</para>
/// </summary>
public sealed class TesterSchemaBookkeepingRepairTests : IAsyncLifetime
{
    private readonly PostgreSqlContainer _db = new PostgreSqlBuilder("postgres:16-alpine")
        .WithDatabase("tester_bookkeeping")
        .WithUsername("networker")
        .WithPassword("networker")
        .Build();

    public async Task InitializeAsync() => await _db.StartAsync();
    public async Task DisposeAsync() => await _db.DisposeAsync();

    private async Task<NpgsqlConnection> OpenAsync()
    {
        var conn = new NpgsqlConnection(_db.GetConnectionString());
        await conn.OpenAsync();
        return conn;
    }

    private static async Task<string?> VersionColumnTypeAsync(NpgsqlConnection conn)
    {
        await using var cmd = new NpgsqlCommand(
            "SELECT data_type FROM information_schema.columns "
            + "WHERE table_name='_schema_versions' AND column_name='version'", conn);
        return (string?)await cmd.ExecuteScalarAsync();
    }

    private static async Task<long> RowCountAsync(NpgsqlConnection conn)
    {
        await using var cmd = new NpgsqlCommand("SELECT count(*) FROM _schema_versions", conn);
        return (long)(await cmd.ExecuteScalarAsync())!;
    }

    [Fact]
    public async Task An_integer_bookkeeping_table_from_the_old_installer_is_dropped()
    {
        await using var conn = await OpenAsync();
        // Exactly what install.sh used to write.
        await using (var seed = new NpgsqlCommand(
            "CREATE TABLE _schema_versions (version INTEGER PRIMARY KEY, applied_at TIMESTAMPTZ DEFAULT now());"
            + "INSERT INTO _schema_versions (version) VALUES (1) ON CONFLICT DO NOTHING;", conn))
        {
            await seed.ExecuteNonQueryAsync();
        }
        Assert.Equal("integer", await VersionColumnTypeAsync(conn));

        await AttemptPersister.RepairVersionBookkeepingAsync(conn, null, CancellationToken.None);

        Assert.Null(await VersionColumnTypeAsync(conn));   // table gone, ready to be recreated
    }

    [Fact]
    public async Task After_the_repair_the_real_bookkeeping_insert_succeeds()
    {
        await using var conn = await OpenAsync();
        await using (var seed = new NpgsqlCommand(
            "CREATE TABLE _schema_versions (version INTEGER PRIMARY KEY, applied_at TIMESTAMPTZ DEFAULT now());"
            + "INSERT INTO _schema_versions (version) VALUES (1);", conn))
        {
            await seed.ExecuteNonQueryAsync();
        }

        // Without the repair this throws 22P02 — the whole bug.
        await AttemptPersister.RepairVersionBookkeepingAsync(conn, null, CancellationToken.None);
        await using (var real = new NpgsqlCommand(
            "CREATE TABLE IF NOT EXISTS _schema_versions (version VARCHAR(20) NOT NULL PRIMARY KEY, applied_at TIMESTAMPTZ NOT NULL DEFAULT now());"
            + "INSERT INTO _schema_versions (version) VALUES ('V001'),('V008') ON CONFLICT DO NOTHING;", conn))
        {
            await real.ExecuteNonQueryAsync();
        }

        Assert.Equal("character varying", await VersionColumnTypeAsync(conn));
        Assert.Equal(2, await RowCountAsync(conn));
    }

    [Fact]
    public async Task A_correctly_typed_table_with_real_versions_is_LEFT_ALONE()
    {
        // The safety property. A tester-created table holds genuine migration
        // history; dropping it would make the tester re-run every migration and
        // would lose the record of what was applied.
        await using var conn = await OpenAsync();
        await using (var seed = new NpgsqlCommand(
            "CREATE TABLE _schema_versions (version VARCHAR(20) NOT NULL PRIMARY KEY, applied_at TIMESTAMPTZ NOT NULL DEFAULT now());"
            + "INSERT INTO _schema_versions (version) VALUES ('V001'),('V002'),('V003'),('V004'),('V005'),('V006'),('V007');", conn))
        {
            await seed.ExecuteNonQueryAsync();
        }

        await AttemptPersister.RepairVersionBookkeepingAsync(conn, null, CancellationToken.None);

        Assert.Equal("character varying", await VersionColumnTypeAsync(conn));
        Assert.Equal(7, await RowCountAsync(conn));   // untouched
    }

    [Fact]
    public async Task A_database_with_no_bookkeeping_table_at_all_is_a_no_op()
    {
        await using var conn = await OpenAsync();
        Assert.Null(await VersionColumnTypeAsync(conn));
        await AttemptPersister.RepairVersionBookkeepingAsync(conn, null, CancellationToken.None);
        Assert.Null(await VersionColumnTypeAsync(conn));
    }
}
