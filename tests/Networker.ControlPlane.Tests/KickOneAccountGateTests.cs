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
/// #791 / #793 P1-1: the provisioning orchestrator kicked deploys against
/// cloud accounts already known to be broken (status <c>error</c> with a
/// stored <c>validation_error</c>) — each cell burned a full provision +
/// readiness timeout and failed with an unrelated cloud symptom. KickOneAsync
/// now fails the run up front with the account's own error via the normal
/// failure channel (<c>test_run.error_message</c>), which slice (c) renders
/// on the run page.
/// </summary>
public class KickOneAccountGateTests
{
    private const string ProjectId = "proj-gate-0001";

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
        // The minimal schema has no cloud_account table (nothing needed it) —
        // real column names so EF reads/writes succeed.
        RunDispatcherTesterFkTests.Exec(conn, """
            CREATE TABLE cloud_account (
                account_id TEXT PRIMARY KEY,
                owner_id TEXT,
                name TEXT NOT NULL,
                provider TEXT NOT NULL,
                credentials_enc BLOB NOT NULL,
                credentials_nonce BLOB NOT NULL,
                region_default TEXT,
                status TEXT NOT NULL DEFAULT 'active',
                last_validated TEXT,
                validation_error TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                project_id TEXT NOT NULL
            );
            """);
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

    private static (TestRun Run, TestConfig Cfg, Guid AccountId) SeedQueuedRun(
        NetworkerDbContext db, string accountStatus, string? validationError)
    {
        var now = DateTime.UtcNow;
        db.Projects.Add(new Project
        {
            ProjectId = ProjectId,
            Name = "gate",
            Slug = "gate",
            Settings = "{}",
            CreatedAt = now,
            UpdatedAt = now,
        });
        var accountId = Guid.NewGuid();
        db.CloudAccounts.Add(new CloudAccount
        {
            AccountId = accountId,
            Name = "AWS prod",
            Provider = "aws",
            Status = accountStatus,
            ValidationError = validationError,
            CredentialsEnc = [],
            CredentialsNonce = [],
            CreatedAt = now,
            UpdatedAt = now,
            ProjectId = ProjectId,
        });
        var cfg = new TestConfig
        {
            Id = Guid.NewGuid(),
            ProjectId = ProjectId,
            Name = $"cell-{Guid.NewGuid():N}",
            EndpointKind = "pending",
            EndpointRef = $$"""{"kind":"pending","cloud_account_id":"{{accountId}}","region":"us-east-1","vm_size":"t3.small","os":"linux","proxy_stack":"nginx"}""",
            Workload = "{}",
            MaxDurationSecs = 600,
            CreatedAt = now,
            UpdatedAt = now,
        };
        db.TestConfigs.Add(cfg);
        var run = new TestRun
        {
            Id = Guid.NewGuid(),
            TestConfigId = cfg.Id,
            ProjectId = ProjectId,
            Status = "queued",
            CreatedAt = now,
        };
        db.TestRuns.Add(run);
        db.SaveChanges();
        return (run, cfg, accountId);
    }

    [Fact]
    public async Task error_account_fails_the_run_with_its_validation_error_and_provisions_nothing()
    {
        var (sp, conn) = BuildHost();
        using var _ = conn;
        Guid runId;
        bool kicked;
        using (var db = Db(sp))
        {
            var (run, cfg, _) = SeedQueuedRun(db, "error", "Invalid access key ID");
            runId = run.Id;
            kicked = await Orchestrator(sp).KickOneAsync(db, run, cfg, CancellationToken.None);
        }

        Assert.False(kicked);
        using var check = Db(sp);
        var failed = check.TestRuns.Single(r => r.Id == runId);
        Assert.Equal("failed", failed.Status);
        Assert.Equal("cloud account 'AWS prod' is in error state: Invalid access key ID", failed.ErrorMessage);
        Assert.NotNull(failed.FinishedAt);
        Assert.Null(failed.ProvisioningDeploymentId);
        // The whole point: no deployment (→ no VM) was ever created.
        Assert.Empty(check.Deployments.ToList());
    }

    [Fact]
    public async Task non_active_account_without_validation_error_still_fails_with_the_status()
    {
        var (sp, conn) = BuildHost();
        using var _ = conn;
        Guid runId;
        using (var db = Db(sp))
        {
            var (run, cfg, _) = SeedQueuedRun(db, "validating", validationError: null);
            runId = run.Id;
            await Orchestrator(sp).KickOneAsync(db, run, cfg, CancellationToken.None);
        }

        using var check = Db(sp);
        var failed = check.TestRuns.Single(r => r.Id == runId);
        Assert.Equal("failed", failed.Status);
        Assert.Equal("cloud account 'AWS prod' is in validating state", failed.ErrorMessage);
    }

    [Fact]
    public async Task missing_account_keeps_the_existing_skip_behavior()
    {
        var (sp, conn) = BuildHost();
        using var _ = conn;
        Guid runId;
        bool kicked;
        using (var db = Db(sp))
        {
            var (run, cfg, accountId) = SeedQueuedRun(db, "active", null);
            runId = run.Id;
            // Point the config at an account that does not exist.
            cfg.EndpointRef = cfg.EndpointRef.Replace(accountId.ToString(), Guid.NewGuid().ToString());
            db.SaveChanges();
            kicked = await Orchestrator(sp).KickOneAsync(db, run, cfg, CancellationToken.None);
        }

        Assert.False(kicked);
        using var check = Db(sp);
        // Not-found stays a skip (transient DB shapes must not fail runs) —
        // the run remains queued and no deployment is created.
        Assert.Equal("queued", check.TestRuns.Single(r => r.Id == runId).Status);
        Assert.Empty(check.Deployments.ToList());
    }
}

/// <summary>Pins the shared message shape both launch gates emit.</summary>
public class CloudAccountGateMessageTests
{
    [Fact]
    public void active_is_never_blocked()
        => Assert.Null(CloudAccountGate.NotActiveReason("A", "active", "stale error text"));

    [Fact]
    public void error_with_detail_includes_it()
        => Assert.Equal(
            "cloud account 'AWS prod' is in error state: Invalid access key ID",
            CloudAccountGate.NotActiveReason("AWS prod", "error", "Invalid access key ID"));

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("   ")]
    public void blank_detail_is_omitted(string? detail)
        => Assert.Equal(
            "cloud account 'GCP lab' is in disabled state",
            CloudAccountGate.NotActiveReason("GCP lab", "disabled", detail));
}
