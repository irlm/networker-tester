using Networker.ControlPlane.Provisioning;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// GCP endpoint-VM teardown (#838): comparison-cell endpoint VMs are created by
/// install.sh, so the deployment row never holds a resource id — the teardown
/// reverse-looks the instance up by the public IP install.sh reported, then
/// deletes it through the gcp lifecycle op. Before this, the reverse-lookup was
/// Azure-only ("not implemented for cloud 'gcp'") and every GCP cell leaked its
/// VM.
///
/// <para>Behavioural: <c>GCLOUD_CMD</c> points at a fake POSIX-sh gcloud that
/// records argv + the credential env per call and answers
/// <c>compute instances list</c> with a canned document. Shares the
/// <c>cloud-cli-fake-bins</c> collection so *_CMD env mutation cannot race the
/// sibling fake-CLI tests. Windows returns early (no <c>/bin/sh</c>).</para>
/// </summary>
[Collection("cloud-cli-fake-bins")]
public sealed class CliProvisionerGcpTeardownTests : IDisposable
{
    private const string JsonKey =
        /*lang=json*/ """{"type":"service_account","project_id":"kepler-1","client_email":"vms@kepler-1.iam.gserviceaccount.com","private_key":"fake"}""";

    /// <summary>Two instances; the second owns the endpoint we search for. The
    /// first's NAT IP is a prefix-trap for the exact match.</summary>
    private const string ListJson = """
        [
          { "name": "other-vm", "selfLink": "https://www.googleapis.com/compute/v1/projects/kepler-1/zones/us-east1-b/instances/other-vm",
            "networkInterfaces": [ { "accessConfigs": [ { "natIP": "34.148.103.230" } ] } ] },
          { "name": "nwk-a-11f093f9", "selfLink": "https://www.googleapis.com/compute/v1/projects/kepler-1/zones/us-east1-b/instances/nwk-a-11f093f9",
            "networkInterfaces": [ { "accessConfigs": [ { "natIP": "34.148.103.234" } ] } ] }
        ]
        """;

    private readonly string _workDir;
    private readonly string _callLogPath;

