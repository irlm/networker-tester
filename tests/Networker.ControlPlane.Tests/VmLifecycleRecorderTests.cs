using Microsoft.Data.Sqlite;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Provisioning;
using Networker.Data;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Tests for the vm_lifecycle audit recorder ported from Rust
/// <c>vm_lifecycle_recorder.rs</c> — the happy-path insert (resource_type
/// hardcoded to 'tester', resource_id ← tester_id) and the faithful
/// swallow-on-error behavior (a failing insert never throws to the caller).
/// </summary>
public sealed class VmLifecycleRecorderTests
{
    // A relational provider is used because the code path calls SaveChanges over
    // real tables. The full Postgres model can't be built on Sqlite (Timescale
    // sequences), so only the two tables this path touches are created.
    private static (ServiceProvider Sp, SqliteConnection Conn) BuildHost()
    {
        var conn = new SqliteConnection("DataSource=:memory:");
        conn.Open();

        var services = new ServiceCollection();
        services.AddLogging(b => b.SetMinimumLevel(LogLevel.Warning));
        services.AddSingleton(conn);
        services.AddDbContext<NetworkerDbContext>(o => o.UseSqlite(conn));
        var sp = services.BuildServiceProvider();
        return (sp, conn);
    }

    private static void CreateSchema(SqliteConnection conn)
    {
        Exec(conn, "PRAGMA foreign_keys = ON;");
        Exec(conn, """
            CREATE TABLE project (
                project_id TEXT PRIMARY KEY
            );
            """);
        Exec(conn, """
            CREATE TABLE vm_lifecycle (
                event_id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL REFERENCES project(project_id),
                resource_type TEXT NOT NULL,
                resource_id TEXT NOT NULL,
                resource_name TEXT,
                cloud TEXT NOT NULL,
                region TEXT,
                vm_size TEXT,
                vm_name TEXT,
                vm_resource_id TEXT,
                cloud_connection_id TEXT,
                cloud_account_name_at_event TEXT,
                provider_account_id TEXT,
                event_type TEXT NOT NULL,
                event_time TEXT NOT NULL,
                triggered_by TEXT,
                metadata TEXT,
                created_at TEXT NOT NULL
            );
            """);
        Exec(conn, "INSERT INTO project(project_id) VALUES ('proj-1');");
    }

    private static void Exec(SqliteConnection conn, string sql)
    {
        using var cmd = conn.CreateCommand();
        cmd.CommandText = sql;
        cmd.ExecuteNonQuery();
    }

    private static TesterEventInput Input(Guid testerId) => new(
        ProjectId: "proj-1",
        TesterId: testerId,
        TesterName: "tester-a",
        Cloud: "azure",
        Region: "eastus",
        VmSize: "Standard_B2s",
        VmName: "tester-vm",
        VmResourceId: "/subscriptions/x/vm",
        CloudConnectionId: null,
        EventType: "created",
        EventTime: DateTime.UtcNow,
        TriggeredBy: null,
        Metadata: null);

    [Fact]
    public async Task Records_event_with_hardcoded_resource_type_and_tester_id()
    {
        var (sp, conn) = BuildHost();
        using var _ = sp;
        CreateSchema(conn);

        var testerId = Guid.NewGuid();
        var db = sp.GetRequiredService<NetworkerDbContext>();
        var recorder = new VmLifecycleRecorder(db, sp.GetRequiredService<ILogger<VmLifecycleRecorder>>());

        await recorder.RecordTesterEventAsync(Input(testerId));

        using var cmd = conn.CreateCommand();
        cmd.CommandText =
            "SELECT resource_type, resource_id, resource_name, event_type FROM vm_lifecycle";
        using var reader = cmd.ExecuteReader();
        Assert.True(reader.Read());
        Assert.Equal("tester", reader.GetString(0));
        Assert.Equal(testerId.ToString(), reader.GetString(1), ignoreCase: true);
        Assert.Equal("tester-a", reader.GetString(2));
        Assert.Equal("created", reader.GetString(3));
    }

    [Fact]
    public async Task Swallows_insert_failure_without_throwing()
    {
        // No schema created => the INSERT fails; the recorder must swallow it and
        // NOT throw (Rust: WARN, "user-facing op unaffected").
        var (sp, _) = BuildHost();
        using var _guard = sp;

        var db = sp.GetRequiredService<NetworkerDbContext>();
        var recorder = new VmLifecycleRecorder(db, sp.GetRequiredService<ILogger<VmLifecycleRecorder>>());

        // Should complete normally despite the missing table.
        await recorder.RecordTesterEventAsync(Input(Guid.NewGuid()));
    }
}
