using Networker.ControlPlane.Endpoints;
using Networker.ControlPlane.Infra;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// The VM network-spec catalog behind the run report's infrastructure
/// envelope, and the endpoint_ref → deployment-id parse the /infra route uses.
/// The B2s entry is load-bearing: its estimate was calibrated against a live
/// measurement (530–580 Mbps egress, 2026-07-31) — if someone edits it, the
/// envelope's "at egress cap" verdict for B2s targets changes with it.
/// </summary>
public sealed class VmNetworkSpecsTests
{
    [Fact]
    public void Lookup_is_case_insensitive_and_whitespace_tolerant()
    {
        var direct = VmNetworkSpecs.Lookup("azure", "standard_b2s");
        var shouty = VmNetworkSpecs.Lookup("AZURE", "  Standard_B2s "); // provisioner casing
        Assert.NotNull(direct);
        Assert.Equal(direct, shouty);
    }

    [Fact]
    public void B2s_stays_calibrated_to_the_live_measurement()
    {
        var spec = VmNetworkSpecs.Lookup("azure", "Standard_B2s");
        Assert.NotNull(spec);
        Assert.Equal(2, spec!.Vcpus);
        Assert.Equal(600, spec.EgressMbps);          // ≈ measured 530–580 Mbps
        Assert.Equal("estimated", spec.Confidence);  // Azure does not guarantee B-series
        Assert.False(spec.AcceleratedNetworking);
    }

    [Fact]
    public void Documented_sizes_carry_documented_confidence()
    {
        var spec = VmNetworkSpecs.Lookup("azure", "Standard_D2s_v5");
        Assert.NotNull(spec);
        Assert.Equal(12500, spec!.EgressMbps);
        Assert.Equal("documented", spec.Confidence);
        Assert.True(spec.AcceleratedNetworking);
    }

    [Theory]
    [InlineData(null, "Standard_B2s")]
    [InlineData("azure", null)]
    [InlineData("azure", "Standard_ZZ99_v9")]  // unknown size → no invented ceiling
    [InlineData("digitalocean", "s-2vcpu-4gb")]
    public void Unknown_or_missing_keys_yield_null(string? cloud, string? size)
    {
        Assert.Null(VmNetworkSpecs.Lookup(cloud, size));
    }

    [Fact]
    public void ToWire_maps_nulls_through_and_specs_to_snake_case_fields()
    {
        Assert.Null(VmNetworkSpecs.ToWire(null));
        Assert.NotNull(VmNetworkSpecs.ToWire(VmNetworkSpecs.Lookup("azure", "standard_b2s")));
    }

    // ── endpoint_ref → deployment id (the /infra target resolution) ──

    [Fact]
    public void Proxy_ref_parses_to_the_deployment_id()
    {
        var ok = TestRunsEndpoints.TryProxyDeploymentId(
            """{"kind":"proxy","proxy_endpoint_id":"5c93744d-edda-4388-82f7-8a4046cd23a4"}""",
            out var id);
        Assert.True(ok);
        Assert.Equal(Guid.Parse("5c93744d-edda-4388-82f7-8a4046cd23a4"), id);
    }

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("not json")]
    [InlineData("""{"kind":"network","host":"example.com"}""")]  // no proxy id
    [InlineData("""{"proxy_endpoint_id":"not-a-guid"}""")]
    [InlineData("""[1,2,3]""")]
    public void Non_proxy_or_malformed_refs_are_false_never_throw(string? endpointRef)
    {
        Assert.False(TestRunsEndpoints.TryProxyDeploymentId(endpointRef, out _));
    }
}
