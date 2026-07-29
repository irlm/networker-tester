using Networker.ControlPlane.Endpoints;
using Networker.ControlPlane.Provisioning;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Unit tests for the deploy-VM teardown reverse-lookup (E2E pass 2026-07-28
/// P1-16): a deployment row stores only the endpoint IP/FQDN (deploy VMs are
/// created by install.sh, never through the C# provisioner, so there is no
/// stored resource id), and delete used to orphan the VM. These lock down the
/// two pure pieces — the <c>az vm list -d</c> endpoint matcher and the provider
/// fallback parser. The live cloud delete is verified against a real provisioned
/// deployment in the E2E cycle.
/// </summary>
public class DeployVmTeardownTests
{
    // Two VMs; the second owns the endpoint we search for. `publicIps`/`fqdns`
    // are comma-joined strings exactly as `az vm list -d` emits them.
    private const string ListJson = """
        [
          { "name": "other-vm", "id": "/subscriptions/s1/resourceGroups/rg/providers/Microsoft.Compute/virtualMachines/other-vm",
            "publicIps": "20.1.2.30", "fqdns": "other.eastus.cloudapp.azure.com" },
          { "name": "nwk-auto-fu-vm", "id": "/subscriptions/s1/resourceGroups/networker-rg-endpoint/providers/Microsoft.Compute/virtualMachines/nwk-auto-fu-vm",
            "publicIps": "20.1.2.3", "fqdns": "nwk-auto-fu.eastus.cloudapp.azure.com" }
        ]
        """;

    [Fact]
    public void Matches_vm_by_exact_public_ip()
    {
        var vm = CliComputeProvisioner.MatchAzureVmByEndpoint(ListJson, "20.1.2.3");

        Assert.NotNull(vm);
        Assert.Equal("nwk-auto-fu-vm", vm!.Name);
        Assert.EndsWith("/virtualMachines/nwk-auto-fu-vm", vm.ResourceId);
    }

    [Fact]
    public void Matches_vm_by_fqdn_when_endpoint_was_captured_as_a_hostname()
    {
        var vm = CliComputeProvisioner.MatchAzureVmByEndpoint(ListJson, "nwk-auto-fu.eastus.cloudapp.azure.com");

        Assert.NotNull(vm);
        Assert.Equal("nwk-auto-fu-vm", vm!.Name);
    }

    [Fact]
    public void Ip_match_is_exact_entry_not_prefix()
    {
        // "20.1.2.3" must NOT match the other VM's "20.1.2.30" (substring trap).
        var vm = CliComputeProvisioner.MatchAzureVmByEndpoint(ListJson, "20.1.2.3");
        Assert.Equal("nwk-auto-fu-vm", vm!.Name);
    }

    [Theory]
    [InlineData("203.0.113.9")]            // no VM owns this IP
    [InlineData("")]                        // empty endpoint
    [InlineData("   ")]                     // whitespace
    public void Returns_null_when_no_vm_owns_the_endpoint(string endpoint)
        => Assert.Null(CliComputeProvisioner.MatchAzureVmByEndpoint(ListJson, endpoint));

    [Theory]
    [InlineData("not json")]
    [InlineData("{}")]                      // object, not the expected array
    [InlineData("[]")]                      // empty list
    public void Returns_null_for_non_list_or_malformed_output(string json)
        => Assert.Null(CliComputeProvisioner.MatchAzureVmByEndpoint(json, "20.1.2.3"));

    [Fact]
    public void Handles_multi_ip_vms_joined_by_comma()
    {
        const string multi = """
            [ { "name": "vm", "id": "/subscriptions/s/resourceGroups/rg/providers/Microsoft.Compute/virtualMachines/vm",
                "publicIps": "10.0.0.1,20.1.2.3", "fqdns": "" } ]
            """;
        var vm = CliComputeProvisioner.MatchAzureVmByEndpoint(multi, "20.1.2.3");
        Assert.Equal("vm", vm!.Name);
    }

    [Fact]
    public void FirstProviderFromConfig_reads_endpoints_provider()
    {
        const string cfg = """
            { "version": 1, "endpoints": [ { "provider": "azure", "label": "x" } ], "tester": { "provider": "local" } }
            """;
        Assert.Equal("azure", DeploymentWriteEndpoints.FirstProviderFromConfig(cfg));
    }

    [Theory]
    [InlineData("""{ "endpoints": [] }""")]          // no endpoints
    [InlineData("""{ "endpoints": [ {} ] }""")]      // endpoint without provider
    [InlineData("""{ }""")]                            // no endpoints key
    [InlineData("not json")]
    [InlineData(null)]
    public void FirstProviderFromConfig_returns_null_when_absent(string? cfg)
        => Assert.Null(DeploymentWriteEndpoints.FirstProviderFromConfig(cfg));
}
