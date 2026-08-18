using Microsoft.Data.Sqlite;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.Logging.Abstractions;
using Networker.ControlPlane.Background;
using Networker.ControlPlane.Provisioning;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pins the startup deployment-recovery pass (issue #764): install.sh runs as a
/// child of the control plane, so every release restart SIGTERMs any in-flight
/// endpoint deployment (exit 143). The recovery pass re-runs, on startup,
/// deployments the previous process left behind — rows wedged at
/// pending/running (crash) and rows recently failed with the runner's
/// "Deployment interrupted" marker (graceful shutdown) — while leaving genuine
/// failures, stale history, capped rows, and dead-run-linked rows alone.
/// </summary>
public class DeploymentRecoveryTests
{
    private const string ProjectId = "proj-recov-0001";

    /// <summary>The exact prod message shape DeployRunner writes on exit 143
    /// (v0.28.213's interrupted messaging) — the recovery pass must recognize
    /// the real thing.</summary>
    private static readonly string InterruptedMessage =
        ProvisioningFailureClassifier.InterruptedErrorPrefix
        + "TERM, exit 143) — the control plane restarted or was shut down while deploying. "
        + "Any VM it had already created is reaped by the orphan sweep; retry the deployment.";

    // ── Classifier ───────────────────────────────────────────────────────

    [Fact]
    public void classifier_recognizes_the_runner_interrupted_messages()
    {
        Assert.True(ProvisioningFailureClassifier.IsInterruptedFailure(InterruptedMessage));
        Assert.True(ProvisioningFailureClassifier.IsInterruptedFailure(
            "Deployment interrupted (install.sh received SIGKILL, exit 137) — the control plane restarted "
            + "or was shut down while deploying. Any VM it had already created is reaped by the orphan sweep; "
            + "retry the deployment."));
    }

    [Theory]
    [InlineData("install.sh exited with code 1")]
    [InlineData("Deployment cancelled")]
    [InlineData("install.sh timed out after 30m and was killed")]
    [InlineData("")]
    [InlineData(null)]
    public void classifier_leaves_other_failures_alone(string? message)
    {
        Assert.False(ProvisioningFailureClassifier.IsInterruptedFailure(message));
    }

    // ── Harness ──────────────────────────────────────────────────────────

    private static (NetworkerDbContext Db, SqliteConnection Conn) BuildDb()
    {
        var conn = new SqliteConnection("DataSource=:memory:");
        conn.Open();
        RunDispatcherTesterFkTests.CreateMinimalSchema(conn);

        var options = new DbContextOptionsBuilder<NetworkerDbContext>()
            .UseSqlite(conn)
            .Options;
        var db = new NetworkerDbContext(options);
        db.Projects.Add(new Project
        {
            ProjectId = ProjectId,
            Name = "recovery",
            Slug = "recovery",
            Settings = "{}",
            CreatedAt = DateTime.UtcNow,
            UpdatedAt = DateTime.UtcNow,
        });
        db.SaveChanges();
        return (db, conn);
    }

    private static Guid SeedDeployment(
        NetworkerDbContext db,
        string status,
        string? error = null,
        DateTime? finishedAt = null,
        short recoveryAttempts = 0,
        DateTime? createdAt = null)
    {
        var id = Guid.NewGuid();
        db.Deployments.Add(new Deployment
        {
            DeploymentId = id,
            Name = $"dep-{id:N}",
            Status = status,
            Config = """{"version":1,"endpoints":[]}""",
            ErrorMessage = error,
            FinishedAt = finishedAt,
            RecoveryAttempts = recoveryAttempts,
            CreatedAt = createdAt ?? DateTime.UtcNow.AddMinutes(-5),
            ProjectId = ProjectId,
        });
        db.SaveChanges();
        return id;
    }

    private static Task<List<DeploymentRecoveryService.RecoveredDeployment>> Recover(NetworkerDbContext db) =>
        DeploymentRecoveryService.RecoverAsync(db, NullLogger.Instance, CancellationToken.None);

    // ── Crash leftovers (pending/running) ────────────────────────────────

    [Theory]
    [InlineData("pending")]
    [InlineData("running")]
    public async Task stuck_driverless_deployment_is_reclaimed_for_a_rerun(string status)
    {
        var (db, conn) = BuildDb();
        using var _ = conn;
        using var __ = db;
        var id = SeedDeployment(db, status);

        var recovered = await Recover(db);

        var r = Assert.Single(recovered);
        Assert.Equal(id, r.DeploymentId);
        Assert.Equal(status, r.PriorStatus);
        Assert.Equal(1, r.Attempt);

        var row = db.Deployments.AsNoTracking().Single(d => d.DeploymentId == id);
        Assert.Equal("pending", row.Status);
        Assert.Equal(1, row.RecoveryAttempts);
        Assert.Null(row.ErrorMessage);
        Assert.Null(row.FinishedAt);
        // started_at is stamped with the claim time: the watchdog's stale sweep
        // must measure THIS attempt, not the original one, or a recovered
        // long-running deploy is reaped against the old created_at.
        Assert.NotNull(row.StartedAt);
        Assert.True(row.StartedAt > DateTime.UtcNow.AddMinutes(-1));
    }

    // ── Graceful-SIGTERM leftovers (failed + interrupted marker) ─────────

    [Fact]
    public async Task recently_interrupted_failed_deployment_is_reclaimed()
    {
        var (db, conn) = BuildDb();
        using var _ = conn;
        using var __ = db;
        var id = SeedDeployment(db, "failed", InterruptedMessage, finishedAt: DateTime.UtcNow.AddMinutes(-1));

        var recovered = await Recover(db);

        Assert.Equal(id, Assert.Single(recovered).DeploymentId);
        var row = db.Deployments.AsNoTracking().Single(d => d.DeploymentId == id);
        Assert.Equal("pending", row.Status);
        Assert.Equal(1, row.RecoveryAttempts);
        Assert.Null(row.ErrorMessage);
    }

    [Fact]
    public async Task interrupted_failure_outside_the_retry_window_is_history_not_work()
    {
        var (db, conn) = BuildDb();
        using var _ = conn;
        using var __ = db;
        SeedDeployment(db, "failed", InterruptedMessage,
            finishedAt: DateTime.UtcNow - DeploymentRecoveryService.InterruptedRetryWindow - TimeSpan.FromMinutes(5));

        Assert.Empty(await Recover(db));
    }

    [Fact]
    public async Task genuine_failures_and_terminal_states_are_left_alone()
    {
        var (db, conn) = BuildDb();
        using var _ = conn;
        using var __ = db;
        SeedDeployment(db, "failed", "install.sh exited with code 1", finishedAt: DateTime.UtcNow);
        SeedDeployment(db, "completed");
        SeedDeployment(db, "cancelled", finishedAt: DateTime.UtcNow);
        SeedDeployment(db, ProvisioningOrchestrator.DeploymentTornDown, finishedAt: DateTime.UtcNow);

        Assert.Empty(await Recover(db));
    }

    // ── Safety rails ─────────────────────────────────────────────────────

    [Fact]
    public async Task recovery_attempt_cap_stops_the_automatic_loop()
    {
        var (db, conn) = BuildDb();
        using var _ = conn;
        using var __ = db;
        SeedDeployment(db, "running", recoveryAttempts: DeploymentRecoveryService.MaxRecoveryAttempts);
        SeedDeployment(db, "failed", InterruptedMessage, finishedAt: DateTime.UtcNow,
            recoveryAttempts: DeploymentRecoveryService.MaxRecoveryAttempts);

        Assert.Empty(await Recover(db));
    }

    [Fact]
    public async Task deployment_whose_linked_run_is_already_terminal_is_not_rerun()
    {
        var (db, conn) = BuildDb();
        using var _ = conn;
        using var __ = db;
        var id = SeedDeployment(db, "failed", InterruptedMessage, finishedAt: DateTime.UtcNow);
        SeedRun(db, id, "failed");

        // Re-running would build a VM nothing consumes — the teardown phase
        // would immediately race to delete it.
        Assert.Empty(await Recover(db));
        Assert.Equal("failed", db.Deployments.AsNoTracking().Single(d => d.DeploymentId == id).Status);
    }

    [Fact]
    public async Task deployment_whose_linked_run_still_provisions_is_rerun()
    {
        var (db, conn) = BuildDb();
        using var _ = conn;
        using var __ = db;
        var id = SeedDeployment(db, "failed", InterruptedMessage, finishedAt: DateTime.UtcNow);
        SeedRun(db, id, "provisioning");

        // The run keeps waiting in `provisioning`; the orchestrator promotes it
        // once the recovered deployment completes.
        Assert.Equal(id, Assert.Single(await Recover(db)).DeploymentId);
    }

    private static void SeedRun(NetworkerDbContext db, Guid deploymentId, string status)
    {
        var configId = Guid.NewGuid();
        db.TestConfigs.Add(new TestConfig
        {
            Id = configId,
            ProjectId = ProjectId,
            Name = $"cfg-{configId:N}",
            EndpointKind = "pending",
            EndpointRef = """{"kind": "pending", "proxy_stack": "nginx"}""",
            Workload = "{}",
            MaxDurationSecs = 600,
            CreatedAt = DateTime.UtcNow,
            UpdatedAt = DateTime.UtcNow,
        });
        db.TestRuns.Add(new TestRun
        {
            Id = Guid.NewGuid(),
            TestConfigId = configId,
            ProjectId = ProjectId,
            Status = status,
            ProvisioningDeploymentId = deploymentId,
            CreatedAt = DateTime.UtcNow,
            FinishedAt = status is "failed" or "completed" or "cancelled" ? DateTime.UtcNow : null,
        });
        db.SaveChanges();
    }
}
