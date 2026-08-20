using Microsoft.Data.Sqlite;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Background;
using Networker.ControlPlane.Provisioning;
using Networker.ControlPlane.Realtime;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pins issue #817 — installs killed with exit -1 in quota-saturated matrices:
///
/// <list type="number">
///   <item><b>Budget aging</b> — the deploy budget must age from when the
///     install actually STARTS (the first <c>Step N: Install … reference API</c>
///     line install.sh prints once the VM exists and SSH answers), not from
///     spawn/creation: quota/provisioning contention ahead of the install used
///     to eat the whole window and slow-but-healthy AOT publishes were
///     tree-killed mid-publish. The runner re-arms its timer at the marker and
///     re-stamps <c>deployment.started_at</c> so the watchdog ages from the
///     same anchor.</item>
///   <item><b>Kill classification</b> — a kill during a control-plane shutdown
///     window (exit 1/-1 with the signal laundered away, #804 evidence) and a
///     watchdog reap of a never-started install are RETRYABLE through the
///     existing V049/#764 retry machinery; credential/permanent failures stay
///     terminal.</item>
/// </list>
/// </summary>
public class InstallBudgetAnchorTests
{
    private const string ProjectId = "proj-anchor-817";

    /// <summary>One reference-API language → 30m base + 8m (the matrix-cell shape).</summary>
    private const string OneLanguageConfig =
        """{"version":1,"endpoints":[{"provider":"azure","languages":["csharp-net8-aot"]}]}""";

    // ── Install-phase marker detection ───────────────────────────────────

    [Theory]
    [InlineData("Step 7: Install csharp-net8-aot reference API on 20.1.2.3 (port 8085, app mode)")]
    [InlineData("Step 4: Install java reference API locally (port 8085, app mode)")]
    [InlineData("Step 9: Install go reference API (Azure Windows)")]
    public void marker_matches_the_install_phase_step_lines(string line)
    {
        Assert.Matches(DeployRunner.InstallPhaseMarkerRe, line);
    }

    [Theory]
    // VM provisioning steps must NOT re-anchor the budget.
    [InlineData("Step 3: Create Azure VM nwk-a-1a2b3c4d")]
    [InlineData("Step 5: Wait for SSH on 20.1.2.3")]
    // Echoed config / prose mentioning the phrase mid-line must not trip it.
    [InlineData("  will Install cpp reference API later")]
    [InlineData("deploy.json: languages=[csharp-net8-aot] → Install reference API steps queued")]
    public void marker_ignores_non_install_lines(string line)
    {
        Assert.DoesNotMatch(DeployRunner.InstallPhaseMarkerRe, line);
    }

    [Fact]
    public void first_marker_line_sets_the_anchor_and_fires_the_callback_once()
    {
        var output = new DeployRunner.DeployOutput();
        var fired = 0;
        output.OnInstallPhaseStarted = () => fired++;

        output.ProcessLine("Step 2: Create resource group nwk-rg", "stdout");
        Assert.Null(output.InstallPhaseStartedUtc);
        Assert.Equal(0, fired);

        output.ProcessLine("Step 7: Install csharp-net8-aot reference API on 20.1.2.3 (port 8085, app mode)", "stdout");
        Assert.NotNull(output.InstallPhaseStartedUtc);
        Assert.Equal(1, fired);

        // A second language's install step keeps the FIRST anchor: the full
        // scaled budget from the first install already covers all languages.
        var anchor = output.InstallPhaseStartedUtc;
        output.ProcessLine("Step 8: Install java reference API on 20.1.2.3 (port 8085, app mode)", "stdout");
        Assert.Equal(anchor, output.InstallPhaseStartedUtc);
        Assert.Equal(1, fired);
    }

    // ── Exit classification (shutdown-window kills are retryable) ────────

    [Fact]
    public void exit_zero_classifies_as_success()
    {
        Assert.Null(DeployRunner.ClassifyExit(0, shuttingDown: false));
        Assert.Null(DeployRunner.ClassifyExit(0, shuttingDown: true));
    }

    [Theory]
    [InlineData(143, "TERM")]
    [InlineData(137, "KILL")]
    public void signal_exits_keep_the_interrupted_marker(int code, string sig)
    {
        var msg = DeployRunner.ClassifyExit(code, shuttingDown: false);
        Assert.True(ProvisioningFailureClassifier.IsInterruptedFailure(msg));
        Assert.Contains($"SIG{sig}", msg);
        Assert.Contains($"exit {code}", msg);
    }

    [Theory]
    [InlineData(1)]
    [InlineData(-1)]
    public void nonzero_exit_during_shutdown_window_is_the_interrupted_marker(int code)
    {
        // The SIGTERM often lands on install.sh's child (ssh/az) first, so the
        // kill surfaces as a plain exit 1/-1 without the 143/137 code — it
        // must still classify as retryable interruption, not a config failure.
        var msg = DeployRunner.ClassifyExit(code, shuttingDown: true);
        Assert.True(ProvisioningFailureClassifier.IsInterruptedFailure(msg));
        Assert.True(ProvisioningFailureClassifier.IsRetryableInfrastructureKill(msg));
        Assert.Contains("shutdown", msg);
    }

