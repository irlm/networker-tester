using System.Text;
using System.Text.Json.Nodes;
using Microsoft.Data.Sqlite;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Provisioning;
using Networker.ControlPlane.Realtime;
using Networker.Data;
using Networker.Data.Entities;
using Networker.Security;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pins the deploy.json GCP zone for auto-provisioned endpoint deployments
/// (#831 — the endpoint-deploy sibling of #829): <c>BuildDeployJson</c> used to
/// hardcode <c>"zone": "{region}-a"</c>, and install.sh consumes that value
/// verbatim as <c>$GCP_ZONE</c>, so every GCP comparison cell / endpoint deploy
/// in a region without an "-a" zone (us-east1, europe-west1) failed at VM
/// create. KickOneAsync now resolves the zone through the singleton
/// <see cref="CliComputeProvisioner"/>'s listing cache (shared with the
/// tester-create path) and passes it into the builder; the "-a" guess survives
/// only as the documented no-credentials fallback.
///
/// <para>Behavioural: <c>GCLOUD_CMD</c> points at a fake POSIX-sh gcloud (call
/// log + canned zones listing), so the assertions cover the real decrypt →
/// key-file → gcloud spawn path. Windows returns early (no <c>/bin/sh</c>).
/// Shares the <c>cloud-cli-fake-bins</c> collection so *_CMD env mutation
/// cannot race the sibling fake-CLI tests.</para>
/// </summary>
[Collection("cloud-cli-fake-bins")]
public sealed class DeployJsonGcpZoneTests : IDisposable
{
    private const string ProjectId = "proj-zone-0001";

    private const string JsonKey =
        /*lang=json*/ """{"type":"service_account","project_id":"proj-1","private_key":"fake"}""";

    /// <summary>us-east1's real shape: no -a zone; -b DOWN exercises the status
    /// filter; -c is the first UP zone by ordinal name.</summary>
    private const string ZonesJson =
        /*lang=json*/ """[{"name":"us-east1-d","status":"UP"},{"name":"us-east1-b","status":"DOWN"},{"name":"us-east1-c","status":"UP"}]""";

    private readonly string _workDir;
    private readonly string _callLogPath;
    private readonly string? _previousInstallSh;

    public DeployJsonGcpZoneTests()
    {
        _workDir = Path.Combine(Path.GetTempPath(), $"deploy-zone-test-{Guid.NewGuid():N}");
        Directory.CreateDirectory(_workDir);
        _callLogPath = Path.Combine(_workDir, "calls.log");

        // KickOneAsync spawns the REAL DeployRunner detached; without an
        // override it walks up from cwd and finds the repo's install.sh — which,
        // handed a GCP deploy.json on a host with live gcloud auth, would try to
        // provision an actual VM from a unit test. Pin it to an inert stub for
        // the class lifetime (each test also awaits deploy terminality before
        // returning, so the env var is never cleared under a pending spawn).
        var stub = Path.Combine(_workDir, "install-stub.sh");
        File.WriteAllText(stub, "#!/bin/sh\nexit 0\n");
        _previousInstallSh = Environment.GetEnvironmentVariable("INSTALL_SH_PATH");
        Environment.SetEnvironmentVariable("INSTALL_SH_PATH", stub);
    }

    public void Dispose()
    {
        Environment.SetEnvironmentVariable("GCLOUD_CMD", null);
        Environment.SetEnvironmentVariable("INSTALL_SH_PATH", _previousInstallSh);
        try
        {
            Directory.Delete(_workDir, recursive: true);
        }
        catch
        {
            // best-effort temp cleanup
        }
    }

