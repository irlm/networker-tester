using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Phase 2 capability enforcement: the server-side mode↔target gate
/// (<see cref="ModeTargetCompatibility"/>) must reject exactly the (mode,
/// endpoint.kind) combinations the frontend disables (mode-capabilities.ts) and
/// — critically — must NOT reject the legitimate flows: throughput on
/// proxy/pending, apibench on pending (Application Benchmark), sdkprobe on
/// runtime. `pending` fails open because its real capability is decided by the
/// wizard + language matrix, not by kind.
/// </summary>
public class ModeTargetCompatibilityTests
{
    // ── RequirementOf (single-sourced from AllModes ⇄ shared/modes.json) ──────

    [Theory]
    [InlineData("tcp", "any")]
    [InlineData("dns", "any")]
    [InlineData("http3", "any")]
    [InlineData("udp", "any")]
    [InlineData("pageload3", "any")]
    [InlineData("browser2", "any")]
    [InlineData("download", "networker-endpoint")]
    [InlineData("upload3", "networker-endpoint")]
    [InlineData("udpdownload", "networker-endpoint")]
    [InlineData("sdkprobe", "sdk-endpoint")]
    [InlineData("apibench", "reference-apis")]
    public void RequirementOf_matches_manifest(string mode, string requires)
    {
        Assert.Equal(requires, PlatformEndpoints.RequirementOf(mode));
    }

    [Theory]
    [InlineData("DOWNLOAD", "networker-endpoint")] // case-insensitive
    [InlineData("totally-made-up", "any")] //          unknown → any
    [InlineData("", "any")] //                          blank → any
    public void RequirementOf_is_case_insensitive_and_defaults_to_any(string mode, string requires)
    {
        Assert.Equal(requires, PlatformEndpoints.RequirementOf(mode));
    }

    // ── endpoint.kind → TargetKind ────────────────────────────────────────────

    [Theory]
    [InlineData("network", "url")]
    [InlineData("proxy", "endpoint")]
    [InlineData("runtime", "sdk")]
    public void TargetKindFor_maps_the_resolvable_kinds(string kind, string target)
    {
        Assert.Equal(target, ModeTargetCompatibility.TargetKindFor(kind));
    }

    [Theory]
    [InlineData("pending")] // provisioning request — capability decided later
    [InlineData("bogus")]
    [InlineData(null)]
    public void TargetKindFor_fails_open_for_pending_and_unknown(string? kind)
    {
        Assert.Null(ModeTargetCompatibility.TargetKindFor(kind));
    }

    // ── The gate: what gets rejected (defense-in-depth) ───────────────────────

    [Fact]
    public void Network_url_rejects_throughput_sdkprobe_and_apibench()
    {
        // These can only fail against a raw URL — the exact case the gate exists
        // for ("we can call tests that will fail every time").
        foreach (var mode in new[] { "download", "upload", "udpdownload", "sdkprobe", "apibench" })
        {
            var bad = ModeTargetCompatibility.IncompatibleModes([mode], "network");
            Assert.Single(bad);
            Assert.Equal(mode, bad[0].Mode);
        }
    }

    [Fact]
    public void Network_url_allows_any_modes_including_udp_pageload_browser()
    {
        // URL Diagnostics runs all of these against arbitrary URLs.
        var modes = new[] { "dns", "tcp", "tls", "http1", "http2", "http3", "curl", "udp", "pageload", "pageload3", "browser1", "browser3" };
        Assert.Empty(ModeTargetCompatibility.IncompatibleModes(modes, "network"));
    }

    [Fact]
    public void Proxy_endpoint_allows_throughput_but_rejects_sdkprobe_and_apibench()
    {
        // Network Test (proxy) legitimately runs throughput; it never offers
        // sdkprobe/apibench, but a direct API caller could — and those fail.
        Assert.Empty(ModeTargetCompatibility.IncompatibleModes(
            ["tcp", "http2", "download", "upload", "pageload"], "proxy"));

        Assert.Single(ModeTargetCompatibility.IncompatibleModes(["sdkprobe"], "proxy"));
        Assert.Single(ModeTargetCompatibility.IncompatibleModes(["apibench"], "proxy"));
    }

    [Fact]
    public void Runtime_sdk_allows_sdkprobe_and_throughput_but_rejects_apibench()
    {
        Assert.Empty(ModeTargetCompatibility.IncompatibleModes(
            ["sdkprobe", "http1", "download"], "runtime"));

        Assert.Single(ModeTargetCompatibility.IncompatibleModes(["apibench"], "runtime"));
    }

    [Fact]
    public void Pending_fails_open_for_every_flow_it_carries()
    {
        // CRITICAL: `pending` is used by BOTH Full Stack (throughput) AND
        // Application Benchmark (apibench). The gate must never reject it, or it
        // would break valid Application Benchmark config creation.
        Assert.Empty(ModeTargetCompatibility.IncompatibleModes(["apibench", "http1"], "pending"));
        Assert.Empty(ModeTargetCompatibility.IncompatibleModes(["download", "upload"], "pending"));
        Assert.Empty(ModeTargetCompatibility.IncompatibleModes(["sdkprobe"], "pending"));
    }

    [Fact]
    public void Reports_every_incompatible_mode_not_just_the_first()
    {
        var bad = ModeTargetCompatibility.IncompatibleModes(
            ["tcp", "download", "http2", "apibench"], "network");
        Assert.Equal(2, bad.Count);
        Assert.Contains(bad, x => x.Mode == "download");
        Assert.Contains(bad, x => x.Mode == "apibench");
    }

    [Fact]
    public void Empty_or_blank_modes_are_ignored()
    {
        Assert.Empty(ModeTargetCompatibility.IncompatibleModes([], "network"));
        Assert.Empty(ModeTargetCompatibility.IncompatibleModes(["", "  "], "network"));
    }
}
