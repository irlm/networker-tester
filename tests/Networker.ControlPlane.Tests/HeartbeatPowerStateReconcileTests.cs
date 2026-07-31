using Microsoft.Data.Sqlite;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Realtime;
using Networker.ControlPlane.Realtime.RawWs;
using Networker.Data;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// A live agent heartbeat must reconcile a STALE power_state. After an Azure
/// auto-shutdown+restart the VM returns and the systemd agent reconnects, but
/// nothing flipped power_state off the 'stopped' the AutoShutdownService wrote
/// — so the UI showed a running runner as stopped and the upgrade path no-op'd
/// (2026-07-31). OnHeartbeat now flips a settled 'stopped'/'stopping' back to
/// 'running' and clears the auto-shutdown status message; a 'running' row is
/// left untouched (no needless write on steady-state beats). SQLite (relational)
/// is used because the handler runs ExecuteUpdateAsync, unsupported by InMemory.
/// </summary>
public sealed class HeartbeatPowerStateReconcileTests
{
    private static ServiceProvider BuildHost()
    {
        var conn = new SqliteConnection("DataSource=:memory:");
        conn.Open();
        var services = new ServiceCollection();
        services.AddLogging(b => b.SetMinimumLevel(LogLevel.Warning));
        services.AddSignalR();
        services.AddAgentProtocol();
        services.AddDashboardEventBus();
        services.AddSingleton(conn);
        services.AddDbContext<NetworkerDbContext>(o => o.UseSqlite(conn));
        var sp = services.BuildServiceProvider();

        Exec(conn, """
            CREATE TABLE agent (
                agent_id TEXT PRIMARY KEY, name TEXT NOT NULL, region TEXT, provider TEXT,
                status TEXT NOT NULL, version TEXT, os TEXT, arch TEXT, last_heartbeat TEXT,
                registered_at TEXT NOT NULL, api_key_hash TEXT, api_key_expires_at TEXT,
                api_key_last_used_at TEXT, api_key_last_used_ip TEXT, tags TEXT,
                project_id TEXT NOT NULL, tester_id TEXT);
            """);
        Exec(conn, """
            CREATE TABLE project_tester (
                tester_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, name TEXT NOT NULL,
                cloud TEXT NOT NULL, region TEXT NOT NULL, vm_size TEXT NOT NULL, vm_name TEXT,
                vm_resource_id TEXT, public_ip TEXT, ssh_user TEXT NOT NULL, power_state TEXT NOT NULL,
                allocation TEXT NOT NULL, status_message TEXT, locked_by_config_id TEXT,
                installer_version TEXT, last_installed_at TEXT,
                auto_shutdown_enabled INTEGER NOT NULL DEFAULT 0, auto_shutdown_local_hour INTEGER NOT NULL DEFAULT 0,
                next_shutdown_at TEXT, shutdown_deferral_count INTEGER NOT NULL DEFAULT 0,
                auto_probe_enabled INTEGER NOT NULL DEFAULT 0, last_used_at TEXT,
                avg_benchmark_duration_seconds REAL, benchmark_run_count INTEGER NOT NULL DEFAULT 0,
                created_by TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
                cloud_connection_id TEXT, cloud_account_id TEXT, requested_os TEXT, requested_variant TEXT,
                os_distro TEXT, os_version TEXT, os_variant TEXT, os_arch TEXT, os_kernel TEXT);
            """);
        return sp;
    }

    private static void Exec(SqliteConnection c, string sql)
    {
        using var cmd = c.CreateCommand();
        cmd.CommandText = sql;
        cmd.ExecuteNonQuery();
    }

    private static AgentMessageProcessor Processor(IServiceProvider sp) => new(
        sp.GetRequiredService<NetworkerDbContext>(),
        sp.GetRequiredService<EventBus>(),
        sp.GetRequiredService<ILogger<AgentMessageProcessor>>());

    private static async Task SeedAsync(IServiceProvider sp, Guid agentId, Guid testerId, string powerState, string? status)
    {
        var db = sp.GetRequiredService<NetworkerDbContext>();
        var now = DateTime.UtcNow;
        db.Agents.Add(new Data.Entities.Agent
        {
            AgentId = agentId, Name = "a", Status = "offline", RegisteredAt = now,
            ProjectId = "p-1", TesterId = testerId,
        });
        db.ProjectTesters.Add(new Data.Entities.ProjectTester
        {
            TesterId = testerId, ProjectId = "p-1", Name = "runner", Cloud = "azure",
            Region = "eastus", VmSize = "Standard_B2s", SshUser = "azureuser",
            PowerState = powerState, Allocation = "idle", StatusMessage = status,
            CreatedAt = now, UpdatedAt = now,
        });
        await db.SaveChangesAsync();
    }

    private static async Task<string> PowerStateAsync(IServiceProvider sp, Guid testerId)
    {
        var db = sp.GetRequiredService<NetworkerDbContext>();
        return await db.ProjectTesters.AsNoTracking()
            .Where(t => t.TesterId == testerId).Select(t => t.PowerState).FirstAsync();
    }

    [Theory]
    [InlineData("stopped")]
    [InlineData("stopping")]
    public async Task Heartbeat_flips_stale_stopped_to_running_and_clears_status(string stale)
    {
        var sp = BuildHost();
        var agentId = Guid.NewGuid();
        var testerId = Guid.NewGuid();
        await SeedAsync(sp, agentId, testerId, stale, "auto-shutdown completed");

        await Processor(sp).HandleFrameAsync(agentId,
            """{"type":"heartbeat","load":0.1,"version":"0.28.118"}""");

        Assert.Equal("running", await PowerStateAsync(sp, testerId));
        var status = await sp.GetRequiredService<NetworkerDbContext>().ProjectTesters
            .AsNoTracking().Where(t => t.TesterId == testerId).Select(t => t.StatusMessage).FirstAsync();
        Assert.Null(status);
    }

    [Fact]
    public async Task Heartbeat_leaves_a_running_runner_untouched()
    {
        var sp = BuildHost();
        var agentId = Guid.NewGuid();
        var testerId = Guid.NewGuid();
        await SeedAsync(sp, agentId, testerId, "running", "Start completed");

        await Processor(sp).HandleFrameAsync(agentId,
            """{"type":"heartbeat","load":0.1,"version":"0.28.118"}""");

        Assert.Equal("running", await PowerStateAsync(sp, testerId));
    }
}
