using Microsoft.Data.Sqlite;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Provisioning;
using Networker.ControlPlane.Realtime;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Contract tests for the deploy runner's terminal persistence on an endpoint
/// UPDATE re-run (silent-update investigation, prod 2026-08-18: clicking
/// "update" on a deployed target surfaced nothing and changed nothing).
///
/// <para>The honest-failure half: a re-deploy that fails must record
/// <c>status=failed</c> with a real <c>error_message</c> — and must NOT wipe
/// the row's <c>endpoint_ips</c>/<c>endpoint_hosts</c>. Those describe the
/// STILL-SERVING endpoints of the live deployment being updated; nulling them
/// (the old behaviour) made the target silently vanish from the deployed-
/// targets/system-versions panels and destroyed the reverse-lookup inputs the
/// DELETE VM teardown depends on (orphaning the VM to the reaper).</para>
///
/// <para>Joins the <c>cloud-cli-fake-bins</c> collection: every class that
/// points the process-wide <c>INSTALL_SH_PATH</c> at its own stub must run
/// serialized with the others (see <c>DeployDiagnosticsTests</c>).</para>
/// </summary>
[Collection("cloud-cli-fake-bins")]
public class DeployRunnerUpdateContractTests
{
    private const string ProjectId = "p-upd";
    private const string OriginalIpsJson = """["20.1.2.3"]""";
    private const string OriginalHostsJson = """["nwk-ep-ubuntu-tfm7.eastus.cloudapp.azure.com"]""";

    private static (ServiceProvider Sp, SqliteConnection Conn) BuildHost()
    {
        var conn = new SqliteConnection("DataSource=:memory:");
        conn.Open();

        var services = new ServiceCollection();
        services.AddLogging(b => b.SetMinimumLevel(LogLevel.Warning));
        services.AddSignalR();
        services.AddDashboardEventBus();
        services.AddSingleton(conn);
        services.AddDbContext<NetworkerDbContext>(o => o.UseSqlite(conn));

        var sp = services.BuildServiceProvider();
        RunDispatcherTesterFkTests.CreateMinimalSchema(conn);
        return (sp, conn);
    }

    private static DeployRunner Runner(IServiceProvider sp) => new(
        sp.GetRequiredService<IServiceScopeFactory>(),
        sp.GetRequiredService<EventBus>(),
        sp.GetRequiredService<ILogger<DeployRunner>>());

    /// <summary>Seed a COMPLETED deployment with live endpoint_ips/hosts — the
    /// state an update re-run starts from.</summary>
    private static Guid SeedCompletedDeployment(IServiceProvider sp)
    {
        using var scope = sp.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
        var now = DateTime.UtcNow;
        db.Projects.Add(new Project
        {
            ProjectId = ProjectId,
            Name = "upd",
            Slug = "upd",
            Settings = "{}",
            CreatedAt = now,
            UpdatedAt = now,
        });
        var id = Guid.NewGuid();
        db.Deployments.Add(new Deployment
        {
            DeploymentId = id,
            Name = "target-azure-eastus-nginx-x759",
            Status = "completed",
            Config = "{}",
            ProviderSummary = "azure / eastus",
            CreatedAt = now,
            FinishedAt = now,
            EndpointIps = OriginalIpsJson,
            EndpointHosts = OriginalHostsJson,
            ProjectId = ProjectId,
        });
        db.SaveChanges();
        return id;
    }

    private static Deployment Row(IServiceProvider sp, Guid id)
    {
        using var scope = sp.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
        return db.Deployments.AsNoTracking().Single(d => d.DeploymentId == id);
    }

    /// <summary>Point INSTALL_SH_PATH at a throwaway script for the duration of
    /// <paramref name="body"/>, restoring the previous value afterwards.</summary>
    private static async Task WithInstallShAsync(string scriptBody, Func<Task> body)
    {
        var path = Path.Combine(Path.GetTempPath(), $"install-stub-{Guid.NewGuid():N}.sh");
        await File.WriteAllTextAsync(path, scriptBody);
        var previous = Environment.GetEnvironmentVariable("INSTALL_SH_PATH");
        Environment.SetEnvironmentVariable("INSTALL_SH_PATH", path);
        try
        {
            await body();
        }
        finally
        {
            Environment.SetEnvironmentVariable("INSTALL_SH_PATH", previous);
            try { File.Delete(path); } catch { /* best-effort */ }
        }
    }

    [Fact]
    public async Task Failed_update_redeploy_records_honest_error_and_preserves_endpoint_ips()
    {
        var (sp, conn) = BuildHost();
        await using var _ = sp;
        using var __ = conn;

        var id = SeedCompletedDeployment(sp);

        await WithInstallShAsync("#!/usr/bin/env bash\nexit 7\n", async () =>
        {
            await Runner(sp).RunDeploymentAsync(id, "{}", CancellationToken.None);
        });

        var row = Row(sp, id);
        Assert.Equal("failed", row.Status);
        // An honest, non-empty reason — never a silent failure.
        Assert.False(string.IsNullOrWhiteSpace(row.ErrorMessage));
        // The live endpoints of the deployment being updated must survive the
        // failed re-run (regression: they were nulled, vanishing the target
        // from the UI and breaking DELETE's VM teardown reverse-lookup).
        Assert.Equal(OriginalIpsJson, row.EndpointIps);
        Assert.Equal(OriginalHostsJson, row.EndpointHosts);
        Assert.NotNull(row.FinishedAt);
    }

    [Fact]
    public async Task Failed_update_redeploy_persists_the_failure_detail_in_the_log()
    {
        var (sp, conn) = BuildHost();
        await using var _ = sp;
        using var __ = conn;

        var id = SeedCompletedDeployment(sp);

        // The install's own diagnostics (stderr included) must survive into the
        // persisted log the UI links to — the failure may only be explained there.
        await WithInstallShAsync("#!/usr/bin/env bash\necho 'preflight: cannot reach VM' >&2\nexit 1\n", async () =>
        {
            await Runner(sp).RunDeploymentAsync(id, "{}", CancellationToken.None);
        });

        var row = Row(sp, id);
        Assert.Equal("failed", row.Status);
        Assert.False(string.IsNullOrWhiteSpace(row.ErrorMessage));
        Assert.Equal(OriginalIpsJson, row.EndpointIps);
        Assert.Equal(OriginalHostsJson, row.EndpointHosts);
        // The failure detail reaches the persisted log for the UI to show.
        Assert.Contains("cannot reach VM", row.Log);
    }

    [Fact]
    public async Task Successful_update_redeploy_refreshes_endpoint_ips()
    {
        if (!File.Exists("/bin/bash"))
        {
            return; // Windows CI: the success path needs a real bash run.
        }

        var (sp, conn) = BuildHost();
        await using var _ = sp;
        using var __ = conn;

        var id = SeedCompletedDeployment(sp);

        await WithInstallShAsync("#!/usr/bin/env bash\necho 'endpoint_ip: 20.9.9.9'\nexit 0\n", async () =>
        {
            await Runner(sp).RunDeploymentAsync(id, "{}", CancellationToken.None);
        });

        var row = Row(sp, id);
        Assert.Equal("completed", row.Status);
        Assert.Null(row.ErrorMessage);
        Assert.Contains("20.9.9.9", row.EndpointIps);
    }
}
