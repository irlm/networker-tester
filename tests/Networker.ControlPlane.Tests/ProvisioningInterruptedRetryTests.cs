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
/// Pins the interrupted-by-restart retry arm of the provisioning orchestrator
/// (issue #764): a control-plane release restart SIGTERMs any in-flight
/// install.sh (exit 143), and the runner marks the deployment failed with the
/// "Deployment interrupted" message. The startup recovery pass normally
/// revives the deployment before the orchestrator sees it; when it can't
/// (recovery cap, missed window, or a kill without a restart), the run must
/// re-queue for a fresh kick — the interruption is transient by construction —
/// instead of failing terminally, mirroring the V049 quota-retry machinery.
/// </summary>
public class ProvisioningInterruptedRetryTests
{
    private const string ProjectId = "proj-intr-0001";

    private static readonly string InterruptedMessage =
        ProvisioningFailureClassifier.InterruptedErrorPrefix
        + "TERM, exit 143) — the control plane restarted or was shut down while deploying. "
        + "Any VM it had already created is reaped by the orphan sweep; retry the deployment.";

    private static (ServiceProvider Sp, SqliteConnection Conn) BuildHost()
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
        RunDispatcherTesterFkTests.CreateMinimalSchema(conn);
        return (sp, conn);
    }

    private static ProvisioningOrchestrator Orchestrator(IServiceProvider sp) => new(
        sp.GetRequiredService<IServiceScopeFactory>(),
        new DeployRunner(
            sp.GetRequiredService<IServiceScopeFactory>(),
            sp.GetRequiredService<EventBus>(),
            sp.GetRequiredService<ILogger<DeployRunner>>()),
        sp.GetRequiredService<ILogger<ProvisioningOrchestrator>>());

    private static NetworkerDbContext Db(IServiceProvider sp) =>
        sp.CreateScope().ServiceProvider.GetRequiredService<NetworkerDbContext>();

    private static (Guid RunId, Guid ConfigId, Guid DeploymentId) SeedInterruptedProvision(
        NetworkerDbContext db, short attempts = 0)
    {
        var now = DateTime.UtcNow;
        if (!db.Projects.Any(p => p.ProjectId == ProjectId))
        {
            db.Projects.Add(new Project
            {
                ProjectId = ProjectId,
                Name = "interrupted",
                Slug = "interrupted",
                Settings = "{}",
                CreatedAt = now,
                UpdatedAt = now,
            });
        }
        var configId = Guid.NewGuid();
        db.TestConfigs.Add(new TestConfig
        {
            Id = configId,
            ProjectId = ProjectId,
            Name = $"cell-{configId:N}",
            EndpointKind = "pending",
            EndpointRef = """{"kind": "pending", "proxy_stack": "nginx"}""",
            Workload = "{}",
            MaxDurationSecs = 600,
            CreatedAt = now,
            UpdatedAt = now,
        });
        var deploymentId = Guid.NewGuid();
        db.Deployments.Add(new Deployment
        {
            DeploymentId = deploymentId,
            Name = $"auto-cell-{deploymentId:N}",
            Status = "failed",
            Config = "{}",
            ErrorMessage = InterruptedMessage,
            FinishedAt = now,
            CreatedAt = now,
            ProjectId = ProjectId,
        });
        var runId = Guid.NewGuid();
        db.TestRuns.Add(new TestRun
        {
            Id = runId,
            TestConfigId = configId,
            ProjectId = ProjectId,
            Status = "provisioning",
            ProvisioningDeploymentId = deploymentId,
            ProvisionAttempts = attempts,
            CreatedAt = now,
        });
        db.SaveChanges();
        return (runId, configId, deploymentId);
    }

    [Fact]
    public async Task interrupted_failure_requeues_with_backoff_and_releases_the_dead_deployment()
    {
        var (sp, conn) = BuildHost();
        using var _ = conn;
        Guid runId, configId, deploymentId;
        using (var db = Db(sp))
        {
            (runId, configId, deploymentId) = SeedInterruptedProvision(db);
        }

        bool resolved;
        using (var db = Db(sp))
        {
            resolved = await Orchestrator(sp)
                .HandleProvisioningRunAsync(db, runId, configId, deploymentId, CancellationToken.None);
        }
        Assert.True(resolved);

        using var check = Db(sp);
        var run = check.TestRuns.Single(r => r.Id == runId);
        Assert.Equal("queued", run.Status);
        Assert.Null(run.ProvisioningDeploymentId);
        Assert.Equal(1, run.ProvisionAttempts);
        Assert.NotNull(run.NextProvisionAttemptAt);
        Assert.True(run.NextProvisionAttemptAt > DateTime.UtcNow, "backoff must be in the future");
        Assert.Contains("retry 1/", run.ErrorMessage);
        Assert.Contains("restart interrupted provisioning", run.ErrorMessage);
        Assert.Null(run.FinishedAt); // NOT terminal — the run lives on

        // The dead deployment stops counting toward the capacity throttle but
        // keeps its row+log as the diagnostic.
        var dep = check.Deployments.Single(d => d.DeploymentId == deploymentId);
        Assert.Equal(ProvisioningOrchestrator.DeploymentTornDown, dep.Status);
    }

    [Fact]
    public async Task interrupted_failure_after_max_attempts_fails_with_a_human_message()
    {
        var (sp, conn) = BuildHost();
        using var _ = conn;
        Guid runId, configId, deploymentId;
        using (var db = Db(sp))
        {
            (runId, configId, deploymentId) = SeedInterruptedProvision(
                db, attempts: ProvisioningOrchestrator.MaxProvisionAttempts);
        }

        using (var db = Db(sp))
        {
            Assert.True(await Orchestrator(sp)
                .HandleProvisioningRunAsync(db, runId, configId, deploymentId, CancellationToken.None));
        }

        using var check = Db(sp);
        var run = check.TestRuns.Single(r => r.Id == runId);
        Assert.Equal("failed", run.Status);
        Assert.NotNull(run.FinishedAt);
        Assert.Contains("interrupted by control-plane restarts", run.ErrorMessage);
        Assert.DoesNotContain("exit 143", run.ErrorMessage); // human, not exit-code
    }
}
