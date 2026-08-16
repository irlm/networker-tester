using System.Text.Json;
using Networker.ControlPlane.Dispatch;
using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Drift guard: <c>shared/http-stacks.json</c> ⇄ <see cref="HttpStackCatalog"/>
/// (the embedded copy the control plane serves and gates on) ⇄ the modes
/// manifest ⇄ the dispatcher's port table. The same file is embedded by the
/// Rust tester (<c>http_stacks.rs</c>), mirrored by the dashboard
/// (<c>lib/http-stacks.ts</c>, guarded by <c>http-stacks-manifest.test.ts</c>)
/// and read by <c>lab/validate.sh</c>; this class keeps the C# side honest.
/// </summary>
public class HttpStacksManifestTests
{
    private static string RepoCopy() =>
        File.ReadAllText(Path.Combine(AppContext.BaseDirectory, "shared", "http-stacks.json"));

    private static JsonDocument ModesManifest() => JsonDocument.Parse(
        File.ReadAllText(Path.Combine(AppContext.BaseDirectory, "shared", "modes.json")));

    [Fact]
    public void Embedded_copy_matches_the_repo_file()
    {
        var repo = HttpStackCatalog.Parse(RepoCopy());
        Assert.Equal(repo.H3Modes, HttpStackCatalog.H3Modes);
        Assert.Equal(repo.Stacks, HttpStackCatalog.Stacks);
    }

    [Fact]
    public void Manifest_lists_every_installer_stack_with_the_lab_measured_h3_capability()
    {
        // nginx (mainline+quic), caddy (h1 h2 h3), iis (http.sys) and the bare
        // endpoint serve HTTP/3; traefik / haproxy / apache are configured
        // h1/h2 only (measured 0/N on http3/pageload3 in the lab).
        Assert.True(HttpStackCatalog.HasH3("endpoint"));
        Assert.True(HttpStackCatalog.HasH3("nginx"));
        Assert.True(HttpStackCatalog.HasH3("caddy"));
        Assert.True(HttpStackCatalog.HasH3("iis"));
        Assert.False(HttpStackCatalog.HasH3("traefik"));
        Assert.False(HttpStackCatalog.HasH3("haproxy"));
        Assert.False(HttpStackCatalog.HasH3("apache"));
        // Unknown stacks are null so callers fail open, never fabricate.
        Assert.Null(HttpStackCatalog.HasH3("envoy"));
        Assert.Null(HttpStackCatalog.HasH3(""));
        Assert.Null(HttpStackCatalog.HasH3(null));
        // Case-insensitive like the Rust by_name().
        Assert.True(HttpStackCatalog.HasH3("NGINX"));
    }

    [Fact]
    public void H3_modes_are_real_catalog_modes_and_cover_every_quic_probe()
    {
        using var modes = ModesManifest();
        var ids = modes.RootElement.GetProperty("modes").EnumerateArray()
            .Select(m => m.GetProperty("id").GetString()!)
            .ToHashSet(StringComparer.Ordinal);
        foreach (var m in HttpStackCatalog.H3Modes)
        {
            Assert.Contains(m, ids);
        }
        // Every mode whose catalog text says QUIC / HTTP/3 must be listed — a new
        // h3 probe added to modes.json without an h3_modes entry would slip
        // through the gate.
        foreach (var m in modes.RootElement.GetProperty("modes").EnumerateArray())
        {
            var detail = m.TryGetProperty("detail", out var d) ? d.GetString() ?? "" : "";
            var desc = m.TryGetProperty("description", out var s) ? s.GetString() ?? "" : "";
            var quic = detail.Contains("QUIC", StringComparison.OrdinalIgnoreCase)
                       || detail.Contains("HTTP/3", StringComparison.OrdinalIgnoreCase)
                       || desc.Contains("QUIC", StringComparison.OrdinalIgnoreCase);
            // browser2 mentions "QUIC disabled" — that is the H2 probe.
            var id = m.GetProperty("id").GetString()!;
            if (quic && !detail.Contains("QUIC disabled", StringComparison.OrdinalIgnoreCase))
            {
                Assert.True(HttpStackCatalog.IsH3Mode(id), $"{id} needs HTTP/3 but is not in h3_modes");
            }
        }
        Assert.True(HttpStackCatalog.IsH3Mode("http3"));
        Assert.True(HttpStackCatalog.IsH3Mode("pageload3"));
        Assert.True(HttpStackCatalog.IsH3Mode("browser3"));
        Assert.False(HttpStackCatalog.IsH3Mode("http2"));
        Assert.False(HttpStackCatalog.IsH3Mode("browser2"));
        Assert.False(HttpStackCatalog.IsH3Mode(null));
    }

    [Fact]
    public void Ports_agree_with_the_dispatcher_table()
    {
        foreach (var s in HttpStackCatalog.Stacks.Where(s => s.Id != "endpoint"))
        {
            Assert.Equal(s.HttpsPort, RunDispatcher.ProxyHttpsPort(s.Id));
        }
    }

    [Fact]
    public void Wire_shape_carries_h3_modes_and_stacks()
    {
        var json = JsonSerializer.Serialize(HttpStackCatalog.ToWire());
        using var doc = JsonDocument.Parse(json);
        var h3 = doc.RootElement.GetProperty("h3_modes").EnumerateArray().Select(e => e.GetString()).ToList();
        Assert.Contains("http3", h3);
        var apache = doc.RootElement.GetProperty("stacks").EnumerateArray()
            .Single(s => s.GetProperty("id").GetString() == "apache");
        Assert.False(apache.GetProperty("h3").GetBoolean());
        Assert.Equal(8457, apache.GetProperty("https_port").GetInt32());
    }

    [Theory]
    [InlineData("""{"stacks":[]}""")]
    [InlineData("""{"h3_modes":["http3"],"stacks":[]}""")]
    [InlineData("""{"h3_modes":[],"stacks":[{"id":"nginx","http_port":1,"https_port":2,"h3":true}]}""")]
    [InlineData("""{"h3_modes":["http3"],"stacks":[{"id":"nginx","http_port":1,"https_port":2,"h3":true},{"id":"NGINX","http_port":3,"https_port":4,"h3":false}]}""")]
    public void Parse_rejects_malformed_manifests(string json)
        => Assert.Throws<InvalidOperationException>(() => HttpStackCatalog.Parse(json));
}
