using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Networker.ControlPlane.Background;
using Networker.ControlPlane.Provisioning;
using Networker.ControlPlane.Realtime;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pins the watchdog's stale-deploy sweep against the workload-scaled deploy
/// budget (issue #804): the sweep's old flat 30m cutoff undercut
/// <see cref="DeployRunner.DeployTimeoutFor"/>'s scaled budget (#740) and
/// killed a legitimate 38m cpp deploy at 30 minutes — with a message blaming a
/// control-plane restart. Now the sweep derives its threshold from the SAME
/// budget function plus slack (so the runner's own timeout always fires first
/// with its richer message), the reaped message states the budget it enforced,
/// the restart hint appears only when a #785 recovery re-run actually happened
/// (recovery_attempts &gt; 0), and a recovery re-claim resets the aging basis.
/// </summary>
public sealed class WatchdogDeploymentBudgetTests
{
    private const string ProjectId = "proj-wd-budget";

    /// <summary>The field shape: one reference-API language → 30m base + 8m.</summary>
    private const string CppConfig =
        """{"version":1,"endpoints":[{"provider":"azure","languages":["cpp"]}]}""";

    private const string StackOnlyConfig =
        """{"version":1,"endpoints":[{"provider":"azure","http_stacks":["nginx","caddy"]}]}""";

    private const string ThreeLanguageConfig =
        """{"endpoints":[{"languages":["go","nodejs","python"]}]}""";

    // ── Budget agreement: watchdog threshold == runner budget + slack ────

    [Theory]
    [InlineData(CppConfig)]
    [InlineData(StackOnlyConfig)]
    [InlineData(ThreeLanguageConfig)]
    [InlineData("not json at all")]
    public void watchdog_threshold_is_the_runner_budget_plus_slack(string config)
    {
        Assert.Equal(
            DeployRunner.DeployTimeoutFor(config) + WatchdogService.DeploymentBudgetSlack,
            WatchdogService.DeploymentReapCutoffFor(config));
        // Strictly later than the runner's own timeout: the runner tree-kills a
        // wedged install and writes its richer message before the watchdog can.
        Assert.True(WatchdogService.DeploymentReapCutoffFor(config) > DeployRunner.DeployTimeoutFor(config));
    }

    [Fact]
    public void the_804_field_deploy_gets_43_minutes_not_30()
    {
        Assert.Equal(TimeSpan.FromMinutes(38), DeployRunner.DeployTimeoutFor(CppConfig));
        Assert.Equal(TimeSpan.FromMinutes(43), WatchdogService.DeploymentReapCutoffFor(CppConfig));
    }

    // ── Message honesty ──────────────────────────────────────────────────

    [Fact]
    public void reaped_message_states_the_enforced_budget_and_language_count()
    {
        Assert.Equal(
            "Deployment did not finish within its 38m budget (1 language)",
            WatchdogService.DeploymentReapedErrorFor(CppConfig, recoveredFromRestart: false));
        Assert.Equal(
            "Deployment did not finish within its 30m budget (stack-only)",
            WatchdogService.DeploymentReapedErrorFor(StackOnlyConfig, recoveredFromRestart: false));
        Assert.Equal(
            "Deployment did not finish within its 54m budget (3 languages)",
            WatchdogService.DeploymentReapedErrorFor(ThreeLanguageConfig, recoveredFromRestart: false));
    }

    [Fact]
    public void restart_hint_appears_only_when_a_recovery_rerun_happened()
    {
        var plain = WatchdogService.DeploymentReapedErrorFor(CppConfig, recoveredFromRestart: false);
        Assert.DoesNotContain("restart", plain, StringComparison.OrdinalIgnoreCase);

        var recovered = WatchdogService.DeploymentReapedErrorFor(CppConfig, recoveredFromRestart: true);
        Assert.StartsWith("Deployment did not finish within its 38m budget (1 language)", recovered);
        Assert.Contains("control-plane restart", recovered);
        Assert.Contains("recovery re-run", recovered);
    }

    // ── Sweep behaviour (single watchdog tick against a seeded DB) ───────

    [Fact]
    public async Task deploy_within_its_scaled_budget_is_never_watchdog_killed()
    {
        using var sp = RunDispatcherTesterFkTests.BuildHost(nameof(WatchdogDeploymentBudgetTests));
        var db = Db(sp);
        // 33 minutes in: past the old flat 30m cutoff, well under the 38m
        // scaled budget (+5m slack). The exact #804 field kill.
        var id = SeedDeployment(db, CppConfig, "running",
            startedAt: DateTime.UtcNow.AddMinutes(-33));

        await WatchdogTickHarness.RunOnceAsync(sp, sp.GetRequiredService<AgentConnectionRegistry>());

        Assert.Equal("running", Row(sp, id).Status);
    }

    [Fact]
    public async Task deploy_past_budget_plus_slack_is_reaped_with_the_honest_message()
    {
        using var sp = RunDispatcherTesterFkTests.BuildHost(nameof(WatchdogDeploymentBudgetTests));
        var db = Db(sp);
        var id = SeedDeployment(db, CppConfig, "running",
            startedAt: DateTime.UtcNow.AddMinutes(-44)); // budget 38m + 5m slack = 43m

        await WatchdogTickHarness.RunOnceAsync(sp, sp.GetRequiredService<AgentConnectionRegistry>());

        var row = Row(sp, id);
        Assert.Equal("failed", row.Status);
        Assert.NotNull(row.FinishedAt);
        Assert.Equal(
            "Deployment did not finish within its 38m budget (1 language)",
            row.ErrorMessage);
    }

