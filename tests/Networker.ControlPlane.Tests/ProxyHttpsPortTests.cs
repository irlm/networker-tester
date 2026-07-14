using Networker.ControlPlane.Dispatch;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Unit tests for the proxy-stack → HTTPS listener port table the dispatcher
/// uses when resolving <c>Proxy</c> endpoints to <c>Network{host,port}</c>
/// (the v0.28.10 prod fix ported from Rust
/// <c>networker_common::test_config::proxy_https_port</c>). Exercised through
/// <see cref="RunDispatcher.ProxyHttpsPort"/>, which delegates to the single
/// shared table in ProvisioningOrchestrator — so both call sites are covered.
/// </summary>
public class ProxyHttpsPortTests
{
    [Theory]
    [InlineData("nginx", 8444)]
    [InlineData("caddy", 8454)]
    [InlineData("traefik", 8455)]
    [InlineData("haproxy", 8456)]
    [InlineData("apache", 8457)]
    [InlineData("iis", 443)]
    public void Known_stacks_map_to_their_listener_ports(string stack, int expected)
        => Assert.Equal(expected, RunDispatcher.ProxyHttpsPort(stack));

    [Theory]
    [InlineData("")]
    [InlineData("unknown")]
    [InlineData("envoy")]
    public void Unknown_stacks_default_to_443(string stack)
        => Assert.Equal(443, RunDispatcher.ProxyHttpsPort(stack));

    [Fact]
    public void Table_is_case_sensitive_matching_the_rust_match_arms()
    {
        // Rust matches on the exact lowercase stack string; anything else falls
        // through to the 443 default.
        Assert.Equal(443, RunDispatcher.ProxyHttpsPort("NGINX"));
    }
}
