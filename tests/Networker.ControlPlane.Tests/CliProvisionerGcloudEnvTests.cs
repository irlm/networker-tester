using Microsoft.Extensions.Logging.Abstractions;
using Networker.ControlPlane.Provisioning;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pins the credential-env contract for every gcloud invocation (#827): the
/// gcloud CLI does NOT read <c>GOOGLE_APPLICATION_CREDENTIALS</c> (that is the
/// Application Default Credentials variable for Google client libraries), so a
/// gcloud call that carries only the ADC var — or no credential env at all —
/// fails "You do not currently have an active account selected" on any host
/// whose gcloud was never interactively authenticated (i.e. prod). Every GCP
/// operation must therefore pass <c>CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE</c>
/// (the stateless per-invocation override gcloud honours) pointing at a
/// tempfile copy of the service-account key, plus <c>CLOUDSDK_CORE_PROJECT</c>.
///
/// <para>Behavioural tests point <c>GCLOUD_CMD</c> at a fake POSIX-sh gcloud
/// that records its argv AND the credential env it received (including the key
/// file's content, proving the file existed during the call), so the assertion
/// covers the real <c>RunAsync</c> spawn path, not just a builder. On Windows
/// the behavioural tests return early (no <c>/bin/sh</c>); the pure
/// <see cref="CliComputeProvisioner.BuildGcloudEnv"/> tests still run.</para>
/// </summary>
[Collection("cloud-cli-fake-bins")]
public sealed class CliProvisionerGcloudEnvTests : IDisposable
{
    private const string JsonKey =
        /*lang=json*/ """{"type":"service_account","project_id":"proj-1","private_key":"fake"}""";

    private readonly string _workDir;
    private readonly string _envLogPath;

    public CliProvisionerGcloudEnvTests()
    {
        _workDir = Path.Combine(Path.GetTempPath(), $"gcloud-env-test-{Guid.NewGuid():N}");
        Directory.CreateDirectory(_workDir);
        _envLogPath = Path.Combine(_workDir, "env.log");
    }

    public void Dispose()
    {
        Environment.SetEnvironmentVariable("GCLOUD_CMD", null);
        try
        {
            Directory.Delete(_workDir, recursive: true);
        }
        catch
        {
            // best-effort temp cleanup
        }
    }

    // ── Pure env-builder contract ─────────────────────────────────────────────

    [Fact]
    public void BuildGcloudEnv_sets_cli_override_adc_and_project()
    {
        var env = CliComputeProvisioner.BuildGcloudEnv("/tmp/key.json", "proj-1");

        // The var gcloud actually reads — the #827 fix.
        Assert.Equal("/tmp/key.json", env["CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE"]);
        // Kept for ADC consumers (client libraries a gcloud component may spawn).
        Assert.Equal("/tmp/key.json", env["GOOGLE_APPLICATION_CREDENTIALS"]);
        Assert.Equal("proj-1", env["CLOUDSDK_CORE_PROJECT"]);
        Assert.Equal(3, env.Count);
    }

    [Fact]
    public void BuildGcloudEnv_omits_project_when_unknown()
    {
        var env = CliComputeProvisioner.BuildGcloudEnv("/tmp/key.json", null);

        Assert.Equal("/tmp/key.json", env["CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE"]);
        Assert.False(env.ContainsKey("CLOUDSDK_CORE_PROJECT"));
    }

    [Theory]
    [InlineData("""{"type":"service_account","project_id":"proj-1"}""", "proj-1")]
    [InlineData("""{"type":"service_account"}""", null)]
    [InlineData("not json", null)]
    [InlineData("""{"project_id":42}""", null)]
    public void ParseGcpProjectId_extracts_or_returns_null(string jsonKey, string? expected)
    {
        Assert.Equal(expected, CliComputeProvisioner.ParseGcpProjectId(jsonKey));
    }

    // ── Behavioural: every GCP lifecycle op authenticates the spawned gcloud ──

    [Theory]
    [InlineData("start")]
    [InlineData("stop")]
    [InlineData("deallocate")]
    [InlineData("delete")]
    [InlineData("show")]
    public async Task GcpLifecycleOps_pass_credential_env_to_gcloud(string op)
    {
        if (OperatingSystem.IsWindows())
        {
            return; // fake CLI needs /bin/sh
        }

        WriteFakeGcloud();
        var p = NewProvisioner();
        var tester = GcpTester();
        var creds = GcpCreds();

        var result = op switch
        {
            "start" => await p.StartAsync(tester, creds, CancellationToken.None),
            "stop" => await p.StopAsync(tester, creds, CancellationToken.None),
            "deallocate" => await p.DeallocateAsync(tester, creds, CancellationToken.None),
            "delete" => await p.DeleteAsync(tester, creds, CancellationToken.None),
            _ => await p.ShowAsync(tester, creds, CancellationToken.None),
        };

        Assert.True(result.Success);
        var env = ReadEnvLog();

        // gcloud saw the per-invocation credential override + project, and the
        // 0600 key tempfile really held the service-account key during the call.
        Assert.NotEqual("", env["override"]);
        Assert.Equal(env["override"], env["adc"]);
        Assert.Equal("proj-1", env["project"]);
        Assert.Equal(JsonKey, env["keycontent"]);

        // Hygiene: the key tempfile is deleted once the invocation returns.
        Assert.False(File.Exists(env["override"]));
    }

    [Fact]
    public async Task GcpCreate_passes_credential_env_to_gcloud()
    {
        if (OperatingSystem.IsWindows())
        {
            return; // fake CLI needs /bin/sh
        }

        // The create path parses the fake's stdout as the created-instance JSON.
        WriteFakeGcloud(stdout:
            """[{"selfLink":"https://www.googleapis.com/compute/v1/projects/proj-1/zones/us-east1-a/instances/tester-useast1-abc12","networkInterfaces":[{"accessConfigs":[{"natIP":"203.0.113.5"}]}]}]""");

        var request = new VmCreateRequest(
            Cloud: "gcp",
            Name: "tester-useast1-abc12",
            Region: "us-east1",
            VmSize: "e2-standard-2",
            SshUser: "ubuntu",
            Image: "ubuntu-2404-lts-amd64",
            BootstrapScript: null);

        var result = await NewProvisioner().CreateVmAsync(request, GcpCreds(), CancellationToken.None);

        Assert.True(result.Success, result.Error);
        var env = ReadEnvLog();

        Assert.NotEqual("", env["override"]);
        Assert.Equal(env["override"], env["adc"]);
        Assert.Equal("proj-1", env["project"]);
        Assert.Equal(JsonKey, env["keycontent"]);
        Assert.False(File.Exists(env["override"]));
    }

    [Fact]
    public async Task GcpLifecycle_without_json_key_keeps_ambient_auth()
    {
        if (OperatingSystem.IsWindows())
        {
            return; // fake CLI needs /bin/sh
        }

        WriteFakeGcloud();

        // No Extra/json_key (e.g. unreadable connection config) → the previous
        // ambient-host-auth behaviour is preserved: no credential env injected.
        var result = await NewProvisioner().StartAsync(
            GcpTester(), new ProviderCredentials("gcp"), CancellationToken.None);

        Assert.True(result.Success);
        var env = ReadEnvLog();
        Assert.Equal("", env["override"]);
        Assert.Equal("", env["adc"]);
        Assert.Equal("", env["project"]);
    }

    // ── Harness ───────────────────────────────────────────────────────────────

    private static CliComputeProvisioner NewProvisioner() =>
        new(NullLogger<CliComputeProvisioner>.Instance);

    private static ProjectTester GcpTester() =>
        new()
        {
            TesterId = Guid.NewGuid(),
            ProjectId = "p",
            Name = "t",
            Cloud = "gcp",
            Region = "us-east1",
            VmSize = "e2-standard-2",
            SshUser = "ubuntu",
            PowerState = "running",
            Allocation = "on-demand",
            VmName = "tester-useast1-abc12",
            VmResourceId =
                "https://www.googleapis.com/compute/v1/projects/proj-1/zones/us-east1-a/instances/tester-useast1-abc12",
        };

    private static ProviderCredentials GcpCreds() =>
        new("gcp", null, null, "us-east1",
            new Dictionary<string, string> { ["json_key"] = JsonKey });

    /// <summary>
    /// Fake gcloud (registered via <c>GCLOUD_CMD</c>): records argv, the three
    /// credential env vars, and the CONTENT of the key file the override points
    /// at (empty when unset/absent — proves the 0600 tempfile existed while the
    /// CLI ran), then prints <paramref name="stdout"/> and exits 0.
    /// </summary>
    private void WriteFakeGcloud(string stdout = "")
    {
        var path = Path.Combine(_workDir, "fake-gcloud.sh");
        File.WriteAllText(
            path,
            "#!/bin/sh\n" +
            $"{{\n" +
            $"  printf 'argv=%s\\n' \"$*\"\n" +
            $"  printf 'override=%s\\n' \"$CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE\"\n" +
            $"  printf 'adc=%s\\n' \"$GOOGLE_APPLICATION_CREDENTIALS\"\n" +
            $"  printf 'project=%s\\n' \"$CLOUDSDK_CORE_PROJECT\"\n" +
            $"  printf 'keycontent=%s\\n' \"$(cat \"$CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE\" 2>/dev/null)\"\n" +
            $"}} >> '{_envLogPath}'\n" +
            (stdout.Length > 0 ? $"printf '%s' '{stdout}'\n" : "") +
            "exit 0\n");
        // The fake CLI is a /bin/sh script, so every caller already skips on Windows;
        // guarding here too keeps the helper safe to call and satisfies CA1416.
        if (!OperatingSystem.IsWindows())
        {
            File.SetUnixFileMode(
                path,
                UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute);
        }

        Environment.SetEnvironmentVariable("GCLOUD_CMD", path);
    }

    private Dictionary<string, string> ReadEnvLog()
    {
        var env = new Dictionary<string, string>(StringComparer.Ordinal);
        foreach (var line in File.ReadAllLines(_envLogPath))
        {
            var idx = line.IndexOf('=', StringComparison.Ordinal);
            if (idx > 0)
            {
                env[line[..idx]] = line[(idx + 1)..];
            }
        }
        return env;
    }
}
