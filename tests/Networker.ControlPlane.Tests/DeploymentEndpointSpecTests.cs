using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Unit tests for <see cref="DeploymentsEndpoints.ParseEndpointSpecs"/> — the
/// deployment-config parse behind the deployment cost/identity view. The VM
/// size lives under a provider-specific key (azure vm_size / aws instance_type
/// / gcp machine_type) and ssh/lan targets have none; the parse must be
/// tolerant of malformed config (it feeds a read-only detail page).
/// </summary>
public class DeploymentEndpointSpecTests
{
    [Fact]
    public void Parses_provider_specific_vm_size_keys()
    {
        var specs = DeploymentsEndpoints.ParseEndpointSpecs("""
            { "endpoints": [
                { "label": "az", "provider": "azure", "region": "eastus", "vm_size": "Standard_B2s", "os": "linux" },
                { "label": "aws", "provider": "aws", "region": "us-east-1", "instance_type": "t3.small" },
                { "label": "gcp", "provider": "gcp", "zone": "us-central1-a", "machine_type": "e2-small" }
            ] }
            """);

        Assert.Equal(3, specs.Count);
        Assert.Equal(("azure", "eastus", "Standard_B2s", "linux"), (specs[0].Provider, specs[0].Region, specs[0].VmSize, specs[0].Os));
        Assert.Equal("t3.small", specs[1].VmSize);
        // gcp: region falls back to zone.
        Assert.Equal(("us-central1-a", "e2-small"), (specs[2].Region, specs[2].VmSize));
    }

    [Fact]
    public void Falls_back_to_the_provider_block_for_cloud_configs()
    {
        // The REAL shape the deploy wizard + provisioning orchestrator write:
        // region/vm_size/os/vm_name nested under ep.<provider>, top level null.
        // (Copied from a prod deployment that rendered Region/VM size/OS as "—".)
        var specs = DeploymentsEndpoints.ParseEndpointSpecs("""
            { "endpoints": [
                { "provider": "azure", "region": null, "vm_size": null, "os": null,
                  "http_stacks": ["nginx", "caddy"],
                  "azure": { "os": "linux", "region": "eastus", "vm_name": "nwk-ep-ubuntu-edne", "vm_size": "Standard_B2s" } },
                { "provider": "aws",
                  "aws": { "region": "us-east-1", "instance_type": "t3.small", "os": "linux", "instance_name": "nwk-ep-aws-1" } },
                { "provider": "gcp",
                  "gcp": { "zone": "us-central1-a", "machine_type": "e2-small", "os": "linux" } }
            ] }
            """);

        Assert.Equal(3, specs.Count);
        Assert.Equal(("eastus", "Standard_B2s", "linux", "nwk-ep-ubuntu-edne"),
            (specs[0].Region, specs[0].VmSize, specs[0].Os, specs[0].VmName));
        Assert.Equal(("us-east-1", "t3.small", "nwk-ep-aws-1"), (specs[1].Region, specs[1].VmSize, specs[1].VmName));
        Assert.Equal(("us-central1-a", "e2-small"), (specs[2].Region, specs[2].VmSize));
    }

    [Fact]
    public void Top_level_wins_over_the_provider_block()
    {
        var specs = DeploymentsEndpoints.ParseEndpointSpecs("""
            { "endpoints": [
                { "provider": "azure", "region": "westus2",
                  "azure": { "region": "eastus", "vm_size": "Standard_B2s" } }
            ] }
            """);

        var s = Assert.Single(specs);
        Assert.Equal("westus2", s.Region);          // explicit top-level beats nested
        Assert.Equal("Standard_B2s", s.VmSize);     // nested fills the gap
    }

    [Fact]
    public void Ssh_target_has_no_vm_size_and_gets_default_label()
    {
        var specs = DeploymentsEndpoints.ParseEndpointSpecs("""
            { "endpoints": [ { "provider": "ssh", "ip": "10.0.0.5" } ] }
            """);

        var s = Assert.Single(specs);
        Assert.Null(s.VmSize);          // priced as null, never a made-up number
        Assert.Equal("endpoint 1", s.Label);
        Assert.Equal("ssh", s.Provider);
    }

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("not json")]
    [InlineData("{}")]                                  // no endpoints array
    [InlineData("""{ "endpoints": "oops" }""")]         // wrong type
    public void Malformed_config_yields_empty_not_throw(string? raw)
        => Assert.Empty(DeploymentsEndpoints.ParseEndpointSpecs(raw));

    [Fact]
    public void Non_object_entries_are_skipped_but_numbering_is_positional()
    {
        var specs = DeploymentsEndpoints.ParseEndpointSpecs("""
            { "endpoints": [ 42, { "provider": "azure", "vm_size": "Standard_B1s" } ] }
            """);

        var s = Assert.Single(specs);
        Assert.Equal("endpoint 2", s.Label); // positional — matches the config array
        Assert.Equal("Standard_B1s", s.VmSize);
    }
}
