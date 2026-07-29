using Networker.ControlPlane.Provisioning;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Unit tests for the readiness-gate endpoint parser
/// (<see cref="ProvisioningOrchestrator.TryParseNetworkHostPort"/>) added for the
/// E2E-pass 2026-07-28 P1-3 fix. The TCP-connect gate itself is exercised live
/// (a provisioned endpoint's proxy port), but the shared-config promote path
/// must recover host+port from an already-rewritten <c>Network</c> endpoint_ref
/// to gate the second run — these lock that extraction (and its
/// behaviour-preserving fallback for a non-Network / malformed ref).
/// </summary>
public class ReadinessGateParseTests
{
    [Fact]
    public void Parses_host_and_port_from_a_rewritten_network_endpoint()
    {
        var ok = ProvisioningOrchestrator.TryParseNetworkHostPort(
            """{"kind":"network","host":"20.1.2.3","port":8444}""",
            out var host, out var port);

        Assert.True(ok);
        Assert.Equal("20.1.2.3", host);
        Assert.Equal(8444, port);
    }

    [Fact]
    public void Trims_whitespace_around_host()
    {
        var ok = ProvisioningOrchestrator.TryParseNetworkHostPort(
            """{"kind":"network","host":"  ep.example.com  ","port":443}""",
            out var host, out var port);

        Assert.True(ok);
        Assert.Equal("ep.example.com", host);
        Assert.Equal(443, port);
    }

    [Theory]
    // Still Pending (not yet promoted) — no host/port to gate on.
    [InlineData("""{"kind":"pending","proxy_stack":"nginx"}""")]
    // Network but missing port → can't gate; caller falls back to immediate re-queue.
    [InlineData("""{"kind":"network","host":"20.1.2.3"}""")]
    // Network but missing host.
    [InlineData("""{"kind":"network","port":8444}""")]
    // port present but zero/negative is not a real listener.
    [InlineData("""{"kind":"network","host":"20.1.2.3","port":0}""")]
    // Non-object / malformed.
    [InlineData("""["network"]""")]
    [InlineData("not json")]
    public void Returns_false_for_non_network_or_incomplete_refs(string endpointRef)
    {
        var ok = ProvisioningOrchestrator.TryParseNetworkHostPort(endpointRef, out var host, out var port);

        Assert.False(ok);
        // out-params are always initialised so a false return is safe to ignore.
        Assert.True(string.IsNullOrEmpty(host) || port <= 0);
    }
}
