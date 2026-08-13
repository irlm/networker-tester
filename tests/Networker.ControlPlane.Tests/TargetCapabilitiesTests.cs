using System.Text.Json;
using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pins the services→modes mapping behind the per-target capability report
/// (v0.28.202): a target's /health self-report decides exactly which
/// endpoint-required probe modes a launch flow may offer. Disabled listeners
/// (port-0 stamp/udp) must surface as unsupported WITH a reason, and the
/// always-on HTTP routes carry the whole throughput/rpm family.
/// </summary>
public class TargetCapabilitiesTests
{
    private static JsonElement Services(string json) =>
        JsonDocument.Parse(json).RootElement.Clone();

    [Fact]
    public void full_service_report_supports_everything()
    {
        var (supported, unsupported) = TargetCapabilities.MapServices(Services("""
            { "download": true, "upload": true, "ws_echo": true, "page_assets": true,
              "udp_echo": 9999, "udp_throughput": 9998, "stamp": 9997, "h3": 8443 }
            """));

        Assert.Empty(unsupported);
        Assert.Contains("download", supported);
        Assert.Contains("mthroughput", supported);
        Assert.Contains("rpm", supported);
        Assert.Contains("udp", supported);
        Assert.Contains("udpdownload", supported);
        Assert.Contains("stamp", supported);
        Assert.Contains("websocket", supported);
        Assert.Contains("pageload3", supported);
    }

    [Fact]
    public void disabled_stamp_and_udp_surface_as_unsupported_with_reasons()
    {
        // Port-0-disabled listeners self-report null (v0.28.170 semantics).
        var (supported, unsupported) = TargetCapabilities.MapServices(Services("""
            { "download": true, "upload": true, "ws_echo": true, "page_assets": true,
              "udp_echo": null, "udp_throughput": null, "stamp": null, "h3": null }
            """));

        Assert.DoesNotContain("stamp", supported);
        Assert.DoesNotContain("udp", supported);
        Assert.DoesNotContain("udpupload", supported);
        Assert.Contains(unsupported, kv => kv.Key == "stamp" && kv.Value.Contains("disabled"));
        Assert.Contains(unsupported, kv => kv.Key == "udp" && kv.Value.Contains("disabled"));
        Assert.Contains(unsupported, kv => kv.Key == "udpdownload");
        // The HTTP-route family is unaffected.
        Assert.Contains("download", supported);
        Assert.Contains("websocket", supported);
    }

    [Fact]
    public void missing_keys_read_as_unsupported_never_as_supported()
    {
        // A future endpoint that drops a key (or a partial report) must fail
        // CLOSED for that mode — absence is not capability.
        var (supported, unsupported) = TargetCapabilities.MapServices(Services("""
            { "download": true, "upload": true }
            """));

        Assert.Contains("download", supported);
        Assert.DoesNotContain("udp", supported);
        Assert.DoesNotContain("websocket", supported);
        Assert.DoesNotContain("pageload", supported);
        Assert.Contains(unsupported, kv => kv.Key == "websocket");
        Assert.Contains(unsupported, kv => kv.Key == "pageload2");
    }
}