    [Fact]
    public async Task Kicked_gcp_deploy_carries_the_resolved_zone_and_shares_the_listing_cache()
    {
        if (OperatingSystem.IsWindows())
        {
            return; // fake CLI needs /bin/sh
        }

        WriteFakeGcloud(zonesBody: $"printf '%s' '{ZonesJson}'\nexit 0");
        var (sp, conn) = BuildHost(withCipherAndProvisioner: true);
        using var _ = conn;

        Guid firstRunId, secondRunId;
        using (var db = Db(sp))
        {
            var accountId = SeedProjectAndAccount(db, sp.GetRequiredService<CredentialCipher>(), JsonKey);
            var (runA, cfgA) = SeedQueuedRun(db, accountId);
            var (runB, cfgB) = SeedQueuedRun(db, accountId);
            firstRunId = runA.Id;
            secondRunId = runB.Id;

            var orchestrator = Orchestrator(sp);
            Assert.True(await orchestrator.KickOneAsync(db, runA, cfgA, CancellationToken.None));
            Assert.True(await orchestrator.KickOneAsync(db, runB, cfgB, CancellationToken.None));
        }

        await AwaitDeploysTerminalAsync(sp, firstRunId, secondRunId);

        using var check = Db(sp);
        Assert.Equal("us-east1-c", ZoneOfRun(check, firstRunId));
        Assert.Equal("us-east1-c", ZoneOfRun(check, secondRunId));

        // One listing for the pair: the second kick is a cache hit on the SAME
        // singleton provisioner cache the tester-create path uses (#830).
        var calls = File.ReadAllLines(_callLogPath);
        Assert.Equal(1, calls.Count(c => c.StartsWith("compute zones list", StringComparison.Ordinal)));
        Assert.Contains("--filter=region:(us-east1)", calls[0], StringComparison.Ordinal);
    }

    [Fact]
    public async Task Failed_zone_listing_falls_back_to_dash_a()
    {
        if (OperatingSystem.IsWindows())
        {
            return; // fake CLI needs /bin/sh
        }

        WriteFakeGcloud(zonesBody: "printf '%s\\n' 'boom-listing' >&2\nexit 1");
        var (sp, conn) = BuildHost(withCipherAndProvisioner: true);
        using var _ = conn;

        Guid runId;
        using (var db = Db(sp))
        {
            var accountId = SeedProjectAndAccount(db, sp.GetRequiredService<CredentialCipher>(), JsonKey);
            var (run, cfg) = SeedQueuedRun(db, accountId);
            runId = run.Id;
            Assert.True(await Orchestrator(sp).KickOneAsync(db, run, cfg, CancellationToken.None));
        }

        await AwaitDeploysTerminalAsync(sp, runId);

        // The old behaviour is the floor: the deploy still kicks, against -a.
        using var check = Db(sp);
        Assert.Equal("us-east1-a", ZoneOfRun(check, runId));
    }

    [Fact]
    public async Task Account_without_json_key_keeps_the_dash_a_guess_without_calling_gcloud()
    {
        if (OperatingSystem.IsWindows())
        {
            return; // fake CLI needs /bin/sh
        }

        WriteFakeGcloud(zonesBody: $"printf '%s' '{ZonesJson}'\nexit 0");
        var (sp, conn) = BuildHost(withCipherAndProvisioner: true);
        using var _ = conn;

        Guid runId;
        using (var db = Db(sp))
        {
            var accountId = SeedProjectAndAccount(db, sp.GetRequiredService<CredentialCipher>(), credentialJson: "{}");
            var (run, cfg) = SeedQueuedRun(db, accountId);
            runId = run.Id;
            Assert.True(await Orchestrator(sp).KickOneAsync(db, run, cfg, CancellationToken.None));
        }

        await AwaitDeploysTerminalAsync(sp, runId);

        using var check = Db(sp);
        Assert.Equal("us-east1-a", ZoneOfRun(check, runId));
        Assert.False(File.Exists(_callLogPath)); // no credentials → no gcloud roundtrip
    }

    [Fact]
    public async Task Bare_host_without_cipher_keeps_the_dash_a_guess()
    {
        // No /bin/sh needed — the resolution path exits before any gcloud spawn.
        var (sp, conn) = BuildHost(withCipherAndProvisioner: false);
        using var _ = conn;

        Guid runId;
        using (var db = Db(sp))
        {
            var accountId = SeedProjectAndAccount(db, cipher: null, JsonKey);
            var (run, cfg) = SeedQueuedRun(db, accountId);
            runId = run.Id;
            Assert.True(await Orchestrator(sp).KickOneAsync(db, run, cfg, CancellationToken.None));
        }

        await AwaitDeploysTerminalAsync(sp, runId);

        using var check = Db(sp);
        Assert.Equal("us-east1-a", ZoneOfRun(check, runId));
    }

