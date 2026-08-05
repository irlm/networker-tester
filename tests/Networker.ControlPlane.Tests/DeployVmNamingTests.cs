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

/// <summary>Pins the workload-derived cell deadline (v0.28.136): the fixed
/// 900s deadline was impossible for real matrix workloads — every cell that
/// reached the runner was killed at ~16 minutes (2026-08-03).</summary>
public class CellMaxDurationTests
{
    [Fact]
    public void Full_matrix_workload_gets_hours_not_minutes()
        // runs=100 × 26 modes → 100*26*8 + 600 = 21400s ≈ 6h (8s/unit after the
        // live 2026-08-04 measurement: real attempts ≈ 1.7× runs×modes and the
        // slower proxies overshot the 4s budget at 78-85% complete).
        => Assert.Equal(21400, ComparisonGroupsEndpoints.CellMaxDurationSecs(
            """{"runs":100,"modes":["a","b","c","d","e","f","g","h","i","j","k","l","m","n","o","p","q","r","s","t","u","v","w","x","y","z"]}"""));

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("not json")]
    [InlineData("[]")]
    public void Missing_or_malformed_workload_falls_back_to_default(string? json)
        => Assert.Equal(900, ComparisonGroupsEndpoints.CellMaxDurationSecs(json));

    [Fact]
    public void Small_workloads_keep_the_original_floor()
        => Assert.Equal(900, ComparisonGroupsEndpoints.CellMaxDurationSecs(
            """{"runs":10,"modes":["download","upload"]}"""));

    [Fact]
    public void Estimate_is_capped_at_eight_hours()
        => Assert.Equal(28800, ComparisonGroupsEndpoints.CellMaxDurationSecs(
            """{"runs":100000,"modes":["a","b","c"]}"""));
}

/// <summary>Pins the launch-time unsupported-combo gate (v0.28.141).</summary>
public class UnsupportedComboTests
{
    private static ComparisonGroupsEndpoints.CellSpec Cell(string os, string stack) => new(
        $"Azure/eastus {os} · {stack}",
        $$"""{"kind":"pending","cloud_account_id":"{{Guid.NewGuid()}}","region":"eastus","vm_size":"Standard_B2s","os":"{{os}}","proxy_stack":"{{stack}}"}""",
        "pending",
        null);

    [Fact]
    public void Windows_haproxy_is_rejected_with_reason()
    {
        var why = ComparisonGroupsEndpoints.UnsupportedComboReason(Cell("windows", "haproxy"));
        Assert.NotNull(why);
        Assert.Contains("no native Windows build", why);
    }

    [Fact]
    public void Windows_apache_is_rejected_with_reason()
    {
        // Apache Lounge serves an HTML decoy to every scripted download and no
        // other Windows httpd binary source exists (verified 2026-08-04).
        var why = ComparisonGroupsEndpoints.UnsupportedComboReason(Cell("windows", "apache"));
        Assert.NotNull(why);
        Assert.Contains("no scriptable Windows binary source", why);
    }

    [Theory]
    [InlineData("linux", "haproxy")]
    [InlineData("linux", "apache")]
    [InlineData("windows", "iis")]
    [InlineData("windows", "traefik")]
    [InlineData("windows", "caddy")]
    [InlineData("linux", "nginx")]
    public void Supported_combos_pass(string os, string stack)
        => Assert.Null(ComparisonGroupsEndpoints.UnsupportedComboReason(Cell(os, stack)));

    [Fact]
    public void Non_pending_cells_are_never_gated()
        => Assert.Null(ComparisonGroupsEndpoints.UnsupportedComboReason(
            new ComparisonGroupsEndpoints.CellSpec("net", """{"kind":"network","host":"h","port":1}""", "network", null)));
}
