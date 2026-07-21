using Microsoft.Extensions.Logging.Abstractions;
using Networker.ControlPlane.Provisioning;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// End-to-end behavioural tests for the Azure tester-delete cascade — the
/// self-cleaning NSG + public-IP follow-up deletes added to
/// <see cref="CliComputeProvisioner.DeleteAsync"/>.
///
/// <para>Each test points <c>AZ_CMD</c> (and <c>AWS_CMD</c>/<c>GCLOUD_CMD</c>) at
/// a tiny fake CLI script that appends its argv to an invocation log, so we can
/// assert <b>exactly</b> which CLI calls the cascade made (and, crucially, which
/// it did NOT) without any real cloud access. The script's exit code / stderr is
/// scripted per-test to simulate success, "already gone", or a hard failure.</para>
///
/// <para>The fake CLI is a POSIX-sh script, so these assertions run on Unix/macOS
/// CI. On Windows the test returns early (no <c>/bin/sh</c>); the pure argv
/// builders in <see cref="CliProvisionerCreateArgsTests"/> still cover Windows.</para>
/// </summary>
public sealed class CliProvisionerDeleteCascadeTests : IDisposable
{
    private readonly string _workDir;
    private readonly string _logPath;

    public CliProvisionerDeleteCascadeTests()
    {
        _workDir = Path.Combine(Path.GetTempPath(), $"cascade-test-{Guid.NewGuid():N}");
        Directory.CreateDirectory(_workDir);
        _logPath = Path.Combine(_workDir, "invocations.log");
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

    /// <summary>
    /// Write a POSIX-sh fake CLI that logs its argv (one line, space-joined) to
    /// <see cref="_logPath"/> then exits with <paramref name="exitCode"/>, first
    /// printing <paramref name="stderr"/> to stderr. Registers it under the given
    /// override env var (<c>AZ_CMD</c> / <c>AWS_CMD</c> / <c>GCLOUD_CMD</c>).
    /// </summary>
    private void WriteFakeCli(string overrideVar, int exitCode = 0, string stderr = "")
    {
        var path = Path.Combine(_workDir, $"fake-{overrideVar}.sh");
        var stderrLine = string.IsNullOrEmpty(stderr) ? "" : $"echo '{stderr}' >&2\n";
        File.WriteAllText(
            path,
            "#!/bin/sh\n" +
            $"printf '%s\\n' \"$*\" >> '{_logPath}'\n" +
            stderrLine +
            $"exit {exitCode}\n");
        File.SetUnixFileMode(
            path,
            UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute);

        Environment.SetEnvironmentVariable(overrideVar, path);
    }

    private IReadOnlyList<string> InvocationLines() =>
        File.Exists(_logPath) ? File.ReadAllLines(_logPath) : Array.Empty<string>();

    private static ProjectTester AzureTester(string? vmName) =>
        new()
        {
            TesterId = Guid.NewGuid(),
            ProjectId = "p",
            Name = "t",
            Cloud = "azure",
            Region = "eastus",
            VmSize = "Standard_B2s",
            SshUser = "azureuser",
            PowerState = "running",
            Allocation = "on-demand",
            VmName = vmName,
            VmResourceId =
                "/subscriptions/sub-123/resourceGroups/networker-testers/providers/Microsoft.Compute/virtualMachines/tester-eastus-5cab8",
        };

    private static CliComputeProvisioner NewProvisioner() =>
        new(NullLogger<CliComputeProvisioner>.Instance);

    private static ProviderCredentials AzureCreds() =>
        new("azure", SubscriptionId: "sub-123", ResourceGroup: "networker-testers");

    [Fact]
    public async Task AzureDelete_success_deletes_vm_then_publicIp_then_nsg()
    {
        if (OperatingSystem.IsWindows())
        {
            return; // fake CLI needs /bin/sh
        }

        WriteFakeCli("AZ_CMD", exitCode: 0);

        var result = await NewProvisioner().DeleteAsync(
            AzureTester("tester-eastus-5cab8"), AzureCreds(), CancellationToken.None);

        Assert.True(result.Success);

        var lines = InvocationLines();
        Assert.Equal(3, lines.Count);

        // 1. VM delete (releases the NIC/IP association).
        Assert.Contains("vm delete", lines[0]);
        Assert.Contains("--yes", lines[0]);

        // 2. public IP BEFORE nsg, both by exact name + rg + subscription.
        Assert.Contains("network public-ip delete", lines[1]);
        Assert.Contains("--name tester-eastus-5cab8PublicIP", lines[1]);
        Assert.Contains("--resource-group networker-testers", lines[1]);
        Assert.Contains("--subscription sub-123", lines[1]);

        Assert.Contains("network nsg delete", lines[2]);
        Assert.Contains("--name tester-eastus-5cab8NSG", lines[2]);
        Assert.Contains("--resource-group networker-testers", lines[2]);
        Assert.Contains("--subscription sub-123", lines[2]);

        // Never a list/filter that could hit another tester (#419 safety).
        Assert.DoesNotContain(lines, l => l.Contains("nsg list") || l.Contains("public-ip list"));
    }

    [Fact]
    public async Task AzureDelete_empty_vmName_skips_nsg_and_ip_cleanup()
    {
        if (OperatingSystem.IsWindows())
        {
            return;
        }

        WriteFakeCli("AZ_CMD", exitCode: 0);

        var result = await NewProvisioner().DeleteAsync(
            AzureTester(vmName: null), AzureCreds(), CancellationToken.None);

        Assert.True(result.Success);

        var lines = InvocationLines();
        // Only the VM delete ran — no NSG/IP calls, because we can't derive the
        // exact names without a vm_name (never guess).
        Assert.Single(lines);
        Assert.Contains("vm delete", lines[0]);
        Assert.DoesNotContain(lines, l => l.Contains("nsg") || l.Contains("public-ip"));
    }

    [Fact]
    public async Task AzureDelete_notFound_nsg_and_ip_does_not_fail_overall_delete()
    {
        if (OperatingSystem.IsWindows())
        {
            return;
        }

        // Every az call exits non-zero with a "not found" stderr — the VM delete
        // is treated as already-gone (success), and the NSG/IP not-found deletes
        // must NOT flip the result to failure.
        WriteFakeCli("AZ_CMD", exitCode: 1, stderr: "ResourceNotFound: was not found");

        var result = await NewProvisioner().DeleteAsync(
            AzureTester("tester-eastus-5cab8"), AzureCreds(), CancellationToken.None);

        Assert.True(result.Success);

        var lines = InvocationLines();
        Assert.Equal(3, lines.Count); // VM + IP + NSG all attempted, all tolerated.
    }

    [Fact]
    public async Task AzureDelete_hard_vm_failure_skips_cascade_and_reports_failure()
    {
        if (OperatingSystem.IsWindows())
        {
            return;
        }

        // A real (non "already gone") failure on the VM delete: the overall result
        // is a failure, and the NSG/IP cascade is guarded off (only runs on a
        // successful/already-gone VM delete).
        WriteFakeCli("AZ_CMD", exitCode: 3, stderr: "AuthorizationFailed: nope");

        var result = await NewProvisioner().DeleteAsync(
            AzureTester("tester-eastus-5cab8"), AzureCreds(), CancellationToken.None);

        Assert.False(result.Success);
        var lines = InvocationLines();
        Assert.Single(lines); // only the VM delete attempt; cascade skipped on failure
        Assert.Contains("vm delete", lines[0]);
    }

    [Fact]
    public async Task AwsDelete_never_makes_nsg_or_ip_calls()
    {
        if (OperatingSystem.IsWindows())
        {
            return;
        }

        WriteFakeCli("AWS_CMD", exitCode: 0);

        var tester = new ProjectTester
        {
            TesterId = Guid.NewGuid(),
            ProjectId = "p",
            Name = "t",
            Cloud = "aws",
            Region = "us-east-1",
            VmSize = "t3.small",
            SshUser = "ubuntu",
            PowerState = "running",
            Allocation = "on-demand",
            VmName = "tester-us-east-1-ab12c",
            VmResourceId = "i-0123456789abcdef0",
        };

        var result = await NewProvisioner().DeleteAsync(
            tester, new ProviderCredentials("aws", Region: "us-east-1"), CancellationToken.None);

        Assert.True(result.Success);
        var lines = InvocationLines();
        Assert.Single(lines);
        Assert.Contains("ec2 terminate-instances", lines[0]);
        Assert.DoesNotContain(lines, l => l.Contains("nsg") || l.Contains("public-ip"));
    }

    [Fact]
    public async Task GcpDelete_never_makes_nsg_or_ip_calls()
    {
        if (OperatingSystem.IsWindows())
        {
            return;
        }

        WriteFakeCli("GCLOUD_CMD", exitCode: 0);

        var tester = new ProjectTester
        {
            TesterId = Guid.NewGuid(),
            ProjectId = "p",
            Name = "t",
            Cloud = "gcp",
            Region = "us-central1",
            VmSize = "e2-small",
            SshUser = "ubuntu",
            PowerState = "running",
            Allocation = "on-demand",
            VmName = "tester-us-central1-9f8e7",
            VmResourceId =
                "https://www.googleapis.com/compute/v1/projects/proj/zones/us-central1-a/instances/tester-us-central1-9f8e7",
        };

        var result = await NewProvisioner().DeleteAsync(
            tester, new ProviderCredentials("gcp"), CancellationToken.None);

        Assert.True(result.Success);
        var lines = InvocationLines();
        Assert.Single(lines);
        Assert.Contains("compute instances delete", lines[0]);
        Assert.DoesNotContain(lines, l => l.Contains("nsg") || l.Contains("public-ip"));
    }
}
