using System.Text.Json;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Per-target test-capability resolution ("the target must return the tests
/// supported", 2026-08-13). The endpoint's <c>/health</c> self-reports a
/// <c>services</c> map — LIVE truth about which listeners this instance runs
/// (stamp/udp can be port-0-disabled, h3 is optional) — and this class maps
/// that report onto the probe-mode vocabulary so launch flows offer exactly
/// what the target supports instead of inferring from static config.
///
/// <para>Pre-0.28.202 endpoints return a /health WITHOUT <c>services</c>;
/// they are reported with <c>services: null</c> and the caller falls back to
/// config-derived support (never a fabricated capability list).</para>
/// </summary>
public static class TargetCapabilities
{
    /// <summary>Modes every reachable networker-endpoint supports via its
    /// always-on HTTP routes (no optional listener involved).</summary>
    private static readonly string[] HttpRouteModes =
    [
        "download", "upload",
        "download1", "download2", "download3",
        "upload1", "upload2", "upload3",
        "webdownload", "webupload",
        "mthroughput", "rpm", "responsiveness",
    ];

    /// <summary>
    /// Map a <c>services</c> self-report to the endpoint-required probe modes
    /// it supports and, for the rest, the reason they are off. Universal
    /// (any-target) modes are not listed — they are always allowed.
    /// </summary>
    public static (List<string> Supported, List<KeyValuePair<string, string>> Unsupported)
        MapServices(JsonElement services)
    {
        var supported = new List<string>();
        var unsupported = new List<KeyValuePair<string, string>>();

        bool Has(string key) =>
            services.TryGetProperty(key, out var v)
            && v.ValueKind is not (JsonValueKind.Null or JsonValueKind.False or JsonValueKind.Undefined);

        void Gate(string reason, bool on, params string[] modes)
        {
            if (on)
            {
                supported.AddRange(modes);
            }
            else
            {
                foreach (var m in modes)
                {
                    unsupported.Add(new(m, reason));
                }
            }
        }

        Gate("endpoint download/upload routes not reported", Has("download") && Has("upload"), HttpRouteModes);
        Gate("UDP echo listener disabled on this target", Has("udp_echo"), "udp");
        Gate("UDP throughput listener disabled on this target", Has("udp_throughput"), "udpdownload", "udpupload");
        Gate("STAMP reflector disabled on this target", Has("stamp"), "stamp");
        Gate("WebSocket echo route not reported", Has("ws_echo"), "websocket");
        Gate("synthetic page-asset route not reported", Has("page_assets"), "pageload", "pageload2", "pageload3");

        return (supported, unsupported);
    }

    /// <summary>
    /// Probe one deployment host's /health (HTTPS :8443 then HTTP :8080,
    /// concurrently under the shared budget, self-signed accepted — the
    /// <see cref="VersionEndpoints.ProbeEndpointVersionAsync"/> posture) and
    /// return its capability report. Never throws.
    /// </summary>
    public static async Task<object> ProbeHostAsync(string host)
    {
        using var handler = new HttpClientHandler
        {
            ServerCertificateCustomValidationCallback =
                HttpClientHandler.DangerousAcceptAnyServerCertificateValidator,
        };
        using var client = new HttpClient(handler)
        {
            Timeout = TimeSpan.FromMilliseconds(1500),
        };

        var urls = new[] { $"https://{host}:8443/health", $"http://{host}:8080/health" };
        var bodies = await Task.WhenAll(urls.Select(u => FetchOneAsync(client, u))).ConfigureAwait(false);
        var body = Array.Find(bodies, b => b is not null);
        if (body is null)
        {
            return new { host, reachable = false, version = (string?)null, services = (object?)null, supported_modes = (object?)null, unsupported_modes = (object?)null };
        }

        using var doc = JsonDocument.Parse(body);
        var root = doc.RootElement;
        var version = root.TryGetProperty("version", out var v) && v.ValueKind == JsonValueKind.String
            ? v.GetString()
            : null;

        if (!root.TryGetProperty("services", out var services) || services.ValueKind != JsonValueKind.Object)
        {
            // Pre-0.28.202 endpoint: reachable, but it cannot self-report yet.
            return new { host, reachable = true, version, services = (object?)null, supported_modes = (object?)null, unsupported_modes = (object?)null };
        }

        var (supported, unsupported) = MapServices(services);
        return new
        {
            host,
            reachable = true,
            version,
            services = services.Clone(),
            supported_modes = supported,
            unsupported_modes = unsupported.Select(kv => new { mode = kv.Key, reason = kv.Value }).ToList(),
        };
    }

    private static async Task<string?> FetchOneAsync(HttpClient client, string url)
    {
        try
        {
            using var resp = await client.GetAsync(url).ConfigureAwait(false);
            if (!resp.IsSuccessStatusCode)
            {
                return null;
            }
            return await resp.Content.ReadAsStringAsync().ConfigureAwait(false);
        }
        catch (Exception)
        {
            return null;
        }
    }
}
