using System.Text;
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
/// Env contract for the install.sh process a GCP endpoint deploy spawns
/// (#833): <see cref="DeployRunner"/> used to launch <c>install.sh --deploy</c>
/// with no cloud credentials at all, so install.sh's GCP pre-flight — which
/// can only see gcloud's (never authenticated) config store — failed every
/// prod GCP comparison cell in seconds. The runner now stages the cloud
/// account's service-account key (0600, inside a 0700 throwaway
/// <c>CLOUDSDK_CONFIG</c>) and hands gcloud's credential-override env to the
/// installer, then deletes the whole staging dir.
///
/// <para>Behavioural through the REAL runner: <c>INSTALL_SH_PATH</c> points at
/// a probe script that records the env and file modes it was given, so the
/// assertions cover decrypt → key file → spawn → cleanup. Linux only (bash +
/// GNU stat + Unix modes). Shares the <c>cloud-cli-fake-bins</c> collection so
/// the INSTALL_SH_PATH mutation cannot race the sibling fake-CLI tests.</para>
/// </summary>
[Collection("cloud-cli-fake-bins")]
public sealed class DeployRunnerGcpCredentialEnvTests : IDisposable
{
    private const string ProjectId = "proj-gcp-env-0001";

    private const string JsonKey =
        /*lang=json*/ """{"type":"service_account","project_id":"proj-1","client_email":"deployer@proj-1.iam.gserviceaccount.com","private_key":"fake"}""";

    private const string GcpDeployJson =
        /*lang=json*/ """{"version":1,"tester":{"provider":"local"},"endpoints":[{"provider":"gcp","label":"cell","http_stacks":["nginx"],"gcp":{"region":"us-east1","zone":"us-east1-c","machine_type":"e2-small","os":"linux","instance_name":"nwk-a-1"}}]}""";

    private const string AzureDeployJson =
        /*lang=json*/ """{"version":1,"tester":{"provider":"local"},"endpoints":[{"provider":"azure","label":"cell","http_stacks":["nginx"],"azure":{"region":"eastus","vm_size":"Standard_B1s","os":"linux","vm_name":"nwk-a-1"}}]}""";

    private static readonly byte[] TestKey = Enumerable.Range(0, 32).Select(i => (byte)i).ToArray();

    private readonly string _workDir;
    private readonly string _probePath;
    private readonly string? _previousInstallSh;

    public DeployRunnerGcpCredentialEnvTests()
    {
        _workDir = Path.Combine(Path.GetTempPath(), $"deploy-gcp-env-test-{Guid.NewGuid():N}");
        Directory.CreateDirectory(_workDir);
        _probePath = Path.Combine(_workDir, "probe.txt");

        // The probe install.sh: dump the gcloud env + the modes/content of what
        // it points at, at the moment the installer would use them.
        var stub = Path.Combine(_workDir, "install-probe.sh");
        File.WriteAllText(stub,
            "#!/bin/sh\n" +
            "{\n" +
            "  echo \"OVERRIDE=${CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE:-}\"\n" +
            "  echo \"ADC=${GOOGLE_APPLICATION_CREDENTIALS:-}\"\n" +
            "  echo \"PROJECT=${CLOUDSDK_CORE_PROJECT:-}\"\n" +
            "  echo \"CONFIG=${CLOUDSDK_CONFIG:-}\"\n" +
            "  if [ -n \"${CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE:-}\" ]; then\n" +
            "    echo \"KEY_MODE=$(stat -c %a \"$CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE\")\"\n" +
            "    echo \"KEY_BODY=$(cat \"$CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE\")\"\n" +
            "  fi\n" +
            "  if [ -n \"${CLOUDSDK_CONFIG:-}\" ]; then\n" +
            "    echo \"DIR_MODE=$(stat -c %a \"$CLOUDSDK_CONFIG\")\"\n" +
            "  fi\n" +
            $"}} > '{_probePath}'\n" +
            "exit 0\n");
        _previousInstallSh = Environment.GetEnvironmentVariable("INSTALL_SH_PATH");
        Environment.SetEnvironmentVariable("INSTALL_SH_PATH", stub);
    }

