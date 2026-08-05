using Networker.ControlPlane.Background;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pins the endpoint-VM lifecycle safety net added in v0.28.131 after the
/// 2026-08-01 matrix failure: auto-provisioned cell VMs (nwk-a-*) leaked
/// forever — the reaper's prefix allow-list didn't cover them AND it never
/// swept the endpoint resource group. Adding the sweep makes the deployment
/// vm-name guard load-bearing: without it the reaper would identify the
/// user's STANDING wizard target (nwk-ep-*, present in no tester row) as an
/// orphan and delete it.
/// </summary>
public class EndpointVmLifecycleTests
{
    // ── deployment vm-name guard (the standing-target protection) ─────────────

    [Fact]
    public void Standing_target_and_children_are_protected_by_deployment_vm_name()
    {
        var known = new HashSet<string>(StringComparer.OrdinalIgnoreCase); // in no tester row
        var liveNames = new[] { "nwk-ep-ubuntu-edne" }; // from its live deployment row
        var raw = new[]
        {
            new OrphanReaperService.RawResource("/vm/t", "nwk-ep-ubuntu-edne", "vm", "azure"),
            new OrphanReaperService.RawResource("/nic/t", "nwk-ep-ubuntu-edneVMNic", "nic", "azure"),
            new OrphanReaperService.RawResource("/ip/t", "nwk-ep-ubuntu-ednePublicIP", "public_ip", "azure"),
            new OrphanReaperService.RawResource("/nsg/t", "nwk-ep-ubuntu-edneNSG", "nsg", "azure"),
            // A dead cell's leftover in the same RG — MUST still be reaped.
            new OrphanReaperService.RawResource("/nsg/dead", "nwk-a-bbb8aca2NSG", "nsg", "azure"),
        };

        var orphans = OrphanReaperService.FilterOrphans(raw, known, liveNames);

        Assert.Single(orphans);
        Assert.Equal("/nsg/dead", orphans[0].ResourceId);
    }

    [Fact]
    public void In_flight_cell_vm_is_protected_while_its_deployment_row_lives()
    {
        var known = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var liveNames = new[] { "nwk-a-ebf51aea" }; // live (not torn_down) deployment
        var raw = new[]
        {
            new OrphanReaperService.RawResource("/vm/c", "nwk-a-ebf51aea", "vm", "azure"),
            new OrphanReaperService.RawResource("/nic/c", "nwk-a-ebf51aeaVMNic", "nic", "azure"),
        };

        Assert.Empty(OrphanReaperService.FilterOrphans(raw, known, liveNames));
    }

    // ── vm-name extraction from deploy.json ───────────────────────────────────

    [Theory]
    [InlineData("""{"endpoints":[{"azure":{"vm_name":"nwk-a-12345678"}}]}""", "nwk-a-12345678")]
    [InlineData("""{"endpoints":[{"aws":{"instance_name":"nwk-a-abc"}}]}""", "nwk-a-abc")]
    [InlineData("""{"endpoints":[{"gcp":{"instance_name":"nwk-a-def"}}]}""", "nwk-a-def")]
    [InlineData("""{"endpoints":[{"lan":{"ip":"10.0.0.5"}}]}""", null)] // no VM name → simply unguarded
    [InlineData("""{"endpoints":[]}""", null)]
    [InlineData("""{}""", null)]
    [InlineData("not json", null)]
    public void VmNameFromDeployConfig_reads_provider_blocks(string config, string? expected)
        => Assert.Equal(expected, OrphanReaperService.VmNameFromDeployConfig(config));

    [Fact]
    public void VmNameFromDeployConfig_null_or_empty_is_null()
    {
        Assert.Null(OrphanReaperService.VmNameFromDeployConfig(null));
        Assert.Null(OrphanReaperService.VmNameFromDeployConfig(""));
    }

    // ── torn_down status string stability ─────────────────────────────────────

    [Fact]
    public void Torn_down_status_is_pinned()
        // The reaper's live-deployment filter and the orchestrator's throttle
        // both compare this literal against DB rows — a rename breaks both.
        => Assert.Equal("torn_down", Provisioning.ProvisioningOrchestrator.DeploymentTornDown);
}
