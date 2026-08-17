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
    [InlineData("udp", "networker-endpoint")]
    [InlineData("pageload3", "networker-endpoint")]
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
    public void Network_url_rejects_endpoint_only_throughput_sdkprobe_and_apibench()
    {
        // These can only fail against a raw URL — the exact case the gate exists
        // for ("we can call tests that will fail every time"). udp + pageload*
        // joined the list 2026-08-12 after a live Full diagnostic proved they
        // fail by construction on real websites (echo :9999 / synthetic /asset
        // ladder are endpoint-only).
        foreach (var mode in new[] { "download", "upload", "udpdownload", "sdkprobe", "apibench", "udp", "pageload", "pageload2", "pageload3" })
        {
            var bad = ModeTargetCompatibility.IncompatibleModes([mode], "network");
            Assert.Single(bad);
            Assert.Equal(mode, bad[0].Mode);
        }
    }

    [Fact]
    public void Network_url_allows_primitives_and_browser_modes()
    {
        // URL Diagnostics runs these against arbitrary URLs; browser* load the
        // real page (unlike pageload*, which fetch the endpoint asset ladder).
        var modes = new[] { "dns", "tcp", "tls", "http1", "http2", "http3", "curl", "browser1", "browser3", "ping", "path", "dualstack", "pmtud" };
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

    // ── Rule 2: HTTP/3 by proxy stack (shared/http-stacks.json h3) ───────────

    private static readonly string[] H3Modes = ["http3", "pageload3", "browser3", "download3", "upload3"];

    [Theory]
    [InlineData("nginx")]
    [InlineData("caddy")]
    [InlineData("endpoint")]
    [InlineData("iis")] // SNI hostname binding + hostname dispatch (V050) — lab-measured h3 2/2 through IIS
    public void Stacks_with_quic_allow_the_h3_modes(string stack)
    {
        Assert.Empty(ModeTargetCompatibility.IncompatibleModes(H3Modes, "proxy", stack));
        Assert.Empty(ModeTargetCompatibility.IncompatibleModes(H3Modes, "pending", stack));
    }

    [Theory]
    [InlineData("apache")]
    [InlineData("haproxy")]
    [InlineData("traefik")]
    public void Stacks_without_quic_reject_exactly_the_h3_modes(string stack)
    {
        // proxy kind: h3 modes rejected with the stack named in the reason; the
        // H1/H2 siblings and everything else stay allowed.
        var modes = new[] { "http1", "http2", "http3", "pageload", "pageload2", "pageload3", "browser1", "browser2", "browser3", "download", "download3", "upload3", "tcp" };
        var bad = ModeTargetCompatibility.IncompatibleModes(modes, "proxy", stack);
        Assert.Equal(H3Modes.OrderBy(m => m), bad.Select(b => b.Mode).OrderBy(m => m));
        Assert.All(bad, b =>
        {
            Assert.Equal(ModeTargetCompatibility.H3Requirement, b.Requirement);
            Assert.Contains($"{stack} has no HTTP/3", b.Reason);
            Assert.Contains("shared/http-stacks.json", b.Reason);
        });
    }

    [Fact]
    public void Pending_with_a_quic_less_proxy_stack_rejects_h3_but_still_fails_open_on_kind()
    {
        // The pending kind stays fail-open for the kind rule (apibench / sdkprobe /
        // throughput all pass), but the stack rule IS decidable and applies.
        var bad = ModeTargetCompatibility.IncompatibleModes(
            ["apibench", "sdkprobe", "download", "http1", "http3", "pageload3"], "pending", "apache");
        Assert.Equal(["http3", "pageload3"], bad.Select(b => b.Mode).ToArray());
    }

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("envoy")]
    [InlineData("lighttpd")]
    public void Unknown_or_absent_stack_fails_open_on_h3(string? stack)
    {
        Assert.Empty(ModeTargetCompatibility.IncompatibleModes(H3Modes, "proxy", stack));
        Assert.Empty(ModeTargetCompatibility.IncompatibleModes(H3Modes, "pending", stack));
        // A raw URL never has a stack — http3 against a URL is fine (rule 1 only).
        Assert.Empty(ModeTargetCompatibility.IncompatibleModes(["http3", "browser3"], "network", stack));
    }

    [Fact]
    public void Kind_rule_wins_when_both_apply_and_each_mode_is_reported_once()
    {
        // download3 against a raw URL is rejected for needing an endpoint (rule 1);
        // even with a quic-less stack hint it must appear exactly once.
        var bad = ModeTargetCompatibility.IncompatibleModes(["download3"], "network", "apache");
        Assert.Single(bad);
        Assert.Equal("networker-endpoint", bad[0].Requirement);
    }

    [Fact]
    public void SplitH3ModesForStack_drops_only_h3_modes_on_quic_less_stacks()
    {
        var (kept, dropped) = ModeTargetCompatibility.SplitH3ModesForStack(
            ["http1", "http2", "http3", "pageload3", "download"], "apache");
        Assert.Equal(["http1", "http2", "download"], kept);
        Assert.Equal(["http3", "pageload3"], dropped);

        var (keptNginx, droppedNginx) = ModeTargetCompatibility.SplitH3ModesForStack(
            ["http1", "http3"], "nginx");
        Assert.Equal(["http1", "http3"], keptNginx);
        Assert.Empty(droppedNginx);

        var (keptUnknown, droppedUnknown) = ModeTargetCompatibility.SplitH3ModesForStack(["http3"], null);
        Assert.Equal(["http3"], keptUnknown);
        Assert.Empty(droppedUnknown);
    }
}