    public void Dispose()
    {
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
    public async Task Gcp_deploy_hands_install_sh_the_accounts_key_via_the_gcloud_override_env_and_cleans_up()
    {
        if (!OperatingSystem.IsLinux())
        {
            return; // probe needs bash + GNU stat
        }

        var (sp, conn) = BuildHost(withCipher: true);
        using var _ = conn;
        Guid deploymentId;
        using (var db = Db(sp))
        {
            var accountId = SeedProjectAndAccount(db, sp.GetRequiredService<CredentialCipher>(), JsonKey);
            deploymentId = SeedDeployment(db, GcpDeployJson, accountId);
        }

        await Runner(sp).RunDeploymentAsync(deploymentId, GcpDeployJson, CancellationToken.None);

        var probe = ReadProbe();
        var keyFile = probe["OVERRIDE"];
        var configDir = probe["CONFIG"];
        Assert.False(string.IsNullOrEmpty(keyFile), "CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE must be set");
        Assert.False(string.IsNullOrEmpty(configDir), "CLOUDSDK_CONFIG must be set (isolated gcloud config)");
        Assert.Equal(keyFile, probe["ADC"]);
        Assert.Equal("proj-1", probe["PROJECT"]);
        Assert.StartsWith(configDir + "/", keyFile, StringComparison.Ordinal); // key lives inside the staging dir
        Assert.Equal("600", probe["KEY_MODE"]);
        Assert.Equal("700", probe["DIR_MODE"]);
        Assert.Equal(JsonKey, probe["KEY_BODY"]);

        // Staging dir + key are gone once install.sh has exited.
        Assert.False(File.Exists(keyFile));
        Assert.False(Directory.Exists(configDir));

        using var check = Db(sp);
        var row = check.Deployments.AsNoTracking().Single(d => d.DeploymentId == deploymentId);
        Assert.Equal("completed", row.Status);
        Assert.Contains("GCP credentials: deployer@proj-1.iam.gserviceaccount.com, project proj-1 handed to install.sh", row.Log);
    }

    [Fact]
    public async Task Wizard_deploy_without_an_account_id_uses_the_projects_single_active_gcp_account()
    {
        if (!OperatingSystem.IsLinux())
        {
            return;
        }

        var (sp, conn) = BuildHost(withCipher: true);
        using var _ = conn;
        Guid deploymentId;
        using (var db = Db(sp))
        {
            var cipher = sp.GetRequiredService<CredentialCipher>();
            SeedProjectAndAccount(db, cipher, JsonKey);
            // A disabled sibling must not make the choice ambiguous.
            SeedAccount(db, cipher, JsonKey, status: "disabled", name: "GCP old");
            deploymentId = SeedDeployment(db, GcpDeployJson, cloudAccountId: null);
        }

        await Runner(sp).RunDeploymentAsync(deploymentId, GcpDeployJson, CancellationToken.None);

        var probe = ReadProbe();
        Assert.False(string.IsNullOrEmpty(probe["OVERRIDE"]));
        Assert.Equal(JsonKey, probe["KEY_BODY"]);
    }

    [Fact]
    public async Task Wizard_deploy_with_several_active_gcp_accounts_does_not_guess_and_says_so()
    {
        if (!OperatingSystem.IsLinux())
        {
            return;
        }

        var (sp, conn) = BuildHost(withCipher: true);
        using var _ = conn;
        Guid deploymentId;
        using (var db = Db(sp))
        {
            var cipher = sp.GetRequiredService<CredentialCipher>();
            SeedProjectAndAccount(db, cipher, JsonKey);
            SeedAccount(db, cipher, JsonKey, status: "active", name: "GCP second");
            deploymentId = SeedDeployment(db, GcpDeployJson, cloudAccountId: null);
        }

        await Runner(sp).RunDeploymentAsync(deploymentId, GcpDeployJson, CancellationToken.None);

        var probe = ReadProbe();
        Assert.Equal("", probe["OVERRIDE"]);
        Assert.Equal("", probe["CONFIG"]);
        using var check = Db(sp);
        var log = check.Deployments.AsNoTracking().Single(d => d.DeploymentId == deploymentId).Log;
        Assert.Contains("more than one active GCP cloud account", log);
        Assert.Contains("ambient gcloud auth", log);
    }

    [Fact]
    public async Task Gcp_account_without_a_json_key_leaves_the_env_clean_and_notes_it()
    {
        if (!OperatingSystem.IsLinux())
        {
            return;
        }

        var (sp, conn) = BuildHost(withCipher: true);
        using var _ = conn;
        Guid deploymentId;
        using (var db = Db(sp))
        {
            var accountId = SeedProjectAndAccount(db, sp.GetRequiredService<CredentialCipher>(), jsonKey: null, credentialJson: "{}");
            deploymentId = SeedDeployment(db, GcpDeployJson, accountId);
        }

        await Runner(sp).RunDeploymentAsync(deploymentId, GcpDeployJson, CancellationToken.None);

        var probe = ReadProbe();
        Assert.Equal("", probe["OVERRIDE"]);
        Assert.Equal("", probe["PROJECT"]);
        using var check = Db(sp);
        var log = check.Deployments.AsNoTracking().Single(d => d.DeploymentId == deploymentId).Log;
        Assert.Contains("has no json_key", log);
    }

    [Fact]
    public async Task Azure_deploy_gets_no_gcloud_env_and_no_note()
    {
        if (!OperatingSystem.IsLinux())
        {
            return;
        }

        var (sp, conn) = BuildHost(withCipher: true);
        using var _ = conn;
        Guid deploymentId;
        using (var db = Db(sp))
        {
            var accountId = SeedProjectAndAccount(db, sp.GetRequiredService<CredentialCipher>(), JsonKey);
            deploymentId = SeedDeployment(db, AzureDeployJson, accountId);
        }

        await Runner(sp).RunDeploymentAsync(deploymentId, AzureDeployJson, CancellationToken.None);

        var probe = ReadProbe();
        Assert.Equal("", probe["OVERRIDE"]);
        Assert.Equal("", probe["CONFIG"]);
        using var check = Db(sp);
        var log = check.Deployments.AsNoTracking().Single(d => d.DeploymentId == deploymentId).Log;
        Assert.DoesNotContain("GCP credentials", log);
    }

    [Fact]
    public async Task Bare_host_without_a_cipher_still_runs_the_deploy_on_ambient_auth()
    {
        if (!OperatingSystem.IsLinux())
        {
            return;
        }

        var (sp, conn) = BuildHost(withCipher: false);
        using var _ = conn;
        Guid deploymentId;
        using (var db = Db(sp))
        {
            var accountId = SeedProjectAndAccount(db, cipher: null, JsonKey);
            deploymentId = SeedDeployment(db, GcpDeployJson, accountId);
        }

        await Runner(sp).RunDeploymentAsync(deploymentId, GcpDeployJson, CancellationToken.None);

        var probe = ReadProbe();
        Assert.Equal("", probe["OVERRIDE"]);
        using var check = Db(sp);
        var row = check.Deployments.AsNoTracking().Single(d => d.DeploymentId == deploymentId);
        Assert.Equal("completed", row.Status);
        Assert.Contains("no credential cipher", row.Log);
    }

    // ── Pure shape checks (no process spawn) ──────────────────────────────────

    [Theory]
    [InlineData(GcpDeployJson, true)]
    [InlineData(AzureDeployJson, false)]
    [InlineData("""{"endpoints":[{"provider":"azure"},{"provider":"GCP"}]}""", true)]
    [InlineData("""{"tester":{"provider":"gcp"}}""", false)] // tester-only: install.sh's tester path is not the control plane's
    [InlineData("not json", false)]
    public void DeployNeedsGcp_keys_on_endpoint_providers_only(string json, bool expected)
        => Assert.Equal(expected, GcpInstallerCredentials.DeployNeedsGcp(json));

    // ── Harness (mirrors DeployJsonGcpZoneTests) ──────────────────────────────

    private static (ServiceProvider Sp, SqliteConnection Conn) BuildHost(bool withCipher)
    {
        // Named shared-cache in-memory DB: the runner opens its own scopes on
        // its own connections; the keeper connection holds the database alive.
        var connString = $"DataSource=deploy-gcp-env-{Guid.NewGuid():N};Mode=Memory;Cache=Shared";
        var conn = new SqliteConnection(connString);
        conn.Open();

        var services = new ServiceCollection();
        services.AddLogging(b => b.SetMinimumLevel(LogLevel.Warning));
        services.AddSignalR();
        services.AddAgentProtocol();
        services.AddDashboardEventBus();
        services.AddDbContext<NetworkerDbContext>(o => o.UseSqlite(connString));
        if (withCipher)
        {
            services.AddSingleton(new CredentialCipher(TestKey));
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

    private static DeployRunner Runner(IServiceProvider sp) => new(
        sp.GetRequiredService<IServiceScopeFactory>(),
        sp.GetRequiredService<EventBus>(),
        sp.GetRequiredService<ILogger<DeployRunner>>());

    private static NetworkerDbContext Db(IServiceProvider sp) =>
        sp.CreateScope().ServiceProvider.GetRequiredService<NetworkerDbContext>();

    private static Guid SeedProjectAndAccount(
        NetworkerDbContext db, CredentialCipher? cipher, string? jsonKey, string? credentialJson = null)
    {
        var now = DateTime.UtcNow;
        db.Projects.Add(new Project
        {
            ProjectId = ProjectId,
            Name = "gcp-env",
            Slug = "gcp-env",
            Settings = "{}",
            CreatedAt = now,
            UpdatedAt = now,
        });
        db.SaveChanges();
        return SeedAccount(db, cipher, jsonKey, status: "active", name: "GCP prod", credentialJson);
    }

    private static Guid SeedAccount(
        NetworkerDbContext db, CredentialCipher? cipher, string? jsonKey, string status, string name,
        string? credentialJson = null)
    {
        var now = DateTime.UtcNow;
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
            Name = name,
            Provider = "gcp",
            Status = status,
            CredentialsEnc = enc,
            CredentialsNonce = nonce,
            CreatedAt = now,
            UpdatedAt = now,
            ProjectId = ProjectId,
        });
        db.SaveChanges();
        return accountId;
    }

    private static Guid SeedDeployment(NetworkerDbContext db, string config, Guid? cloudAccountId)
    {
        var deploymentId = Guid.NewGuid();
        var now = DateTime.UtcNow;
        db.Deployments.Add(new Deployment
        {
            DeploymentId = deploymentId,
            Name = $"auto-cell-{deploymentId:N}"[..20],
            Status = "pending",
            Config = config,
            CreatedAt = now,
            ProjectId = ProjectId,
            CloudAccountId = cloudAccountId,
        });
        db.SaveChanges();
        return deploymentId;
    }

    private Dictionary<string, string> ReadProbe()
    {
        Assert.True(File.Exists(_probePath), "install.sh probe never ran");
        var map = new Dictionary<string, string>(StringComparer.Ordinal)
        {
            ["OVERRIDE"] = "", ["ADC"] = "", ["PROJECT"] = "", ["CONFIG"] = "",
        };
        foreach (var line in File.ReadAllLines(_probePath))
        {
            var eq = line.IndexOf('=');
            if (eq > 0)
            {
                map[line[..eq]] = line[(eq + 1)..];
            }
        }
        return map;
    }
}
