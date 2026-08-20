using Microsoft.Extensions.Logging.Abstractions;
using Networker.ControlPlane.Provisioning;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pins GCP zone resolution for the create path (#829): not every GCP region
/// has an "<c>-a</c>" zone (us-east1 and europe-west1 are <c>-b/-c/-d</c>), so
/// the create must resolve the zone from <c>gcloud compute zones list</c> —
/// first <c>status: UP</c> zone by name — instead of assuming
/// <c>{region}-a</c>. The resolution is cached per (project, region) for the
/// provisioner's lifetime (a DI singleton → process lifetime), and a failed
/// listing falls back to <c>{region}-a</c> with the listing failure folded
/// into any downstream create error.
///
/// <para>Behavioural: <c>GCLOUD_CMD</c> points at a fake POSIX-sh gcloud that
/// logs every argv and answers <c>zones list</c> and <c>instances create</c>
/// differently, so the assertions cover the real spawn path. Windows returns
/// early (no <c>/bin/sh</c>), like the sibling gcloud-env tests. Shares the
/// <c>cloud-cli-fake-bins</c> collection so *_CMD env mutation cannot race.</para>
/// </summary>
[Collection("cloud-cli-fake-bins")]
public sealed class CliProvisionerGcpZoneTests : IDisposable
{
    private const string JsonKey =
        /*lang=json*/ """{"type":"service_account","project_id":"proj-1","private_key":"fake"}""";

    /// <summary>us-east1's real shape has no -a zone. -b is DOWN here so the
    /// status filter is exercised; -c is the first UP zone by ordinal name; the
    /// us-central1 decoy proves zones of other regions never qualify even when
    /// the (substring-matching) gcloud filter would let one through.</summary>
    private const string ZonesJson =
        /*lang=json*/ """[{"name":"us-east1-d","status":"UP"},{"name":"us-east1-b","status":"DOWN"},{"name":"us-central1-a","status":"UP"},{"name":"us-east1-c","status":"UP"}]""";

    private const string InstanceJson =
        """[{"selfLink":"https://www.googleapis.com/compute/v1/projects/proj-1/zones/us-east1-c/instances/tester-useast1-abc12","networkInterfaces":[{"accessConfigs":[{"natIP":"203.0.113.5"}]}]}]""";

    private readonly string _workDir;
    private readonly string _callLogPath;

    public CliProvisionerGcpZoneTests()
    {
        _workDir = Path.Combine(Path.GetTempPath(), $"gcloud-zone-test-{Guid.NewGuid():N}");
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

    [Fact]
    public async Task GcpCreate_resolves_zone_from_first_up_zone_in_listing()
    {
        if (OperatingSystem.IsWindows())
        {
            return; // fake CLI needs /bin/sh
        }

        WriteFakeGcloud(zonesBody: $"printf '%s' '{ZonesJson}'\nexit 0");

        var result = await NewProvisioner()
            .CreateVmAsync(Request(), GcpCreds(), CancellationToken.None);

        Assert.True(result.Success, result.Error);
        var calls = File.ReadAllLines(_callLogPath);
        Assert.Equal(2, calls.Length);
        Assert.Equal("compute zones list --filter=region:(us-east1) --format=json", calls[0]);
        // -b is DOWN, us-central1-a is another region's zone: first UP zone of
        // us-east1 by name is -c, and the create argv carries it.
        Assert.StartsWith("compute instances create ", calls[1], StringComparison.Ordinal);
        Assert.Contains(" --zone us-east1-c ", $" {calls[1]} ", StringComparison.Ordinal);
        Assert.DoesNotContain("us-east1-a", calls[1], StringComparison.Ordinal);
    }

    [Fact]
    public async Task GcpCreate_caches_zone_per_project_and_region()
    {
        if (OperatingSystem.IsWindows())
        {
            return; // fake CLI needs /bin/sh
        }

        WriteFakeGcloud(zonesBody: $"printf '%s' '{ZonesJson}'\nexit 0");
        var provisioner = NewProvisioner();

        var first = await provisioner.CreateVmAsync(Request(), GcpCreds(), CancellationToken.None);
        var second = await provisioner.CreateVmAsync(Request(), GcpCreds(), CancellationToken.None);

        Assert.True(first.Success, first.Error);
        Assert.True(second.Success, second.Error);

        var calls = File.ReadAllLines(_callLogPath);
        // One listing for the pair — the second create is a cache hit — and
        // both creates carry the resolved zone.
        Assert.Equal(1, calls.Count(c => c.StartsWith("compute zones list", StringComparison.Ordinal)));
        Assert.Equal(2, calls.Count(c =>
            c.StartsWith("compute instances create", StringComparison.Ordinal)
            && c.Contains("--zone us-east1-c", StringComparison.Ordinal)));
    }

    [Fact]
    public async Task GcpCreate_falls_back_to_dash_a_when_listing_fails()
    {
        if (OperatingSystem.IsWindows())
        {
            return; // fake CLI needs /bin/sh
        }

        WriteFakeGcloud(zonesBody: "printf '%s\\n' 'boom-listing' >&2\nexit 1");

        var result = await NewProvisioner()
            .CreateVmAsync(Request(), GcpCreds(), CancellationToken.None);

        // The listing failed but the create itself succeeded against the
        // fallback zone — the old behaviour is the floor, never worse.
        Assert.True(result.Success, result.Error);
        var calls = File.ReadAllLines(_callLogPath);
        Assert.Contains(" --zone us-east1-a ", $" {calls[^1]} ", StringComparison.Ordinal);
    }

    [Fact]
    public async Task GcpCreate_failure_after_listing_failure_reports_both()
    {
        if (OperatingSystem.IsWindows())
        {
            return; // fake CLI needs /bin/sh
        }

        WriteFakeGcloud(
            zonesBody: "printf '%s\\n' 'boom-listing' >&2\nexit 1",
            createBody: "printf '%s\\n' 'boom-create' >&2\nexit 1");

        var result = await NewProvisioner()
            .CreateVmAsync(Request(), GcpCreds(), CancellationToken.None);

        Assert.False(result.Success);
        Assert.NotNull(result.Error);
        // The create error names the actual CLI failure, the fallback zone it
        // ran against, AND why the zone was a guess — no post-mortem guessing.
        Assert.Contains("boom-create", result.Error, StringComparison.Ordinal);
        Assert.Contains("zone fell back to us-east1-a", result.Error, StringComparison.Ordinal);
        Assert.Contains("boom-listing", result.Error, StringComparison.Ordinal);
    }

    // ── Harness ───────────────────────────────────────────────────────────────

    private static CliComputeProvisioner NewProvisioner() =>
        new(NullLogger<CliComputeProvisioner>.Instance);

    private static VmCreateRequest Request() =>
        new(
            Cloud: "gcp",
            Name: "tester-useast1-abc12",
            Region: "us-east1",
            VmSize: "e2-standard-2",
            SshUser: "ubuntu",
            Image: "ubuntu-2404-lts-amd64",
            BootstrapScript: null);

    private static ProviderCredentials GcpCreds() =>
        new("gcp", null, null, "us-east1",
            new Dictionary<string, string> { ["json_key"] = JsonKey });

    /// <summary>
    /// Fake gcloud (registered via <c>GCLOUD_CMD</c>): appends each argv to the
    /// call log, then runs <paramref name="zonesBody"/> for
    /// <c>compute zones list</c> invocations and <paramref name="createBody"/>
    /// for everything else (default: succeed with the created-instance JSON).
    /// </summary>
    private void WriteFakeGcloud(string zonesBody, string? createBody = null)
    {
        createBody ??= $"printf '%s' '{InstanceJson}'\nexit 0";
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
            $"{createBody}\n" +
            "  ;;\n" +
            "esac\n");
        File.SetUnixFileMode(
            path,
            UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute);

        Environment.SetEnvironmentVariable("GCLOUD_CMD", path);
    }
}