    // ── Harness (mirrors KickOneAccountGateTests) ─────────────────────────────

    private static (ServiceProvider Sp, SqliteConnection Conn) BuildHost(bool withCipherAndProvisioner)
    {
        // Named shared-cache in-memory DB (NOT a single shared SqliteConnection):
        // the kicked deploy runner runs on a detached background task, so it and
        // the test thread would otherwise race on one connection. The returned
        // keeper connection holds the database alive; every DbContext opens its
        // own connection to it.
        var connString = $"DataSource=deploy-zone-{Guid.NewGuid():N};Mode=Memory;Cache=Shared";
        var conn = new SqliteConnection(connString);
        conn.Open();

        var services = new ServiceCollection();
        services.AddLogging(b => b.SetMinimumLevel(LogLevel.Warning));
        services.AddSignalR();
        services.AddAgentProtocol();
        services.AddDashboardEventBus();
        services.AddDbContext<NetworkerDbContext>(o => o.UseSqlite(connString));
        if (withCipherAndProvisioner)
        {
            services.AddSingleton(new CredentialCipher(TestKey));
            services.AddSingleton<CliComputeProvisioner>();
        }

        var sp = services.BuildServiceProvider();
        RunDispatcherTesterFkTests.CreateMinimalSchema(conn);
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

    private static readonly byte[] TestKey = Enumerable.Range(0, 32).Select(i => (byte)i).ToArray();

    private static ProvisioningOrchestrator Orchestrator(IServiceProvider sp) => new(
        sp.GetRequiredService<IServiceScopeFactory>(),
        new DeployRunner(
            sp.GetRequiredService<IServiceScopeFactory>(),
            sp.GetRequiredService<EventBus>(),
            sp.GetRequiredService<ILogger<DeployRunner>>()),
        sp.GetRequiredService<ILogger<ProvisioningOrchestrator>>());

    private static NetworkerDbContext Db(IServiceProvider sp) =>
        sp.CreateScope().ServiceProvider.GetRequiredService<NetworkerDbContext>();

    private static Guid SeedProjectAndAccount(
        NetworkerDbContext db, CredentialCipher? cipher, string? jsonKey = null, string? credentialJson = null)
    {
        var now = DateTime.UtcNow;
        db.Projects.Add(new Project
        {
            ProjectId = ProjectId,
            Name = "zone",
            Slug = "zone",
            Settings = "{}",
            CreatedAt = now,
            UpdatedAt = now,
        });

        credentialJson ??= $$"""{"json_key":{{System.Text.Json.JsonSerializer.Serialize(jsonKey)}}}""";
        byte[] enc = [], nonce = [];
        if (cipher is not null)
        {
            (enc, nonce) = cipher.Encrypt(Encoding.UTF8.GetBytes(credentialJson));
        }

        var accountId = Guid.NewGuid();
        db.CloudAccounts.Add(new CloudAccount
        {
            AccountId = accountId,
            Name = "GCP prod",
            Provider = "gcp",
            Status = "active",
            CredentialsEnc = enc,
            CredentialsNonce = nonce,
            CreatedAt = now,
            UpdatedAt = now,
            ProjectId = ProjectId,
        });
        db.SaveChanges();
        return accountId;
    }

    private static (TestRun Run, TestConfig Cfg) SeedQueuedRun(NetworkerDbContext db, Guid accountId)
    {
        var now = DateTime.UtcNow;
        var cfg = new TestConfig
        {
            Id = Guid.NewGuid(),
            ProjectId = ProjectId,
            Name = $"cell-{Guid.NewGuid():N}",
            EndpointKind = "pending",
            EndpointRef = $$"""{"kind":"pending","cloud_account_id":"{{accountId}}","region":"us-east1","vm_size":"e2-small","os":"linux","proxy_stack":"nginx"}""",
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
        return (run, cfg);
    }

    /// <summary>Wait for every kicked run's background deploy (the inert
    /// install.sh stub) to reach a terminal status, so no detached task is
    /// still running when the test tears its host/env down.</summary>
    private static async Task AwaitDeploysTerminalAsync(IServiceProvider sp, params Guid[] runIds)
    {
        var deadline = DateTime.UtcNow + TimeSpan.FromSeconds(30);
        while (DateTime.UtcNow < deadline)
        {
            using var db = Db(sp);
            var deploymentIds = db.TestRuns.AsNoTracking()
                .Where(r => runIds.Contains(r.Id))
                .Select(r => r.ProvisioningDeploymentId)
                .ToList();
            if (deploymentIds.Count == runIds.Length && deploymentIds.All(id => id is not null))
            {
                var ids = deploymentIds.Select(id => id!.Value).ToList();
                var statuses = db.Deployments.AsNoTracking()
                    .Where(d => ids.Contains(d.DeploymentId))
                    .Select(d => d.Status)
                    .ToList();
                if (statuses.Count == ids.Count
                    && statuses.All(s => s is "completed" or "failed"))
                {
                    return;
                }
            }

            await Task.Delay(50);
        }

        Assert.Fail("kicked deploy(s) never reached a terminal status");
    }

    private static string ZoneOfRun(NetworkerDbContext db, Guid runId)
    {
        var deploymentId = db.TestRuns.Single(r => r.Id == runId).ProvisioningDeploymentId;
        Assert.NotNull(deploymentId);
        var config = db.Deployments.Single(d => d.DeploymentId == deploymentId).Config;
        var zone = JsonNode.Parse(config!)?["endpoints"]?[0]?["gcp"]?["zone"]?.GetValue<string>();
        Assert.NotNull(zone);
        return zone!;
    }

    /// <summary>Fake gcloud via <c>GCLOUD_CMD</c>: logs each argv, answers
    /// <c>compute zones list</c> with <paramref name="zonesBody"/>; anything
    /// else (nothing should reach it from this path) fails loudly.</summary>
    private void WriteFakeGcloud(string zonesBody)
    {
        var path = Path.Combine(_workDir, "fake-gcloud.sh");
        File.WriteAllText(
            path,
            "#!/bin/sh\n" +
            $"printf '%s\\n' \"$*\" >> '{_callLogPath}'\n" +
            "case \"$*\" in\n" +
            "  \"compute zones list\"*)\n" +
            $"{zonesBody}\n" +
            "  ;;\n" +
            "  *)\n" +
            "printf '%s\\n' 'unexpected gcloud invocation' >&2\nexit 1\n" +
            "  ;;\n" +
            "esac\n");
        if (!OperatingSystem.IsWindows())
        {
            File.SetUnixFileMode(
                path,
                UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute);
        }

        Environment.SetEnvironmentVariable("GCLOUD_CMD", path);
    }
}

/// <summary>Shape tests for <c>BuildDeployJson</c>'s zone parameter — the pure
/// builder half of #831, no fake CLI involved.</summary>
public class DeployJsonGcpZoneShapeTests
{
    private static readonly ProvisioningOrchestrator.PendingEndpoint Pending = new(
        Guid.NewGuid(), "us-east1", "e2-small", "linux", "nginx", null);

    private static string Zone(JsonObject json) =>
        json["endpoints"]![0]!["gcp"]!["zone"]!.GetValue<string>();

    [Fact]
    public void Resolved_zone_lands_in_deploy_json()
        => Assert.Equal("us-east1-c", Zone(ProvisioningOrchestrator.BuildDeployJson(
            Pending, "gcp", "cfg", Guid.NewGuid(), gcpZone: "us-east1-c")));

    [Fact]
    public void Null_zone_keeps_the_documented_region_dash_a_fallback()
        => Assert.Equal("us-east1-a", Zone(ProvisioningOrchestrator.BuildDeployJson(
            Pending, "gcp", "cfg", Guid.NewGuid(), gcpZone: null)));
}