    [Theory]
    [InlineData(1)]
    [InlineData(-1)]
    public void nonzero_exit_outside_shutdown_stays_terminal(int code)
    {
        var msg = DeployRunner.ClassifyExit(code, shuttingDown: false);
        Assert.Equal($"install.sh exited with code {code}", msg);
        Assert.False(ProvisioningFailureClassifier.IsRetryableInfrastructureKill(msg));
    }

    [Fact]
    public void timeout_message_says_which_anchor_the_budget_aged_from()
    {
        var pre = DeployRunner.TimeoutMessageFor(TimeSpan.FromMinutes(38), installPhaseStarted: false);
        Assert.Contains("install phase never started", pre);
        Assert.Contains("38m", pre);

        var post = DeployRunner.TimeoutMessageFor(TimeSpan.FromMinutes(38), installPhaseStarted: true);
        Assert.Contains("after its install phase started", post);
        Assert.Contains("38m", post);
    }

    // ── started_at re-stamp (the watchdog's aging anchor) ────────────────

    [Fact]
    public async Task install_start_stamp_moves_started_at_for_a_running_deployment()
    {
        using var sp = RunDispatcherTesterFkTests.BuildHost(nameof(install_start_stamp_moves_started_at_for_a_running_deployment));
        var oldStart = DateTime.UtcNow.AddMinutes(-25); // long provisioning wait
        var id = SeedDeployment(Db(sp), OneLanguageConfig, "running", startedAt: oldStart);

        await Runner(sp).StampInstallStartAsync(id);

        var row = Row(sp, id);
        Assert.NotNull(row.StartedAt);
        Assert.True(row.StartedAt > DateTime.UtcNow.AddMinutes(-1),
            "started_at must be re-stamped at install start so the watchdog ages the install, not the queue wait");
    }

    [Fact]
    public async Task install_start_stamp_leaves_non_running_deployments_alone()
    {
        using var sp = RunDispatcherTesterFkTests.BuildHost(nameof(install_start_stamp_leaves_non_running_deployments_alone));
        var oldStart = DateTime.UtcNow.AddMinutes(-25);
        var id = SeedDeployment(Db(sp), OneLanguageConfig, "failed", startedAt: oldStart);

        await Runner(sp).StampInstallStartAsync(id);

        Assert.Equal(oldStart, Row(sp, id).StartedAt!.Value, TimeSpan.FromSeconds(1));
    }

    /// <summary>The #817 field shape end-to-end at the watchdog: a deployment
    /// CREATED hours ago (quota-saturated matrix) whose install phase started
    /// recently (re-stamped started_at) is inside its budget — not reaped.</summary>
    [Fact]
    public async Task watchdog_spares_a_deploy_whose_install_started_recently_despite_old_creation()
    {
        using var sp = RunDispatcherTesterFkTests.BuildHost(nameof(watchdog_spares_a_deploy_whose_install_started_recently_despite_old_creation));
        var id = SeedDeployment(Db(sp), OneLanguageConfig, "running",
            startedAt: DateTime.UtcNow.AddMinutes(-10),   // install-start stamp
            createdAt: DateTime.UtcNow.AddHours(-2));      // long queue/provisioning wait

        await WatchdogTickHarness.RunOnceAsync(sp, sp.GetRequiredService<AgentConnectionRegistry>());

        Assert.Equal("running", Row(sp, id).Status);
    }

    // ── Watchdog reap classification: never-started ⇒ retryable ──────────

    [Fact]
    public async Task watchdog_reaps_a_never_started_pending_deployment_with_the_retryable_marker()
    {
        using var sp = RunDispatcherTesterFkTests.BuildHost(nameof(watchdog_reaps_a_never_started_pending_deployment_with_the_retryable_marker));
        // pending = install.sh never ran (the runner flips to running before
        // spawning it). Aged far past budget (38m) + slack (5m).
        var id = SeedDeployment(Db(sp), OneLanguageConfig, "pending",
            startedAt: null, createdAt: DateTime.UtcNow.AddMinutes(-50));

        await WatchdogTickHarness.RunOnceAsync(sp, sp.GetRequiredService<AgentConnectionRegistry>());

        var row = Row(sp, id);
        Assert.Equal("failed", row.Status);
        Assert.StartsWith(ProvisioningFailureClassifier.NeverStartedReapPrefix, row.ErrorMessage);
        Assert.True(ProvisioningFailureClassifier.IsRetryableInfrastructureKill(row.ErrorMessage));
    }