    public CliProvisionerGcpTeardownTests()
    {
        _workDir = Path.Combine(Path.GetTempPath(), $"gcp-teardown-test-{Guid.NewGuid():N}");
        Directory.CreateDirectory(_workDir);
        _callLogPath = Path.Combine(_workDir, "calls.log");
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

    // ── Pure matcher ──────────────────────────────────────────────────────────

    [Fact]
    public void Matches_instance_by_exact_nat_ip_and_returns_its_selfLink()
    {
        var vm = CliComputeProvisioner.MatchGcpInstanceByEndpoint(ListJson, "34.148.103.234");

        Assert.NotNull(vm);
        Assert.Equal("nwk-a-11f093f9", vm!.Name);
        Assert.EndsWith("/zones/us-east1-b/instances/nwk-a-11f093f9", vm.ResourceId);
        // The selfLink is what the gcp lifecycle op parses zone + name from.
        Assert.Equal(("nwk-a-11f093f9", "us-east1-b"), CliComputeProvisioner.ParseGcpResourceId(vm.ResourceId));
    }

    [Theory]
    [InlineData("34.148.103.23")]   // prefix of a real IP — must not match
    [InlineData("203.0.113.9")]     // nobody owns it
    [InlineData("")]
    [InlineData("   ")]
    public void Returns_null_when_no_instance_owns_the_endpoint(string endpoint)
        => Assert.Null(CliComputeProvisioner.MatchGcpInstanceByEndpoint(ListJson, endpoint));

    [Theory]
    [InlineData("not json")]
    [InlineData("{}")]
    [InlineData("[]")]
    public void Returns_null_for_unexpected_documents(string json)
        => Assert.Null(CliComputeProvisioner.MatchGcpInstanceByEndpoint(json, "34.148.103.234"));

    [Fact]
    public void Filter_selects_the_first_nic_nat_ip()
        => Assert.Equal("networkInterfaces[0].accessConfigs[0].natIP=34.148.103.234",
            CliComputeProvisioner.GcpNatIpFilter("34.148.103.234"));

    // ── Behavioural: list → resolve → delete through the fake gcloud ─────────

    [Fact]
    public async Task Resolve_lists_instances_by_nat_ip_with_the_account_key_and_delete_uses_the_resolved_zone()
    {
        if (OperatingSystem.IsWindows())
        {
            return; // fake CLI needs /bin/sh
        }

        WriteFakeGcloud(listStdout: ListJson);
        var provisioner = new CliComputeProvisioner(
            Microsoft.Extensions.Logging.Abstractions.NullLogger<CliComputeProvisioner>.Instance);
        var creds = new ProviderCredentials("gcp", Region: "us-east1",
            Extra: new Dictionary<string, string> { ["json_key"] = JsonKey });

        var vm = await provisioner.ResolveByEndpointAsync("gcp", creds, "34.148.103.234", CancellationToken.None);

        Assert.NotNull(vm);
        Assert.Equal("nwk-a-11f093f9", vm!.Name);

        var calls = ReadCalls();
        var list = Assert.Single(calls, c => c.Argv.StartsWith("compute instances list", StringComparison.Ordinal));
        Assert.Contains("--filter networkInterfaces[0].accessConfigs[0].natIP=34.148.103.234", list.Argv, StringComparison.Ordinal);
        Assert.Contains("--format json", list.Argv, StringComparison.Ordinal);
        // Authenticated the #827/#828 way: override var → a key file holding the account key, project from the key.
        Assert.False(string.IsNullOrEmpty(list.Override), "CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE must be set");
        Assert.Equal(JsonKey, list.KeyContent);
        Assert.Equal("kepler-1", list.Project);
        Assert.False(File.Exists(list.Override)); // 0600 tempfile deleted after the call

        // Same shape the teardown builds: the synthetic tester carries the selfLink.
        var synthetic = new ProjectTester { Cloud = "gcp", Region = "us-east1", VmResourceId = vm.ResourceId, VmName = vm.Name };
        var res = await provisioner.DeleteAsync(synthetic, creds, CancellationToken.None);
        Assert.True(res.Success, res.Error ?? res.StdErr);

        var del = Assert.Single(ReadCalls(), c => c.Argv.StartsWith("compute instances delete", StringComparison.Ordinal));
        Assert.Equal("compute instances delete nwk-a-11f093f9 --zone us-east1-b --quiet", del.Argv);
        Assert.Equal(JsonKey, del.KeyContent);
    }

    [Fact]
    public async Task Resolve_without_a_key_still_lists_on_ambient_auth_and_a_failed_listing_yields_null()
    {
        if (OperatingSystem.IsWindows())
        {
            return;
        }

        WriteFakeGcloud(listStdout: ListJson);
        var provisioner = new CliComputeProvisioner(
            Microsoft.Extensions.Logging.Abstractions.NullLogger<CliComputeProvisioner>.Instance);

        var vm = await provisioner.ResolveByEndpointAsync(
            "gcp", new ProviderCredentials("gcp", Region: "us-east1"), "34.148.103.234", CancellationToken.None);
        Assert.NotNull(vm);
        Assert.Equal("", Assert.Single(ReadCalls()).Override); // no key → no override (ambient auth)

        WriteFakeGcloud(listStdout: "", exitCode: 1);
        Assert.Null(await provisioner.ResolveByEndpointAsync(
            "gcp", new ProviderCredentials("gcp"), "34.148.103.234", CancellationToken.None));
    }

    // ── Harness ───────────────────────────────────────────────────────────────

    private sealed record Call(string Argv, string Override, string Project, string KeyContent);

    /// <summary>Fake gcloud via <c>GCLOUD_CMD</c>: appends one record per call
    /// (argv + credential env + the key file's content), answers
    /// <c>compute instances list</c> with <paramref name="listStdout"/>.</summary>
    private void WriteFakeGcloud(string listStdout, int exitCode = 0)
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
            "case \"$*\" in\n" +
            $"  \"compute instances list\"*) printf '%s' '{listStdout}'; exit {exitCode} ;;\n" +
            "  \"compute instances delete\"*) exit 0 ;;\n" +
            "  *) printf '%s\\n' 'unexpected gcloud invocation' >&2; exit 1 ;;\n" +
            "esac\n");
        File.SetUnixFileMode(
            path,
            UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute);

        Environment.SetEnvironmentVariable("GCLOUD_CMD", path);
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