    [Fact]
    public async Task reaped_recovered_deploy_mentions_the_restart()
    {
        using var sp = RunDispatcherTesterFkTests.BuildHost(nameof(WatchdogDeploymentBudgetTests));
        var db = Db(sp);
        var id = SeedDeployment(db, CppConfig, "running",
            startedAt: DateTime.UtcNow.AddMinutes(-44), recoveryAttempts: 1);

        await WatchdogTickHarness.RunOnceAsync(sp, sp.GetRequiredService<AgentConnectionRegistry>());

        var row = Row(sp, id);
        Assert.Equal("failed", row.Status);
        Assert.Contains("control-plane restart", row.ErrorMessage);
    }

    [Fact]
    public async Task pre_v052_rows_with_null_started_at_age_by_created_at()
    {
        using var sp = RunDispatcherTesterFkTests.BuildHost(nameof(WatchdogDeploymentBudgetTests));
        var db = Db(sp);
        var id = SeedDeployment(db, StackOnlyConfig, "pending",
            startedAt: null, createdAt: DateTime.UtcNow.AddMinutes(-40)); // 30m + 5m slack = 35m

        await WatchdogTickHarness.RunOnceAsync(sp, sp.GetRequiredService<AgentConnectionRegistry>());

        Assert.Equal("failed", Row(sp, id).Status);
    }

    /// <summary>The #785 interplay pinned end-to-end: a crash-orphaned
    /// deployment far past ANY budget is re-claimed by the recovery pass
    /// (which stamps <c>started_at</c>), so the watchdog tick right after it
    /// measures the NEW attempt — not the original <c>created_at</c> — and
    /// leaves the recovered re-run its full fresh window.</summary>
    [Fact]
    public async Task recovery_reclaim_resets_the_watchdog_aging_basis()
    {
        using var sp = RunDispatcherTesterFkTests.BuildHost(nameof(WatchdogDeploymentBudgetTests));
        var db = Db(sp);
        var twoHoursAgo = DateTime.UtcNow.AddHours(-2);
        var id = SeedDeployment(db, CppConfig, "running",
            startedAt: twoHoursAgo, createdAt: twoHoursAgo);

        var recovered = await DeploymentRecoveryService.RecoverAsync(
            db, Microsoft.Extensions.Logging.Abstractions.NullLogger.Instance, CancellationToken.None);
        Assert.Equal(id, Assert.Single(recovered).DeploymentId);

        await WatchdogTickHarness.RunOnceAsync(sp, sp.GetRequiredService<AgentConnectionRegistry>());

        // Claimed back to pending with a fresh started_at — NOT reaped.
        var row = Row(sp, id);
        Assert.Equal("pending", row.Status);
        Assert.Null(row.ErrorMessage);
        Assert.True((row.StartedAt ?? row.CreatedAt) > DateTime.UtcNow.AddMinutes(-1));
    }

    [Fact]
    public async Task without_recovery_the_crash_orphan_is_still_reaped()
    {
        using var sp = RunDispatcherTesterFkTests.BuildHost(nameof(WatchdogDeploymentBudgetTests));
        var db = Db(sp);
        var twoHoursAgo = DateTime.UtcNow.AddHours(-2);
        var id = SeedDeployment(db, CppConfig, "running",
            startedAt: twoHoursAgo, createdAt: twoHoursAgo);

        await WatchdogTickHarness.RunOnceAsync(sp, sp.GetRequiredService<AgentConnectionRegistry>());

        Assert.Equal("failed", Row(sp, id).Status);
    }

    // ── Harness ──────────────────────────────────────────────────────────

    private static NetworkerDbContext Db(IServiceProvider sp) =>
        sp.CreateScope().ServiceProvider.GetRequiredService<NetworkerDbContext>();

    private static Deployment Row(IServiceProvider sp, Guid id) =>
        Db(sp).Deployments.AsNoTracking().Single(d => d.DeploymentId == id);

    private static Guid SeedDeployment(
        NetworkerDbContext db,
        string config,
        string status,
        DateTime? startedAt,
        DateTime? createdAt = null,
        short recoveryAttempts = 0)
    {
        if (!db.Projects.Any(p => p.ProjectId == ProjectId))
        {
            db.Projects.Add(new Project
            {
                ProjectId = ProjectId,
                Name = "wd-budget",
                Slug = "wd-budget",
                Settings = "{}",
                CreatedAt = DateTime.UtcNow,
                UpdatedAt = DateTime.UtcNow,
            });
        }
        var id = Guid.NewGuid();
        db.Deployments.Add(new Deployment
        {
            DeploymentId = id,
            Name = $"dep-{id:N}",
            Status = status,
            Config = config,
            StartedAt = startedAt,
            CreatedAt = createdAt ?? DateTime.UtcNow.AddHours(-3),
            RecoveryAttempts = recoveryAttempts,
            ProjectId = ProjectId,
        });
        db.SaveChanges();
        return id;
    }
}
