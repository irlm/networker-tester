using Networker.ControlPlane.Provisioning;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Tests for the pure CLI-argv builders behind
/// <see cref="CliComputeProvisioner"/>'s VM-create path — ports of the Rust
/// <c>build_vm_create_args</c> / <c>build_run_instances_args</c> /
/// <c>build_create_args</c> / <c>azure_windows_computer_name</c> /
/// <c>resolve_ami</c> logic in <c>services/cloud_provider.rs</c>. Every arg
/// list is asserted in full so any drift from the Rust CLI invocations is a
/// test failure.
/// </summary>
public sealed class CliProvisionerCreateArgsTests
{
    private static VmCreateRequest LinuxRequest(string? bootstrap = null) => new(
        Cloud: "azure",
        Name: "tester-eastus-abc12",
        Region: "eastus",
        VmSize: "Standard_B2s",
        SshUser: "azureuser",
        Image: "Canonical:ubuntu-24_04-lts:server:latest",
        BootstrapScript: bootstrap);

    private static VmCreateRequest WindowsRequest() => new(
        Cloud: "azure",
        Name: "tester-eastus-abc12",
        Region: "eastus",
        VmSize: "Standard_D4s_v3",
        SshUser: "azureadmin",
        Image: "MicrosoftWindowsServer:WindowsServer:2022-datacenter-azure-edition:latest",
        BootstrapScript: "$ErrorActionPreference = 'Stop'");

    // ── az vm create ─────────────────────────────────────────────────────────

    [Fact]
    public void BuildAzureCreateArgs_linux_matches_rust_argv()
    {
        var args = CliComputeProvisioner.BuildAzureCreateArgs(
            LinuxRequest(), "sub-123", "rg-testers", adminPassword: null, customDataPath: null);

        Assert.Equal(new[]
        {
            "vm", "create",
            "--subscription", "sub-123",
            "--resource-group", "rg-testers",
            "--name", "tester-eastus-abc12",
            "--location", "eastus",
            "--image", "Canonical:ubuntu-24_04-lts:server:latest",
            "--size", "Standard_B2s",
            "--public-ip-sku", "Standard",
            "--admin-username", "azureuser",
            "--os-disk-delete-option", "Delete",
            "--nic-delete-option", "Delete",
            "--generate-ssh-keys",
            "--output", "json",
        }, args);
    }

    [Fact]
    public void BuildAzureCreateArgs_linux_with_bootstrap_appends_custom_data()
    {
        var args = CliComputeProvisioner.BuildAzureCreateArgs(
            LinuxRequest("#!/bin/bash"), "sub-123", "rg-testers", null, "/tmp/cd123");

        Assert.Equal("--custom-data", args[^2]);
        Assert.Equal("@/tmp/cd123", args[^1]);
    }

    [Fact]
    public void BuildAzureCreateArgs_windows_uses_computer_name_and_password_not_ssh_keys()
    {
        var args = CliComputeProvisioner.BuildAzureCreateArgs(
            WindowsRequest(), "sub-123", "rg-testers", "Sup3r!Secret9x", "/tmp/cd456");

        Assert.DoesNotContain("--generate-ssh-keys", args);

        var computerNameIdx = args.IndexOf("--computer-name");
        Assert.True(computerNameIdx >= 0);
        Assert.Equal("tester-eastus-a", args[computerNameIdx + 1]); // 15-char truncation

        var pwIdx = args.IndexOf("--admin-password");
        Assert.True(pwIdx >= 0);
        Assert.Equal("Sup3r!Secret9x", args[pwIdx + 1]);

        Assert.Equal("--custom-data", args[^2]);
        Assert.Equal("@/tmp/cd456", args[^1]);
    }

    // ── azure_windows_computer_name (mirrors the Rust unit tests) ────────────

    [Theory]
    [InlineData("bm-azure-windows-2022", "bm-azure-window")]
    [InlineData("bm-azure-win11", "bm-azure-win11")]
    [InlineData("bm_azure.win 11", "bm-azure-win-11")]
    [InlineData("1234567890", "w1234567890")]
    [InlineData("", "w")]
    [InlineData("name-with-trailing---", "name-with-trail")]
    public void AzureWindowsComputerName_matches_rust(string input, string expected)
    {
        Assert.Equal(expected, CliComputeProvisioner.AzureWindowsComputerName(input));
    }

    // ── aws resolve_ami filter map ───────────────────────────────────────────

