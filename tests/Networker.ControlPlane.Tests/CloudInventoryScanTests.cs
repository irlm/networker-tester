using System.Diagnostics;
using Microsoft.Extensions.Logging.Abstractions;
using Networker.ControlPlane.Provisioning;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// The real cloud inventory scan (<see cref="CloudInventoryScanner"/>) behind
/// the Settings page's "scan all providers" button, which used to be a stub that
/// always answered <c>{vms: [], errors: []}</c> — indistinguishable from a dead
/// button.
///
/// <para>Two layers: pure projections of realistic <c>az</c>/<c>aws</c>/
/// <c>gcloud</c> JSON (no process spawn), and behavioural tests that point
/// <c>AZ_CMD</c> / <c>AWS_CMD</c> / <c>GCLOUD_CMD</c> at fake POSIX-sh CLIs so
/// the real spawn path, credential env, per-call timeout, account budget and
/// parallel fan-out are exercised end to end. Shares the
/// <c>cloud-cli-fake-bins</c> collection so the *_CMD env mutation cannot race
/// the sibling fake-CLI classes. Windows returns early (no <c>/bin/sh</c>).</para>
/// </summary>
[Collection("cloud-cli-fake-bins")]
public sealed class CloudInventoryScanTests : IDisposable
{
    // ── Fixtures: what the CLIs actually return for our projections ──────────

    /// <summary>`az vm list --show-details --query "<AzureVmQuery>"`: an endpoint
    /// VM with a public IP, and a deallocated tester with neither IP nor FQDN.</summary>
    private const string AzureListJson = """
        [
          {"name":"nwk-ep-ubuntu-edne","rg":"networker-rg-endpoint","location":"eastus",
           "powerState":"VM running","publicIps":"20.55.12.9","fqdns":"","size":"Standard_B2s","os":"Linux"},
          {"name":"tester-ci-01","rg":"networker-testers","location":"westeurope",
           "powerState":"VM deallocated","publicIps":"","fqdns":"","size":"Standard_B1s","os":"Linux"}
        ]
        """;

    /// <summary>`aws ec2 describe-instances --query "<AwsInstanceQuery>"`: a
    /// tagged Linux instance and an UNTAGGED terminated Windows one (name comes
    /// back null — the row must still render, keyed by instance id).</summary>
    private const string AwsListJson = """
        [
          {"name":"nwk-ep-aws-1","id":"i-0abc123","state":"running","ip":"54.12.9.3",
           "dns":"ec2-54-12-9-3.compute-1.amazonaws.com","type":"t3.small","az":"us-east-1b","platform":null},
          {"name":null,"id":"i-0def456","state":"terminated","ip":null,"dns":"",
           "type":"t3.micro","az":"us-east-1a","platform":"windows"}
        ]
        """;

    /// <summary>`gcloud compute instances list --format "<GcpInstanceFormat>"`:
    /// zone/machineType are selfLink URLs; the second instance has no external IP.</summary>
    private const string GcpListJson = """
        [
          {"name":"nwk-a-11f093f9",
           "zone":"https://www.googleapis.com/compute/v1/projects/kepler-1/zones/us-east1-b",
           "status":"RUNNING",
           "machineType":"https://www.googleapis.com/compute/v1/projects/kepler-1/zones/us-east1-b/machineTypes/e2-small",
           "networkInterfaces":[{"accessConfigs":[{"natIP":"34.148.103.234"}]}]},
          {"name":"private-only",
           "zone":"https://www.googleapis.com/compute/v1/projects/kepler-1/zones/us-east1-b",
           "status":"TERMINATED",
           "machineType":"https://www.googleapis.com/compute/v1/projects/kepler-1/zones/us-east1-b/machineTypes/e2-micro",
           "networkInterfaces":[{}]}
        ]
        """;

    private const string GcpJsonKey =
        /*lang=json*/ """{"type":"service_account","project_id":"kepler-1","client_email":"vms@kepler-1.iam.gserviceaccount.com","private_key":"fake-private-key-value"}""";

    private readonly string _workDir;
    private readonly string _callLogDir;