    [Fact]
    public async Task watchdog_reap_of_a_started_install_stays_terminal()
    {
        using var sp = RunDispatcherTesterFkTests.BuildHost(nameof(watchdog_reap_of_a_started_install_stays_terminal));
        var id = SeedDeployment(Db(sp), OneLanguageConfig, "running",
            startedAt: DateTime.UtcNow.AddMinutes(-50));

        await WatchdogTickHarness.RunOnceAsync(sp, sp.GetRequiredService<AgentConnectionRegistry>());

        var row = Row(sp, id);
        Assert.Equal("failed", row.Status);
        Assert.Equal(
            "Deployment did not finish within its 38m budget (1 language)",
            row.ErrorMessage);
        Assert.False(ProvisioningFailureClassifier.IsRetryableInfrastructureKill(row.ErrorMessage));
    }

    // ── Orchestrator: never-started reap re-queues through V049 machinery ─

    [Fact]
    public async Task never_started_reap_requeues_the_run_instead_of_failing_it()
    {
        var (sp, conn) = BuildOrchestratorHost();
        using var _ = conn;
        Guid runId, configId, deploymentId;
        using (var db = Db(sp))
        {
            (runId, configId, deploymentId) = SeedProvisioningRun(
                db, WatchdogService.NeverStartedReapedErrorFor(OneLanguageConfig));
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
        Assert.Contains("reaped before its install ever started", run.ErrorMessage);
        Assert.Contains("retry 1/", run.ErrorMessage);
        Assert.Null(run.FinishedAt); // NOT terminal — the run lives on

        // The dead deployment releases its throttle slot but keeps its row+log.
        Assert.Equal(
            ProvisioningOrchestrator.DeploymentTornDown,
            check.Deployments.Single(d => d.DeploymentId == deploymentId).Status);
    }

    [Fact]
    public async Task never_started_reap_past_max_attempts_fails_with_a_human_message()
    {
        var (sp, conn) = BuildOrchestratorHost();
        using var _ = conn;
        Guid runId, configId, deploymentId;
        using (var db = Db(sp))
        {
            (runId, configId, deploymentId) = SeedProvisioningRun(
                db, WatchdogService.NeverStartedReapedErrorFor(OneLanguageConfig),
                attempts: ProvisioningOrchestrator.MaxProvisionAttempts);
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
        Assert.Contains("before its install could start", run.ErrorMessage);
    }

    /// <summary>Credential/permanent failures must stay terminal — the retry
    /// arm only fires for infrastructure kills.</summary>
    [Fact]
    public async Task plain_install_failure_stays_terminal()
    {
        var (sp, conn) = BuildOrchestratorHost();
        using var _ = conn;
        Guid runId, configId, deploymentId;
        using (var db = Db(sp))
        {
            (runId, configId, deploymentId) = SeedProvisioningRun(
                db, "install.sh exited with code 1"); // e.g. bad credentials/config
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
        Assert.Equal(0, run.ProvisionAttempts); // no retry consumed
        Assert.Contains("Provisioning failed", run.ErrorMessage);
    }

    // ── Harness ──────────────────────────────────────────────────────────

    private static (ServiceProvider Sp, SqliteConnection Conn) BuildOrchestratorHost()
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

    private static DeployRunner Runner(IServiceProvider sp) => new(
        sp.GetRequiredService<IServiceScopeFactory>(),
        sp.GetRequiredService<EventBus>(),
        sp.GetRequiredService<ILogger<DeployRunner>>());

    private static ProvisioningOrchestrator Orchestrator(IServiceProvider sp) => new(
        sp.GetRequiredService<IServiceScopeFactory>(),
        Runner(sp),
        sp.GetRequiredService<ILogger<ProvisioningOrchestrator>>());

    private static NetworkerDbContext Db(IServiceProvider sp) =>
        sp.CreateScope().ServiceProvider.GetRequiredService<NetworkerDbContext>();

    private static Deployment Row(IServiceProvider sp, Guid id) =>
        Db(sp).Deployments.AsNoTracking().Single(d => d.DeploymentId == id);

    private static void EnsureProject(NetworkerDbContext db)
    {
        if (!db.Projects.Any(p => p.ProjectId == ProjectId))
        {
            db.Projects.Add(new Project
            {
                ProjectId = ProjectId,
                Name = "anchor-817",
                Slug = "anchor-817",
                Settings = "{}",
                CreatedAt = DateTime.UtcNow,
                UpdatedAt = DateTime.UtcNow,
            });
        }
    }

    private static Guid SeedDeployment(
        NetworkerDbContext db,
        string config,
        string status,
        DateTime? startedAt,
        DateTime? createdAt = null)
    {
        EnsureProject(db);
        var id = Guid.NewGuid();
        db.Deployments.Add(new Deployment
        {
            DeploymentId = id,
            Name = $"dep-{id:N}",
            Status = status,
            Config = config,
            StartedAt = startedAt,
            CreatedAt = createdAt ?? DateTime.UtcNow.AddHours(-3),
            ProjectId = ProjectId,
        });
        db.SaveChanges();
        return id;
    }

    private static (Guid RunId, Guid ConfigId, Guid DeploymentId) SeedProvisioningRun(
        NetworkerDbContext db, string deploymentError, short attempts = 0)
    {
        EnsureProject(db);
        var now = DateTime.UtcNow;
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
            Config = OneLanguageConfig,
            ErrorMessage = deploymentError,
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
}