    [Theory]
    [InlineData("aws:ubuntu-24.04-server", "099720109477", "ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-amd64-server-*")]
    [InlineData("aws:ubuntu-22.04-server", "099720109477", "ubuntu/images/hvm-ssd/ubuntu-jammy-22.04-amd64-server-*")]
    [InlineData("aws:debian-12-server", "136693071363", "debian-12-amd64-*")]
    [InlineData("aws:windows-2022-server", "801119661308", "Windows_Server-2022-English-Full-Base-*")]
    [InlineData("aws:unknown-thing", "099720109477", "ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-amd64-server-*")]
    [InlineData("ubuntu-24.04-server", "099720109477", "ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-amd64-server-*")]
    public void AwsAmiFilter_matches_rust(string marker, string owner, string filter)
    {
        var (o, f) = CliComputeProvisioner.AwsAmiFilter(marker);
        Assert.Equal(owner, o);
        Assert.Equal(filter, f);
    }

    // ── aws ec2 run-instances ────────────────────────────────────────────────

    [Fact]
    public void BuildAwsRunInstancesArgs_matches_rust_argv()
    {
        var request = new VmCreateRequest(
            "aws", "tester-us-east-1-ab12c", "us-east-1", "t3.small", "ubuntu",
            "aws:ubuntu-24.04-server", "#!/bin/bash");

        var args = CliComputeProvisioner.BuildAwsRunInstancesArgs(
            request, "ami-0123456789abcdef0", "alethedash-tester", "sg-0aa11bb22cc33dd44", "/tmp/ud789");

        Assert.Equal(new[]
        {
            "ec2", "run-instances",
            "--image-id", "ami-0123456789abcdef0",
            "--instance-type", "t3.small",
            "--region", "us-east-1",
            "--key-name", "alethedash-tester",
            "--security-group-ids", "sg-0aa11bb22cc33dd44",
            "--associate-public-ip-address",
            "--tag-specifications", "ResourceType=instance,Tags=[{Key=Name,Value=tester-us-east-1-ab12c}]",
            "--query", "Instances[0]",
            "--output", "json",
            "--user-data", "file:///tmp/ud789",
        }, args);
    }

    [Fact]
    public void BuildAwsRunInstancesArgs_without_bootstrap_omits_user_data()
    {
        var request = new VmCreateRequest(
            "aws", "t", "us-east-1", "t3.small", "ubuntu", "aws:ubuntu-24.04-server", null);
        var args = CliComputeProvisioner.BuildAwsRunInstancesArgs(request, "ami-1", "k", "sg-1", null);
        Assert.DoesNotContain("--user-data", args);
    }

    // ── gcloud compute instances create ──────────────────────────────────────

    [Fact]
    public void BuildGcpCreateArgs_matches_rust_argv()
    {
        var request = new VmCreateRequest(
            "gcp", "tester-us-central1-9f8e7", "us-central1", "e2-small", "ubuntu",
            "ubuntu-2404-lts-amd64", "#!/bin/bash");

        var args = CliComputeProvisioner.BuildGcpCreateArgs(
            request, "us-central1-a", "/tmp/sshkeys", "/tmp/startup");

        Assert.Equal(new[]
        {
            "compute", "instances", "create", "tester-us-central1-9f8e7",
            "--zone", "us-central1-a",
            "--machine-type", "e2-small",
            "--image-family", "ubuntu-2404-lts-amd64",
            "--image-project", "ubuntu-os-cloud",
            "--tags", "alethedash-tester",
            "--format", "json",
            "--metadata-from-file", "ssh-keys=/tmp/sshkeys",
            "--metadata-from-file", "startup-script=/tmp/startup",
        }, args);
    }

    [Theory]
    [InlineData("ubuntu-2404-lts-amd64", "ubuntu-os-cloud")]
    [InlineData("debian-12", "debian-cloud")]
    [InlineData("windows-2022", "windows-cloud")]
    [InlineData("something-else", "ubuntu-os-cloud")]
    public void BuildGcpCreateArgs_image_project_mapping(string image, string expectedProject)
    {
        var request = new VmCreateRequest("gcp", "t", "us-central1", "e2-small", "ubuntu", image, null);
        var args = CliComputeProvisioner.BuildGcpCreateArgs(request, "us-central1-a", null, null);
        var idx = args.IndexOf("--image-project");
        Assert.Equal(expectedProject, args[idx + 1]);
        Assert.DoesNotContain("--metadata-from-file", args);
    }
}
