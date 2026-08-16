using System.Text.Json;
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
    // The legacy Rust table said 443, but the actual Windows deploy
    // (_iis_setup_powershell) binds HTTPS on 8445 — probing 443 failed every
    // IIS matrix cell "never became reachable" (2026-08-03).
    [InlineData("iis", 8445)]
    public void Known_stacks_map_to_their_listener_ports(string stack, int expected)
        => Assert.Equal(expected, RunDispatcher.ProxyHttpsPort(stack));

    [Theory]
    [InlineData("")]
    [InlineData("unknown")]
    [InlineData("envoy")]
    public void Unknown_stacks_default_to_443(string stack)
        => Assert.Equal(443, RunDispatcher.ProxyHttpsPort(stack));

    /// <summary>
    /// Drift guard: the C# table must agree with <c>shared/http-stacks.json</c>,
    /// the canonical layout the Rust tester embeds (its pageload/throughput/
    /// browser HTTPS→HTTP rewrites and <c>--http-stacks</c>) and lab/validate.sh
    /// reads. Every proxy stack in the manifest must resolve to its https_port
    /// here, and every stack this table knows must be in the manifest.
    /// </summary>
    [Fact]
    public void Table_matches_shared_http_stacks_manifest()
    {
        using var doc = JsonDocument.Parse(
            File.ReadAllText(Path.Combine(AppContext.BaseDirectory, "shared", "http-stacks.json")));
        var manifest = doc.RootElement.GetProperty("stacks").EnumerateArray()
            .Select(s => (Id: s.GetProperty("id").GetString()!, Https: s.GetProperty("https_port").GetInt32()))
            .Where(s => s.Id != "endpoint")
            .ToList();
        Assert.NotEmpty(manifest);
        foreach (var (id, https) in manifest)
        {
            Assert.Equal(https, RunDispatcher.ProxyHttpsPort(id));
        }
        // Every stack the C# switch names must be listed (a new arm here needs a
        // manifest row so the tester + lab learn its ports too).
        foreach (var known in new[] { "nginx", "caddy", "traefik", "haproxy", "apache", "iis" })
        {
            Assert.Contains(manifest, s => s.Id == known);
        }
    }

    [Fact]
    public void Table_is_case_sensitive_matching_the_rust_match_arms()
    {
        // Rust matches on the exact lowercase stack string; anything else falls
        // through to the 443 default.
        Assert.Equal(443, RunDispatcher.ProxyHttpsPort("NGINX"));
    }
}