    public CloudInventoryScanTests()
    {
        _workDir = Path.Combine(Path.GetTempPath(), $"inventory-scan-test-{Guid.NewGuid():N}");
        _callLogDir = Path.Combine(_workDir, "calls");
        Directory.CreateDirectory(_callLogDir);
    }

    public void Dispose()
    {
        Environment.SetEnvironmentVariable("AZ_CMD", null);
        Environment.SetEnvironmentVariable("AWS_CMD", null);
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

    // ── Pure projections ─────────────────────────────────────────────────────

    [Fact]
    public void Azure_projection_maps_every_wire_field_and_normalises_power_state()
    {
        var vms = CloudInventoryScanner.ParseAzureVms(AzureListJson, ["20.55.12.9"]);

        Assert.NotNull(vms);
        Assert.Equal(2, vms!.Count);

        var ep = vms[0];
        Assert.Equal("azure", ep.Provider);
        Assert.Equal("nwk-ep-ubuntu-edne", ep.Name);
        Assert.Equal("eastus", ep.Region);
        Assert.Equal("running", ep.Status);            // "VM running" → "running"
        Assert.Equal("20.55.12.9", ep.PublicIp);
        Assert.Null(ep.Fqdn);                          // "" is absent, not empty-string
        Assert.Equal("Standard_B2s", ep.VmSize);
        Assert.Equal("linux", ep.Os);
        Assert.Equal("networker-rg-endpoint", ep.ResourceGroup);
        Assert.True(ep.Managed);                       // its IP is a deployment endpoint

        var tester = vms[1];
        Assert.Equal("deallocated", tester.Status);
        Assert.Null(tester.PublicIp);
        Assert.False(tester.Managed);
    }

    [Fact]
    public void Aws_projection_falls_back_to_instance_id_and_derives_region_and_os()
    {
        var vms = CloudInventoryScanner.ParseAwsInstances(
            AwsListJson, "us-east-1", ["ec2-54-12-9-3.compute-1.amazonaws.com"]);

        Assert.NotNull(vms);
        var tagged = vms![0];
        Assert.Equal("aws", tagged.Provider);
        Assert.Equal("nwk-ep-aws-1", tagged.Name);
        Assert.Equal("us-east-1", tagged.Region);      // from AZ "us-east-1b"
        Assert.Equal("running", tagged.Status);
        Assert.Equal("linux", tagged.Os);              // Platform is null for non-Windows
        Assert.Equal("t3.small", tagged.VmSize);
        Assert.Null(tagged.ResourceGroup);
        Assert.True(tagged.Managed);                   // matched on the public DNS name

        var untagged = vms[1];
        Assert.Equal("i-0def456", untagged.Name);      // no Name tag → instance id
        Assert.Equal("terminated", untagged.Status);
        Assert.Equal("windows", untagged.Os);
        Assert.Null(untagged.PublicIp);
        Assert.Null(untagged.Fqdn);
        Assert.False(untagged.Managed);
    }

    [Fact]
    public void Gcp_projection_unwraps_selflinks_and_missing_external_ips()
    {
        var vms = CloudInventoryScanner.ParseGcpInstances(GcpListJson, ["34.148.103.234"]);

        Assert.NotNull(vms);
        var running = vms![0];
        Assert.Equal("gcp", running.Provider);
        Assert.Equal("nwk-a-11f093f9", running.Name);
        Assert.Equal("us-east1-b", running.Region);    // zone selfLink → last segment
        Assert.Equal("running", running.Status);       // "RUNNING" → "running"
        Assert.Equal("34.148.103.234", running.PublicIp);
        Assert.Equal("e2-small", running.VmSize);      // machineType selfLink → last segment
        Assert.Equal("linux", running.Os);
        Assert.Null(running.Fqdn);                     // GCE has no public DNS name
        Assert.True(running.Managed);

        var privateOnly = vms[1];
        Assert.Null(privateOnly.PublicIp);
        Assert.False(privateOnly.Managed);
        Assert.Equal("terminated", privateOnly.Status);
    }

    [Theory]
    [InlineData("")]
    [InlineData("   ")]
    [InlineData("not json")]
    [InlineData("{\"error\":\"nope\"}")]
    public void Unexpected_documents_are_reported_not_silently_empty(string json)
    {
        // null (not an empty list) is the contract: the caller turns it into an
        // errors[] line rather than showing "0 VMs" for output it couldn't read.
        Assert.Null(CloudInventoryScanner.ParseAzureVms(json, []));
        Assert.Null(CloudInventoryScanner.ParseAwsInstances(json, "us-east-1", []));
        Assert.Null(CloudInventoryScanner.ParseGcpInstances(json, []));
    }

    [Fact]
    public void Aws_regions_put_the_account_default_first_and_dedupe()
    {
        Assert.Equal(CloudInventoryScanner.DefaultAwsRegions, CloudInventoryScanner.AwsRegionsFor(null));
        Assert.Equal(CloudInventoryScanner.DefaultAwsRegions, CloudInventoryScanner.AwsRegionsFor("   "));

        var withExotic = CloudInventoryScanner.AwsRegionsFor("sa-east-1");
        Assert.Equal("sa-east-1", withExotic[0]);
        Assert.Equal(CloudInventoryScanner.DefaultAwsRegions.Length + 1, withExotic.Count);

        var alreadyDefault = CloudInventoryScanner.AwsRegionsFor("eu-west-1");
        Assert.Equal("eu-west-1", alreadyDefault[0]);
        Assert.Equal(CloudInventoryScanner.DefaultAwsRegions.Length, alreadyDefault.Count);
        Assert.Single(alreadyDefault, r => r == "eu-west-1");
    }

    [Theory]
    [InlineData("us-east-1a", "us-east-1")]
    [InlineData("eu-central-1b", "eu-central-1")]
    [InlineData("ap-southeast-1", "ap-southeast-1")]
    [InlineData(null, null)]
    [InlineData("", null)]
    public void Region_is_derived_from_the_availability_zone(string? az, string? expected)
        => Assert.Equal(expected, CloudInventoryScanner.RegionFromAvailabilityZone(az));

    [Fact]
    public void Identical_region_failures_fold_into_one_line()
    {
        // A bad key fails all three regions with the same message; three copies
        // is noise, and noise is what makes people stop reading errors[].
        var folded = CloudInventoryScanner.SummarizeRegionFailures(
            [("us-east-1", "aws account 'x': bad key"),
             ("us-west-2", "aws account 'x': bad key"),
             ("eu-west-1", "aws account 'x': bad key")], 3);

        var line = Assert.Single(folded);
        Assert.StartsWith("aws account 'x': bad key", line, StringComparison.Ordinal);
        Assert.Contains("in all 3 scanned regions", line, StringComparison.Ordinal);

        // A partial failure names exactly the regions it hit …
        var partial = Assert.Single(CloudInventoryScanner.SummarizeRegionFailures(
            [("eu-west-1", "aws account 'x': throttled")], 3));
        Assert.EndsWith("— in eu-west-1", partial, StringComparison.Ordinal);

        // … and distinct failures stay distinct.
        Assert.Equal(2, CloudInventoryScanner.SummarizeRegionFailures(
            [("us-east-1", "a"), ("us-west-2", "b")], 3).Count);
        Assert.Empty(CloudInventoryScanner.SummarizeRegionFailures([], 3));
    }

    [Fact]
    public void Cli_messages_are_collapsed_truncated_and_stripped_of_credentials()
    {
        var secret = "super-secret-client-value";
        var cleaned = CloudInventoryScanner.CleanMessage(
            $"ERROR: sign-in failed\n  using secret {secret}\n\tretry", [secret]);

        Assert.Equal("ERROR: sign-in failed using secret *** retry", cleaned);
        Assert.DoesNotContain(secret, cleaned, StringComparison.Ordinal);

        // Short "secrets" are never substituted (they'd shred ordinary words).
        Assert.Equal("value is abc", CloudInventoryScanner.CleanMessage("value is abc", ["abc"]));

        var huge = CloudInventoryScanner.CleanMessage(new string('x', 500), []);
        Assert.Equal(301, huge.Length);
        Assert.EndsWith("…", huge, StringComparison.Ordinal);
    }

    // ── Behavioural: real spawn through fake CLIs ────────────────────────────

    [Fact]
    public async Task Scans_azure_with_an_isolated_sign_in_and_cross_references_managed_hosts()
    {
        if (OperatingSystem.IsWindows())
        {
            return; // fake CLI needs /bin/sh
        }

        WriteFakeAz(listStdout: AzureListJson);

        var scan = await NewScanner().ScanAsync(
            [AzureAccount()], ["20.55.12.9"], CancellationToken.None);

        Assert.Empty(scan.Errors);
        Assert.Equal(["azure"], scan.Scanned);
        Assert.Equal(2, scan.Vms.Count);
        Assert.True(scan.Vms[0].Managed);

        var calls = ReadCalls();
        var login = Assert.Single(calls, c => c.Argv.StartsWith("login ", StringComparison.Ordinal));
        var list = Assert.Single(calls, c => c.Argv.StartsWith("vm list ", StringComparison.Ordinal));

        // Signed in as the ACCOUNT's service principal, into a private config dir
        // (never the host's shared az session), and the dir is gone afterwards.
        Assert.Contains("--service-principal", login.Argv, StringComparison.Ordinal);
        Assert.False(string.IsNullOrEmpty(login.AzureConfigDir), "AZURE_CONFIG_DIR must be set");
        Assert.Equal(login.AzureConfigDir, list.AzureConfigDir);
        Assert.False(Directory.Exists(login.AzureConfigDir));

        // Read-only, scoped to the account's subscription.
        Assert.Contains("--subscription sub-prod-1", list.Argv, StringComparison.Ordinal);
        Assert.Contains("--show-details", list.Argv, StringComparison.Ordinal);
        Assert.DoesNotContain(calls, c => c.Argv.Contains("delete", StringComparison.Ordinal)
                                          || c.Argv.Contains("create", StringComparison.Ordinal));
    }

    [Fact]
    public async Task Scans_gcp_with_the_account_key_and_deletes_the_key_afterwards()
    {
        if (OperatingSystem.IsWindows())
        {
            return;
        }

        WriteFakeGcloud(listStdout: GcpListJson);

        var scan = await NewScanner().ScanAsync([GcpAccount()], [], CancellationToken.None);

        Assert.Empty(scan.Errors);
        Assert.Equal(2, scan.Vms.Count);

        var list = Assert.Single(ReadCalls());
        Assert.StartsWith("compute instances list", list.Argv, StringComparison.Ordinal);
        // Authenticated the #827 way: credential override → the account key,
        // project pinned from the key, prompts disabled, key file cleaned up.
        Assert.Equal(GcpJsonKey, list.KeyContent);
        Assert.Equal("kepler-1", list.Project);
        Assert.Equal("1", list.NoPrompts);
        Assert.False(File.Exists(list.KeyOverride));
    }

    [Fact]
    public async Task A_missing_cli_is_an_error_naming_the_override_var_never_an_empty_list()
    {
        // Named "aws" so the launch-failure message can map it back to its
        // override var (CloudCli.OverrideVarFor keys on the base name).
        Environment.SetEnvironmentVariable("AWS_CMD", Path.Combine(_workDir, "nowhere", "aws"));

        var scan = await NewScanner().ScanAsync([AwsAccount()], [], CancellationToken.None);

        Assert.Empty(scan.Vms);
        Assert.Equal(["aws"], scan.Scanned);
        var error = Assert.Single(scan.Errors);
        Assert.StartsWith("aws account 'aws-prod':", error, StringComparison.Ordinal);
        Assert.Contains("AWS_CMD", error, StringComparison.Ordinal);
        // Folded: one line for every region, not five copies of the same thing.
        Assert.Contains($"in all {CloudInventoryScanner.DefaultAwsRegions.Length} scanned regions",
            error, StringComparison.Ordinal);
    }

    [Fact]
    public async Task A_non_zero_exit_reports_the_cleaned_vendor_message()
    {
        if (OperatingSystem.IsWindows())
        {
            return;
        }

        WriteFakeAws(
            listStdout: "",
            exitCode: 254,
            stderr: "An error occurred (InvalidClientTokenId) when calling the DescribeInstances operation: "
                    + "The security token included in the request is invalid.");

        var scan = await NewScanner().ScanAsync([AwsAccount()], [], CancellationToken.None);

        Assert.Empty(scan.Vms);
        var error = Assert.Single(scan.Errors);   // one line, all regions
        Assert.Contains("exit 254", error, StringComparison.Ordinal);
        // ProviderCredentialValidator's de-boilerplating is reused, so the human
        // explanation leads (the UI truncates from the left).
        Assert.Contains("Invalid access key ID", error, StringComparison.Ordinal);
    }

    [Fact]
    public async Task A_hung_cli_is_killed_and_reported_as_a_timeout()
    {
        if (OperatingSystem.IsWindows())
        {
            return;
        }

        WriteFakeGcloud(listStdout: GcpListJson, sleepSeconds: 30);

        var scanner = NewScanner();
        scanner.CommandTimeout = TimeSpan.FromSeconds(1);
        var started = Stopwatch.StartNew();
        var scan = await scanner.ScanAsync([GcpAccount()], [], CancellationToken.None);
        started.Stop();

        Assert.Empty(scan.Vms);
        var error = Assert.Single(scan.Errors);
        Assert.Contains("timed out after 1s and was killed", error, StringComparison.Ordinal);
        Assert.True(started.Elapsed < TimeSpan.FromSeconds(20), $"took {started.Elapsed}");
    }

    [Fact]
    public async Task An_exhausted_account_budget_is_reported_not_a_stuck_request()
    {
        if (OperatingSystem.IsWindows())
        {
            return;
        }

        WriteFakeGcloud(listStdout: GcpListJson, sleepSeconds: 30);

        var scanner = NewScanner();
        // Per-call ceiling far away: only the whole-account budget can fire.
        scanner.CommandTimeout = TimeSpan.FromMinutes(5);
        scanner.AccountBudget = TimeSpan.FromSeconds(1);
        var scan = await scanner.ScanAsync([GcpAccount()], [], CancellationToken.None);

        Assert.Empty(scan.Vms);
        Assert.Contains("scan timed out after 1s", Assert.Single(scan.Errors), StringComparison.Ordinal);
    }

    [Fact]
    public async Task Providers_run_in_parallel_and_one_failure_never_hides_the_others()
    {
        if (OperatingSystem.IsWindows())
        {
            return;
        }

        // Each provider's fake CLI sleeps ~1.5s: run sequentially this is >= 4.5s
        // (AWS alone would be 5 regions × 1.5s), in parallel it is ~1.5-2s.
        WriteFakeAz(listStdout: AzureListJson, sleepSeconds: 1.5);
        WriteFakeGcloud(listStdout: "not json at all", sleepSeconds: 1.5);
        WriteFakeAws(listStdout: AwsListJson, sleepSeconds: 1.5);

        var started = Stopwatch.StartNew();
        var scan = await NewScanner().ScanAsync(
            [AzureAccount(), AwsAccount(), GcpAccount()], [], CancellationToken.None);
        started.Stop();

        Assert.True(started.Elapsed < TimeSpan.FromSeconds(4.5),
            $"providers did not overlap: took {started.Elapsed}");

        // Azure + AWS results survived GCP's unreadable output …
        Assert.Equal(["azure", "aws", "gcp"], scan.Scanned);
        Assert.Contains(scan.Vms, v => v.Provider == "azure");
        Assert.Contains(scan.Vms, v => v.Provider == "aws");
        Assert.DoesNotContain(scan.Vms, v => v.Provider == "gcp");
        // … and GCP's failure is stated, not swallowed.
        var error = Assert.Single(scan.Errors);
        Assert.StartsWith("gcp account 'gcp-lab':", error, StringComparison.Ordinal);
        Assert.Contains("not the expected JSON array", error, StringComparison.Ordinal);
    }

    [Fact]
    public async Task Aws_sweeps_every_region_including_the_account_default()
    {
        if (OperatingSystem.IsWindows())
        {
            return;
        }

        WriteFakeAws(listStdout: "[]");

        var scan = await NewScanner().ScanAsync(
            [AwsAccount() with { RegionDefault = "sa-east-1" }], [], CancellationToken.None);

        Assert.Empty(scan.Errors);
        Assert.Empty(scan.Vms);

        var regions = ReadCalls()
            .Select(c => c.Argv.Split(' ') is var parts && Array.IndexOf(parts, "--region") is var i && i >= 0
                ? parts[i + 1]
                : "")
            .ToList();
        Assert.Equal(CloudInventoryScanner.DefaultAwsRegions.Length + 1, regions.Count);
        Assert.Contains("sa-east-1", regions);
        Assert.Contains("us-east-1", regions);
        // Credentials go through the environment, never the argv.
        Assert.All(ReadCalls(), c => Assert.DoesNotContain("AKIA", c.Argv, StringComparison.Ordinal));
        Assert.All(ReadCalls(), c => Assert.Equal("AKIAFAKEFAKEFAKE", c.AwsKeyId));
    }

    [Fact]
    public async Task No_accounts_means_nothing_scanned_and_no_errors()
    {
        // The design fork, pinned: a provider nobody configured is not a failure.
        // The endpoint reports it as not_configured; errors[] stays clean.
        var scan = await NewScanner().ScanAsync([], [], CancellationToken.None);

        Assert.Empty(scan.Vms);
        Assert.Empty(scan.Errors);
        Assert.Empty(scan.Scanned);
    }

    [Fact]
    public async Task An_account_missing_its_credentials_says_so_without_spawning_anything()
    {
        var scan = await NewScanner().ScanAsync(
            [
                new InventoryAccount("azure", "half-configured", new Dictionary<string, string>(), null),
                new InventoryAccount("gcp", "keyless", new Dictionary<string, string>(), null),
                new InventoryAccount("aws", "keyless", new Dictionary<string, string>(), null),
            ],
            [], CancellationToken.None);

        Assert.Empty(scan.Vms);
        Assert.Equal(3, scan.Errors.Count);
        Assert.Contains(scan.Errors, e => e.Contains("no subscription_id", StringComparison.Ordinal));
        Assert.Contains(scan.Errors, e => e.Contains("no json_key", StringComparison.Ordinal));
        Assert.Contains(scan.Errors, e => e.Contains("no access_key_id", StringComparison.Ordinal));
        Assert.Empty(ReadCalls()); // nothing was spawned
    }

    [Fact]
    public async Task An_unknown_provider_is_reported_rather_than_ignored()
    {
        var scan = await NewScanner().ScanAsync(
            [new InventoryAccount("oracle", "oci", new Dictionary<string, string>(), null)],
            [], CancellationToken.None);

        Assert.Empty(scan.Vms);
        Assert.Equal(["oracle"], scan.Scanned);
        Assert.Contains("unsupported provider", Assert.Single(scan.Errors), StringComparison.Ordinal);
    }

    // ── Harness ──────────────────────────────────────────────────────────────

    private static CloudInventoryScanner NewScanner() =>
        new(NullLogger<CloudInventoryScanner>.Instance);

    private static InventoryAccount AzureAccount() => new(
        "azure", "az-prod",
        new Dictionary<string, string>(StringComparer.Ordinal)
        {
            ["subscription_id"] = "sub-prod-1",
            ["client_id"] = "client-abc",
            ["client_secret"] = "secret-xyz-long-enough",
            ["tenant_id"] = "tenant-123",
        },
        RegionDefault: "eastus");

    private static InventoryAccount AwsAccount() => new(
        "aws", "aws-prod",
        new Dictionary<string, string>(StringComparer.Ordinal)
        {
            ["access_key_id"] = "AKIAFAKEFAKEFAKE",
            ["secret_access_key"] = "fake-secret-access-key-value",
        },
        RegionDefault: null);

    private static InventoryAccount GcpAccount() => new(
        "gcp", "gcp-lab",
        new Dictionary<string, string>(StringComparer.Ordinal) { ["json_key"] = GcpJsonKey },
        RegionDefault: "us-east1");

    private sealed record Call(
        string Argv, string AzureConfigDir, string KeyOverride, string Project,
        string NoPrompts, string KeyContent, string AwsKeyId);

    /// <summary>Fake <c>az</c>: records argv + AZURE_CONFIG_DIR per call, answers
    /// <c>login</c> with success and <c>vm list</c> with a canned document.</summary>
    private void WriteFakeAz(string listStdout, double sleepSeconds = 0) =>
        WriteFakeCli("AZ_CMD", "fake-az.sh", $"""
            case "$1" in
              login) exit 0 ;;
              vm) {Sleep(sleepSeconds)}printf '%s' '{listStdout}'; exit 0 ;;
              *) printf 'unexpected az invocation\n' >&2; exit 1 ;;
            esac
            """);

    /// <summary>Fake <c>aws</c>: one canned answer per region call.</summary>
    private void WriteFakeAws(string listStdout, int exitCode = 0, string stderr = "", double sleepSeconds = 0) =>
        WriteFakeCli("AWS_CMD", "fake-aws.sh", $"""
            {Sleep(sleepSeconds)}printf '%s' '{stderr}' >&2
            printf '%s' '{listStdout}'
            exit {exitCode}
            """);

    /// <summary>Fake <c>gcloud</c>: answers <c>compute instances list</c>.</summary>
    private void WriteFakeGcloud(string listStdout, int exitCode = 0, double sleepSeconds = 0) =>
        WriteFakeCli("GCLOUD_CMD", "fake-gcloud.sh", $"""
            {Sleep(sleepSeconds)}printf '%s' '{listStdout}'
            exit {exitCode}
            """);

    private static string Sleep(double seconds) =>
        seconds > 0 ? $"sleep {seconds.ToString(System.Globalization.CultureInfo.InvariantCulture)}; " : "";

    /// <summary>Write a POSIX-sh fake CLI that records one file per call (argv +
    /// the credential env it was handed + the content of any key file it was
    /// pointed at) and then runs <paramref name="body"/>.
    ///
    /// <para>One file PER CALL, not one shared append log: the scan runs several
    /// CLIs concurrently (5 AWS regions, 3 providers) and interleaved appends
    /// from separate processes would shred the records.</para></summary>
    private void WriteFakeCli(string envVar, string fileName, string body)
    {
        var path = Path.Combine(_workDir, fileName);
        File.WriteAllText(
            path,
            "#!/bin/sh\n" +
            // Unique per call: pid, plus a counter for a process that is called
            // twice (az login then az vm list can't share a pid concurrently).
            $"__n=0\nwhile [ -e '{_callLogDir}/call-'$$-$__n ]; do __n=$((__n+1)); done\n" +
            "{\n" +
            "  printf 'argv=%s\\n' \"$*\"\n" +
            "  printf 'azconfig=%s\\n' \"$AZURE_CONFIG_DIR\"\n" +
            "  printf 'keyoverride=%s\\n' \"$CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE\"\n" +
            "  printf 'project=%s\\n' \"$CLOUDSDK_CORE_PROJECT\"\n" +
            "  printf 'noprompts=%s\\n' \"$CLOUDSDK_CORE_DISABLE_PROMPTS\"\n" +
            "  printf 'awskeyid=%s\\n' \"$AWS_ACCESS_KEY_ID\"\n" +
            "  printf 'keycontent=%s\\n' \"$(cat \"$CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE\" 2>/dev/null)\"\n" +
            $"}} > '{_callLogDir}/call-'$$-$__n\n" +
            body + "\n");
        if (!OperatingSystem.IsWindows())
        {
            // Guarded rather than suppressed: SetUnixFileMode throws on Windows,
            // where every caller of this helper has already returned early.
            File.SetUnixFileMode(
                path,
                UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute);
        }

        Environment.SetEnvironmentVariable(envVar, path);
    }

    private List<Call> ReadCalls()
    {
        var calls = new List<Call>();
        foreach (var file in Directory.GetFiles(_callLogDir).OrderBy(f => f, StringComparer.Ordinal))
        {
            var cur = new Dictionary<string, string>(StringComparer.Ordinal);
            foreach (var line in File.ReadAllLines(file))
            {
                var idx = line.IndexOf('=', StringComparison.Ordinal);
                if (idx > 0)
                {
                    cur[line[..idx]] = line[(idx + 1)..];
                }
            }
            calls.Add(new Call(
                cur.GetValueOrDefault("argv", ""),
                cur.GetValueOrDefault("azconfig", ""),
                cur.GetValueOrDefault("keyoverride", ""),
                cur.GetValueOrDefault("project", ""),
                cur.GetValueOrDefault("noprompts", ""),
                cur.GetValueOrDefault("keycontent", ""),
                cur.GetValueOrDefault("awskeyid", "")));
        }
        return calls;
    }
}
