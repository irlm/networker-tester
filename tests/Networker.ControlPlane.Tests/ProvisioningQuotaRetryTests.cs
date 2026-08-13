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
/// Pins the quota-retry arm of the provisioning orchestrator (v0.28.201) —
/// the fix for 13 of 15 matrix cells failing PERMANENTLY on Azure's regional
/// cores quota while cells that raced in later completed fine (user-caught
/// 2026-08-13). Capacity quota is transient by construction (teardown frees
/// cores/IPs), so a quota-failed cell must re-queue with backoff, release its
/// dead deployment's throttle slot, and only fail — with a HUMAN message —
/// after the attempts cap.
/// </summary>
public class ProvisioningQuotaRetryTests
{
    private const string ProjectId = "proj-quota-0001";

    /// <summary>The live 2026-08-13 failure text (abridged) — the classifier
    /// must recognize the real thing, not a synthetic marker.</summary>
    private const string QuotaLog =
        "Operation could not be completed as it results in exceeding approved "
        + "Total Regional Cores quota. Additional details - Deployment Model: Resource Manager, "
        + "Location: eastus, Current Limit: 10, Current Usage: 10, Additional Required: 4. "
        + "Please read more about quota limits or submit a request for Quota increase at https://aka.ms/ProdportalCRP/";

    // ── Classifier ───────────────────────────────────────────────────────

    [Theory]
    [InlineData(QuotaLog)]
    [InlineData("ERROR: {\"code\": \"QuotaExceeded\", \"message\": \"...\"}")]
    [InlineData("PublicIPCountLimitReached: Cannot create more than 10 public IP addresses")]
    public void classifier_recognizes_capacity_quota_failures(string log)
    {
        Assert.True(ProvisioningFailureClassifier.IsQuotaFailure(log, "install.sh exited with code 1"));
    }

    [Theory]
    [InlineData("endpoints[0]: nginx requires Linux but os is 'windows'")]
    [InlineData("curl: (56) Connection died, tried 5 times before giving up")]
    [InlineData("")]
    [InlineData(null)]
    public void classifier_leaves_non_quota_failures_alone(string? log)
    {
        Assert.False(ProvisioningFailureClassifier.IsQuotaFailure(log, "install.sh exited with code 1"));
    }

    // ── Retry flow ───────────────────────────────────────────────────────

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

    /// <summary>Seed a provisioning run linked to a FAILED deployment carrying
    /// <paramref name="log"/>. Returns (runId, configId, deploymentId).</summary>
    private static (Guid RunId, Guid ConfigId, Guid DeploymentId) SeedFailedProvision(
        NetworkerDbContext db, string log, short attempts = 0)
    {
        var now = DateTime.UtcNow;
        if (!db.Projects.Any(p => p.ProjectId == ProjectId))
        {
            db.Projects.Add(new Project
            {
                ProjectId = ProjectId,
                Name = "quota",
                Slug = "quota",
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
            ErrorMessage = "install.sh exited with code 1",
            Log = log,
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
    public async Task quota_failure_requeues_with_backoff_and_releases_the_dead_deployment()
    {
        var (sp, conn) = BuildHost();
        using var _ = conn;
        Guid runId, configId, deploymentId;
        using (var db = Db(sp))
        {
            (runId, configId, deploymentId) = SeedFailedProvision(db, QuotaLog);
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
        Assert.Null(run.FinishedAt); // NOT terminal — the run lives on

        // The dead deployment stops counting toward the capacity throttle but
        // keeps its row+log as the diagnostic.
        var dep = check.Deployments.Single(d => d.DeploymentId == deploymentId);
        Assert.Equal(ProvisioningOrchestrator.DeploymentTornDown, dep.Status);
    }

    [Fact]
    public async Task quota_failure_after_max_attempts_fails_with_a_human_message()
    {
        var (sp, conn) = BuildHost();
        using var _ = conn;
        Guid runId, configId, deploymentId;
        using (var db = Db(sp))
        {
            (runId, configId, deploymentId) = SeedFailedProvision(
                db, QuotaLog, attempts: ProvisioningOrchestrator.MaxProvisionAttempts);
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
        Assert.Contains("quota exceeded", run.ErrorMessage, StringComparison.OrdinalIgnoreCase);
        Assert.Contains("NETWORKER_MAX_CONCURRENT_PROVISIONS", run.ErrorMessage);
        Assert.DoesNotContain("exited with code 1", run.ErrorMessage); // human, not exit-code
    }

    [Fact]
    public async Task non_quota_failure_fails_immediately_with_the_raw_message()
    {
        var (sp, conn) = BuildHost();
        using var _ = conn;
        Guid runId, configId, deploymentId;
        using (var db = Db(sp))
        {
            (runId, configId, deploymentId) = SeedFailedProvision(
                db, "──── Validating deploy config ────\n  ✗ endpoints[0]: nginx requires Linux but os is 'windows'");
        }

        using (var db = Db(sp))
        {
            Assert.True(await Orchestrator(sp)
                .HandleProvisioningRunAsync(db, runId, configId, deploymentId, CancellationToken.None));
        }

        using var check = Db(sp);
        var run = check.TestRuns.Single(r => r.Id == runId);
        Assert.Equal("failed", run.Status);
        Assert.Equal(0, run.ProvisionAttempts); // no retry consumed
        Assert.Contains("Provisioning failed", run.ErrorMessage);
    }
}
