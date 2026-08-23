using System.Text;
using Microsoft.Data.Sqlite;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Background;
using Networker.ControlPlane.Provisioning;
using Networker.Data;
using Networker.Data.Entities;
using Networker.Security;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Tester-lifecycle GCP credentials (#857): on prod every auto-shutdown tick for
/// the idle GCP runner failed with
/// <c>ERROR: (gcloud.compute.instances.stop) You do not currently have an active
/// account selected</c>, because the sweep resolved credentials from the
/// <c>cloud_connection</c> config alone and never loaded/decrypted the cloud
/// ACCOUNT's <c>json_key</c>. gcloud has no ambient-auth mode (#827), so the VM
/// billed for three days. Azure — whose scope lives in the connection config and
/// which DOES have ambient auth — kept working, which is why it went unnoticed.
///
/// <para>Two layers of assertion:</para>
/// <list type="bullet">
///   <item>Through a capturing provisioner: the credentials the sweep hands
///   <c>DeallocateAsync</c> / <c>StartAsync</c> carry the account key for GCP,
///   and Azure's are byte-for-byte what they always were.</item>
///   <item>Through the REAL <see cref="CliComputeProvisioner"/> against a fake
///   <c>gcloud</c> (<c>GCLOUD_CMD</c>, the
///   <see cref="CliProvisionerGcpTeardownTests"/> harness): the process actually
///   receives <c>CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE</c> pointing at a file
///   holding the key, and the tester lands <c>power_state=stopped</c>.</item>
/// </list>
///
/// <para>Shares the <c>cloud-cli-fake-bins</c> collection so the <c>*_CMD</c>
/// env mutation cannot race the sibling fake-CLI tests. The end-to-end cases
/// need <c>/bin/sh</c> and return early on Windows; the capturing-provisioner
/// cases run everywhere. A live GCP stop is NOT exercised anywhere.</para>
/// </summary>
[Collection("cloud-cli-fake-bins")]
public sealed class AutoShutdownGcpCredentialsTests : IDisposable
{
    private const string ProjectId = "proj-gcp-shut1";

    private const string JsonKey =
        /*lang=json*/ """{"type":"service_account","project_id":"kepler-1","client_email":"vms@kepler-1.iam.gserviceaccount.com","private_key":"fake"}""";

    /// <summary>What gcloud printed on prod when the stop ran unauthenticated.
    /// The status_message must keep quoting it (that is what made #857
    /// diagnosable) and must never grow key material.</summary>
    private const string NoAccountStderr =
        "ERROR: (gcloud.compute.instances.stop) You do not currently have an active account selected.";

    private const string SelfLink =
        "https://www.googleapis.com/compute/v1/projects/kepler-1/zones/us-east1-b/instances/gcp-useast1-03";

    private static readonly byte[] TestKey = Enumerable.Range(0, 32).Select(i => (byte)i).ToArray();

    private readonly string _workDir;
    private readonly string _callLogPath;
    private readonly string? _previousGcloud;

    public AutoShutdownGcpCredentialsTests()
    {
        _workDir = Path.Combine(Path.GetTempPath(), $"autoshutdown-gcp-test-{Guid.NewGuid():N}");
        Directory.CreateDirectory(_workDir);
        _callLogPath = Path.Combine(_workDir, "calls.log");
        _previousGcloud = Environment.GetEnvironmentVariable(CloudCli.GcloudOverrideVar);
    }

    public void Dispose()
    {
        Environment.SetEnvironmentVariable(CloudCli.GcloudOverrideVar, _previousGcloud);
        try
        {
            Directory.Delete(_workDir, recursive: true);
        }
        catch
        {
            // best-effort temp cleanup
        }
    }

    // ── The credentials the sweep hands the provisioner ───────────────────────

    [Fact]
    public async Task Gcp_auto_shutdown_hands_the_provisioner_the_accounts_decrypted_key()
    {
        var (sp, conn, prov) = BuildHost(withCipher: true);
        using var _ = conn;
        Guid testerId;
        using (var db = Db(sp))
        {
            var accountId = SeedAccount(db, sp.GetRequiredService<CredentialCipher>(), JsonKey);
            testerId = SeedDueTester(db, cloud: "gcp", accountId: accountId);
        }

        await RunSweepOnceAsync(sp);

        var creds = Assert.Single(prov.DeallocateCredentials);
        Assert.NotNull(creds);
        Assert.Equal("gcp", creds!.Provider);
        // The whole point: the key travels to the provisioner, which turns it
        // into gcloud's credential-override env.
        Assert.Equal(JsonKey, creds.Extra?.GetValueOrDefault("json_key"));

        using var check = Db(sp);
        var tester = await check.ProjectTesters.AsNoTracking().FirstAsync(t => t.TesterId == testerId);
        Assert.Equal("stopped", tester.PowerState);
        Assert.Equal("auto-shutdown completed", tester.StatusMessage);
    }

    [Fact]
    public async Task Gcp_auto_wake_hands_the_provisioner_the_accounts_decrypted_key()
    {
        var (sp, conn, prov) = BuildHost(withCipher: true);
        using var _ = conn;
        using (var db = Db(sp))
        {
            var accountId = SeedAccount(db, sp.GetRequiredService<CredentialCipher>(), JsonKey);
            var testerId = SeedDueTester(db, cloud: "gcp", accountId: accountId, powerState: "stopped");
            SeedQueuedRun(db, testerId);
        }

        await RunSweepOnceAsync(sp);

        // The wake arm ran under the same resolution, not the old connection-only one.
        var creds = Assert.Single(prov.StartCredentials);
        Assert.Equal(JsonKey, creds?.Extra?.GetValueOrDefault("json_key"));
    }

    [Fact]
    public async Task Azure_auto_shutdown_credentials_are_unchanged()
    {
        var (sp, conn, prov) = BuildHost(withCipher: true);
        using var _ = conn;
        using (var db = Db(sp))
        {
            // A GCP account exists in the same project — it must NOT leak into
            // an Azure tester's credentials.
            SeedAccount(db, sp.GetRequiredService<CredentialCipher>(), JsonKey);
            var connectionId = SeedConnection(
                db, "azure",
                /*lang=json*/ """{"subscription_id":"sub-1","resource_group":"networker-testers","region":"eastus"}""");
            SeedDueTester(db, cloud: "azure", accountId: null, connectionId: connectionId);
        }

        await RunSweepOnceAsync(sp);

        var creds = Assert.Single(prov.DeallocateCredentials);
        Assert.NotNull(creds);
        Assert.Equal("azure", creds!.Provider);
        Assert.Equal("sub-1", creds.SubscriptionId);
        Assert.Equal("networker-testers", creds.ResourceGroup);
        Assert.Equal("eastus", creds.Region);
        Assert.Null(creds.Extra?.GetValueOrDefault("json_key"));
    }

    [Fact]
    public async Task Gcp_tester_with_no_resolvable_key_soft_fails_to_ambient_instead_of_throwing()
    {
        // No cipher registered (bare host) — the pre-#857 behaviour, kept: the
        // sweep must not throw, and the CLI-unavailable convergence still holds.
        var (sp, conn, prov) = BuildHost(withCipher: false);
        using var _ = conn;
        Guid testerId;
        using (var db = Db(sp))
        {
            testerId = SeedDueTester(db, cloud: "gcp", accountId: null);
        }
        prov.Result = () => ProvisionResult.SpawnError("gcloud not installed");

        await RunSweepOnceAsync(sp);

        var creds = Assert.Single(prov.DeallocateCredentials);
        Assert.Null(creds?.Extra?.GetValueOrDefault("json_key"));

        using var check = Db(sp);
        var tester = await check.ProjectTesters.AsNoTracking().FirstAsync(t => t.TesterId == testerId);
        // ExitCode == null stays a SOFT success: a credential-less/CI host
        // converges to 'stopped' instead of wedging in 'stopping'.
        Assert.Equal("stopped", tester.PowerState);
        Assert.Equal("auto-shutdown completed (cloud CLI unavailable — state assumed)", tester.StatusMessage);
    }

    [Fact]
    public async Task A_genuine_cli_failure_still_rolls_back_and_quotes_the_cli_without_leaking_the_key()
    {
        var (sp, conn, prov) = BuildHost(withCipher: true);
        using var _ = conn;
        Guid testerId;
        using (var db = Db(sp))
        {
            var accountId = SeedAccount(db, sp.GetRequiredService<CredentialCipher>(), JsonKey);
            testerId = SeedDueTester(db, cloud: "gcp", accountId: accountId);
        }
        prov.Result = () => ProvisionResult.Failed(1, string.Empty, NoAccountStderr);

        await RunSweepOnceAsync(sp);

        using var check = Db(sp);
        var tester = await check.ProjectTesters.AsNoTracking().FirstAsync(t => t.TesterId == testerId);
        Assert.Equal("running", tester.PowerState); // rolled back for the next tick
        Assert.Contains(NoAccountStderr, tester.StatusMessage);
        // The message quotes gcloud, never the credential.
        Assert.DoesNotContain("private_key", tester.StatusMessage);
        Assert.DoesNotContain("service_account", tester.StatusMessage);
    }

    // ── End to end: real provisioner, fake gcloud ─────────────────────────────

    [Fact]
    public async Task Gcp_auto_shutdown_runs_gcloud_with_the_credential_override_and_reaches_stopped()
    {
        if (OperatingSystem.IsWindows())
        {
            return; // fake CLI needs /bin/sh
        }

        WriteFakeGcloud();
        var (sp, conn, _) = BuildHost(withCipher: true, realProvisioner: true);
        using var __ = conn;
        Guid testerId;
        using (var db = Db(sp))
        {
            var accountId = SeedAccount(db, sp.GetRequiredService<CredentialCipher>(), JsonKey);
            testerId = SeedDueTester(db, cloud: "gcp", accountId: accountId);
        }

        await RunSweepOnceAsync(sp);

        var stop = Assert.Single(ReadCalls());
        Assert.Equal("compute instances stop gcp-useast1-03 --zone us-east1-b", stop.Argv);
        // #827's contract: the var gcloud itself reads, pointing at the key.
        Assert.False(string.IsNullOrEmpty(stop.Override), "CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE must be set");
        Assert.Equal(JsonKey, stop.KeyContent);
        Assert.Equal("kepler-1", stop.Project);
        Assert.False(File.Exists(stop.Override)); // 0600 tempfile deleted after the call

        using var check = Db(sp);
        var tester = await check.ProjectTesters.AsNoTracking().FirstAsync(t => t.TesterId == testerId);
        Assert.Equal("stopped", tester.PowerState);
        Assert.Equal("auto-shutdown completed", tester.StatusMessage);
    }

    [Fact]
    public async Task Without_the_key_the_prod_failure_reproduces_verbatim()
    {
        if (OperatingSystem.IsWindows())
        {
            return;
        }

        // The fake gcloud behaves like the real one on an unauthenticated host:
        // no credential override → exit 1 with the "no active account" error.
        WriteFakeGcloud();
        var (sp, conn, _) = BuildHost(withCipher: false, realProvisioner: true);
        using var __ = conn;
        Guid testerId;
        using (var db = Db(sp))
        {
            testerId = SeedDueTester(db, cloud: "gcp", accountId: null);
        }

        await RunSweepOnceAsync(sp);

        Assert.Equal(string.Empty, Assert.Single(ReadCalls()).Override);

        using var check = Db(sp);
        var tester = await check.ProjectTesters.AsNoTracking().FirstAsync(t => t.TesterId == testerId);
        Assert.Equal("running", tester.PowerState);
        Assert.Contains("auto-shutdown deallocate failed", tester.StatusMessage);
        Assert.Contains("You do not currently have an active account selected", tester.StatusMessage);
    }

    // ── Harness ───────────────────────────────────────────────────────────────

    private sealed class CapturingProvisioner : IComputeProvisioner
    {
        public readonly List<ProviderCredentials?> DeallocateCredentials = [];
        public readonly List<ProviderCredentials?> StartCredentials = [];
        public Func<ProvisionResult> Result = () => new ProvisionResult(true, 0, "", "");

        public Task<ProvisionResult> StartAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default)
        {
            StartCredentials.Add(credentials);
            return Task.FromResult(Result());
        }

        public Task<ProvisionResult> StopAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default)
            => Task.FromResult(Result());

        public Task<ProvisionResult> DeallocateAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default)
        {
            DeallocateCredentials.Add(credentials);
            return Task.FromResult(Result());
        }

        public Task<ProvisionResult> DeleteAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default)
            => Task.FromResult(Result());

        public Task<ProvisionResult> ShowAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default)
            => Task.FromResult(Result());

        public Task<ProvisionResult> RunCommandAsync(ProjectTester tester, ProviderCredentials? credentials, string script, CancellationToken ct = default)
            => Task.FromResult(Result());

        public Task<VmCreateResult> CreateVmAsync(VmCreateRequest request, ProviderCredentials? credentials, CancellationToken ct = default)
            => throw new NotSupportedException();

        public Task<ResolvedVm?> ResolveByEndpointAsync(string provider, ProviderCredentials? credentials, string endpoint, CancellationToken ct = default)
            => Task.FromResult<ResolvedVm?>(null);
    }

    private static (ServiceProvider Sp, SqliteConnection Conn, CapturingProvisioner Prov) BuildHost(
        bool withCipher, bool realProvisioner = false)
    {
        var conn = new SqliteConnection("DataSource=:memory:");
        conn.Open();

        var prov = new CapturingProvisioner();
        var services = new ServiceCollection();
        services.AddLogging(b => b.SetMinimumLevel(LogLevel.Warning));
        services.AddDbContext<NetworkerDbContext>(o => o.UseSqlite(conn));
        if (realProvisioner)
        {
            services.AddSingleton<IComputeProvisioner>(s => new CliComputeProvisioner(
                s.GetRequiredService<ILogger<CliComputeProvisioner>>()));
        }
        else
        {
            services.AddSingleton<IComputeProvisioner>(prov);
        }
        if (withCipher)
        {
            services.AddSingleton(new CredentialCipher(TestKey));
        }

        var sp = services.BuildServiceProvider();
        RunDispatcherTesterFkTests.CreateMinimalSchema(conn);
        // cloud_account / cloud_connection are not in the shared minimal schema
        // (only the tables the dispatch tests need); real column names here.
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
        RunDispatcherTesterFkTests.Exec(conn, """
            CREATE TABLE cloud_connection (
                connection_id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                provider TEXT NOT NULL,
                config TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                last_validated TEXT,
                validation_error TEXT,
                created_by TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                project_id TEXT
            );
            """);
        return (sp, conn, prov);
    }

    private static NetworkerDbContext Db(IServiceProvider sp) =>
        sp.CreateScope().ServiceProvider.GetRequiredService<NetworkerDbContext>();

    private static async Task RunSweepOnceAsync(IServiceProvider sp)
    {
        var svc = new AutoShutdownService(
            sp.GetRequiredService<IServiceScopeFactory>(),
            sp.GetRequiredService<ILogger<AutoShutdownService>>());
        var sweep = typeof(AutoShutdownService).GetMethod(
            "SweepAsync",
            System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic)!;
        await (Task)sweep.Invoke(svc, [CancellationToken.None])!;
    }

    private static void EnsureProject(NetworkerDbContext db)
    {
        if (db.Projects.Any(p => p.ProjectId == ProjectId))
        {
            return;
        }
        var now = DateTime.UtcNow;
        db.Projects.Add(new Project
        {
            ProjectId = ProjectId,
            Name = "gcp-shutdown",
            Slug = "gcp-shutdown",
            Settings = "{}",
            CreatedAt = now,
            UpdatedAt = now,
        });
        db.SaveChanges();
    }

    private static Guid SeedAccount(NetworkerDbContext db, CredentialCipher cipher, string jsonKey)
    {
        EnsureProject(db);
        var now = DateTime.UtcNow;
        var credentialJson = $$"""{"json_key":{{System.Text.Json.JsonSerializer.Serialize(jsonKey)}}}""";
        var (enc, nonce) = cipher.Encrypt(Encoding.UTF8.GetBytes(credentialJson));
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

    private static Guid SeedConnection(NetworkerDbContext db, string provider, string config)
    {
        EnsureProject(db);
        var now = DateTime.UtcNow;
        var connectionId = Guid.NewGuid();
        db.CloudConnections.Add(new CloudConnection
        {
            ConnectionId = connectionId,
            Name = $"{provider}-conn",
            Provider = provider,
            Config = config,
            Status = "active",
            ProjectId = ProjectId,
            CreatedAt = now,
            UpdatedAt = now,
        });
        db.SaveChanges();
        return connectionId;
    }

    /// <summary>The prod row shape: idle, auto-shutdown enabled, window already
    /// elapsed — i.e. a sweep candidate.</summary>
    private static Guid SeedDueTester(
        NetworkerDbContext db,
        string cloud,
        Guid? accountId,
        Guid? connectionId = null,
        string powerState = "running")
    {
        EnsureProject(db);
        var now = DateTime.UtcNow;
        var testerId = Guid.NewGuid();
        db.ProjectTesters.Add(new ProjectTester
        {
            TesterId = testerId,
            ProjectId = ProjectId,
            Name = cloud == "gcp" ? "gcp-useast1-03" : "eastus-runner-01",
            Cloud = cloud,
            Region = cloud == "gcp" ? "us-east1" : "eastus",
            VmSize = cloud == "gcp" ? "e2-standard-2" : "Standard_B2s",
            VmName = cloud == "gcp" ? "gcp-useast1-03" : "eastus-runner-01",
            VmResourceId = cloud == "gcp"
                ? SelfLink
                : "/subscriptions/sub-1/resourceGroups/networker-testers/providers/Microsoft.Compute/virtualMachines/eastus-runner-01",
            SshUser = cloud == "gcp" ? "gcpuser" : "azureuser",
            PowerState = powerState,
            Allocation = "idle",
            AutoShutdownEnabled = true,
            AutoShutdownLocalHour = 19,
            NextShutdownAt = now.AddDays(-3),
            ShutdownDeferralCount = 0,
            AutoProbeEnabled = false,
            BenchmarkRunCount = 0,
            CloudAccountId = accountId,
            CloudConnectionId = connectionId,
            CreatedBy = Guid.NewGuid(),
            CreatedAt = now,
            UpdatedAt = now,
        });
        db.SaveChanges();
        // auto_shutdown_enabled is mapped HasDefaultValue(true), so EF omits it
        // from the INSERT when the value equals that default and SQLite applies
        // the minimal schema's own DEFAULT 0 — the row would come back with
        // auto-shutdown OFF and never be a sweep candidate. Set it explicitly.
        db.ProjectTesters
            .Where(t => t.TesterId == testerId)
            .ExecuteUpdate(s => s.SetProperty(t => t.AutoShutdownEnabled, true));
        return testerId;
    }

    private static void SeedQueuedRun(NetworkerDbContext db, Guid testerId)
    {
        var now = DateTime.UtcNow;
        var configId = Guid.NewGuid();
        db.TestConfigs.Add(new TestConfig
        {
            Id = configId,
            ProjectId = ProjectId,
            Name = $"cfg-{configId:N}",
            EndpointKind = "network",
            EndpointRef = "{}",
            Workload = "{}",
            MaxDurationSecs = 60,
            CreatedAt = now,
            UpdatedAt = now,
        });
        db.TestRuns.Add(new TestRun
        {
            Id = Guid.NewGuid(),
            TestConfigId = configId,
            ProjectId = ProjectId,
            Status = "queued",
            TesterId = testerId,
            CreatedAt = now,
        });
        db.SaveChanges();
    }

    private sealed record Call(string Argv, string Override, string Project, string KeyContent);

    /// <summary>Fake gcloud via <c>GCLOUD_CMD</c>: records argv + the credential
    /// env + the key file's content, and — like the real CLI on an
    /// unauthenticated host — refuses with the prod error when no credential
    /// override is present.</summary>
    private void WriteFakeGcloud()
    {
        File.Delete(_callLogPath);
        var path = Path.Combine(_workDir, "fake-gcloud.sh");
        File.WriteAllText(
            path,
            "#!/bin/sh\n" +
            "{\n" +
            "  printf 'argv=%s\\n' \"$*\"\n" +
            "  printf 'override=%s\\n' \"$CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE\"\n" +
            "  printf 'project=%s\\n' \"$CLOUDSDK_CORE_PROJECT\"\n" +
            "  printf 'keycontent=%s\\n' \"$(cat \"$CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE\" 2>/dev/null)\"\n" +
            "  printf -- '--\\n'\n" +
            $"}} >> '{_callLogPath}'\n" +
            "if [ -z \"${CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE:-}\" ]; then\n" +
            $"  printf '%s\\n' '{NoAccountStderr}' >&2\n" +
            "  exit 1\n" +
            "fi\n" +
            "exit 0\n");
        if (!OperatingSystem.IsWindows())
        {
            // Guarded rather than suppressed: every caller returns early on
            // Windows anyway (no /bin/sh), and the guard keeps CA1416 quiet
            // without an analyzer suppression.
            File.SetUnixFileMode(
                path,
                UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute);
        }

        Environment.SetEnvironmentVariable(CloudCli.GcloudOverrideVar, path);
    }

    private List<Call> ReadCalls()
    {
        var calls = new List<Call>();
        if (!File.Exists(_callLogPath))
        {
            return calls;
        }
        var cur = new Dictionary<string, string>(StringComparer.Ordinal);
        foreach (var line in File.ReadAllLines(_callLogPath))
        {
            if (line == "--")
            {
                calls.Add(new Call(
                    cur.GetValueOrDefault("argv", ""),
                    cur.GetValueOrDefault("override", ""),
                    cur.GetValueOrDefault("project", ""),
                    cur.GetValueOrDefault("keycontent", "")));
                cur = new Dictionary<string, string>(StringComparer.Ordinal);
                continue;
            }
            var idx = line.IndexOf('=', StringComparison.Ordinal);
            if (idx > 0)
            {
                cur[line[..idx]] = line[(idx + 1)..];
            }
        }
        return calls;
    }
}
