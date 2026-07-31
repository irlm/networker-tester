using System.Text.Json.Nodes;
using Networker.ControlPlane.Endpoints;
using Networker.ControlPlane.Provisioning;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pins the uniqueness of auto-provisioned VM names and matrix config names.
/// Regression (2026-07-31): the VM label was derived from the first 8 chars of
/// the CONFIG name, and every comparison-group cell shares a config-name
/// prefix ("Azure/eastus …") — so a 10-cell matrix raced to create one Azure
/// VM named "nwk-auto-azuree": nine cells failed with Conflict and the winner
/// was stomped by the other cells' installers. The label must come from the
/// run id, which is unique per cell and re-launch.
/// </summary>
public class DeployVmNamingTests
{
    private static readonly ProvisioningOrchestrator.PendingEndpoint Pending = new(
        Guid.NewGuid(), "eastus", "Standard_B2s", "linux", "nginx", null);

    private static string VmName(JsonObject deployJson) =>
        deployJson["endpoints"]![0]!["azure"]!["vm_name"]!.GetValue<string>();

    [Fact]
    public void Cells_with_identical_config_name_prefixes_get_distinct_vm_names()
    {
        // The exact shape that collided: same-prefix cell config names.
        var a = ProvisioningOrchestrator.BuildDeployJson(
            Pending, "azure", "Azure/eastus linux · nginx · cg-f98c0c24·0", Guid.NewGuid());
        var b = ProvisioningOrchestrator.BuildDeployJson(
            Pending, "azure", "Azure/eastus linux · Caddy · cg-f98c0c24·1", Guid.NewGuid());

        Assert.NotEqual(VmName(a), VmName(b));
    }

    [Fact]
    public void Vm_name_is_deterministic_per_run_and_azure_valid()
    {
        var runId = Guid.NewGuid();
        var one = VmName(ProvisioningOrchestrator.BuildDeployJson(Pending, "azure", "cfg", runId));
        var two = VmName(ProvisioningOrchestrator.BuildDeployJson(Pending, "azure", "cfg", runId));

        Assert.Equal(one, two); // same run → same VM, correlatable with the deployment row
        Assert.Matches("^nwk-a-[a-z0-9]{8}$", one);
        Assert.InRange(one.Length, 1, 15); // Windows NetBIOS / install.sh cap
    }

    [Theory]
    [InlineData("aws", "instance_name")]
    [InlineData("gcp", "instance_name")]
    public void Non_azure_providers_carry_the_same_run_derived_name(string provider, string key)
    {
        var runId = Guid.NewGuid();
        var json = ProvisioningOrchestrator.BuildDeployJson(Pending, provider, "cfg", runId);
        var name = json["endpoints"]![0]![provider]![key]!.GetValue<string>();
        Assert.Matches("^nwk-a-[a-z0-9]{8}$", name);
    }

    [Fact]
    public void Relaunching_a_group_produces_fresh_config_names()
    {
        var groupId = Guid.NewGuid();
        var first = ComparisonGroupsEndpoints.CellConfigName("Azure/eastus linux · nginx", groupId, 0, "aaaa");
        var second = ComparisonGroupsEndpoints.CellConfigName("Azure/eastus linux · nginx", groupId, 0, "bbbb");

        // Same group, same cell index — UNIQUE(project_id, name) must still pass.
        Assert.NotEqual(first, second);
        Assert.StartsWith("Azure/eastus linux · nginx · cg-", first);
    }

    [Fact]
    public void Cells_within_one_launch_stay_distinct()
    {
        var groupId = Guid.NewGuid();
        var names = Enumerable.Range(0, 10)
            .Select(i => ComparisonGroupsEndpoints.CellConfigName("same label", groupId, i, "aaaa"))
            .ToHashSet();
        Assert.Equal(10, names.Count);
    }
}
