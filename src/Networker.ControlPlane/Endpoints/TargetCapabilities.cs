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
    /// The proxy stacks a deployment installed — <c>config.endpoints[0].http_stacks</c>
    /// (a wizard-authored deploy.json). Empty when the config carries none
    /// (bare endpoint / ssh / lan targets). The first entry is the listener a
    /// <c>proxy</c> config is dispatched to.
    /// </summary>
    public static IReadOnlyList<string> StacksOf(string? deploymentConfig)
    {
        if (string.IsNullOrWhiteSpace(deploymentConfig))
        {
            return [];
        }
        try
        {
            using var doc = JsonDocument.Parse(deploymentConfig);
            if (doc.RootElement.TryGetProperty("endpoints", out var eps)
                && eps.ValueKind == JsonValueKind.Array
                && eps.GetArrayLength() > 0
                && eps[0].TryGetProperty("http_stacks", out var stacks)
                && stacks.ValueKind == JsonValueKind.Array)
            {
                return stacks.EnumerateArray()
                    .Where(s => s.ValueKind == JsonValueKind.String)
                    .Select(s => s.GetString()!.Trim())
                    .Where(s => s.Length > 0)
                    .ToList();
            }
        }
        catch (JsonException)
        {
            // fall through
        }
        return [];
    }

    /// <summary>
    /// One host's capability report — the typed form of the wire object the
    /// deployment <c>/capabilities</c> route returns (<see cref="ToWire"/>) and
    /// what <see cref="LiveCapabilityCache"/> stores for the config-create
    /// gate. <see cref="Unsupported"/> is null when the host is unreachable or
    /// runs a pre-0.28.202 endpoint (no <c>services</c> self-report) — "no
    /// knowledge", never "everything supported".
    /// </summary>
    public sealed record HostCapabilityReport(
        string Host,
        bool Reachable,
        string? Version,
        JsonElement? Services,
        IReadOnlyList<string>? Supported,
        IReadOnlyList<KeyValuePair<string, string>>? Unsupported)
    {
        /// <summary>True when the host self-reported its listeners (the live
        /// gate may narrow modes); false = unreachable or too old to report.</summary>
        public bool HasReport => Unsupported is not null;

        /// <summary>mode → reason for the modes this host reports off; empty when
        /// there is no report.</summary>
        public IReadOnlyDictionary<string, string> UnsupportedMap()
        {
            var map = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
            if (Unsupported is not null)
            {
                foreach (var kv in Unsupported)
                {
                    map.TryAdd(kv.Key, kv.Value);
                }
            }
            return map;
        }

        /// <summary>The snake_case wire shape (<c>host / reachable / version /
        /// services / supported_modes / unsupported_modes</c>).</summary>
        public object ToWire() => new
        {
            host = Host,
            reachable = Reachable,
            version = Version,
            services = Services is { } s ? (object?)s : null,
            supported_modes = Supported,
            unsupported_modes = Unsupported?.Select(kv => new { mode = kv.Key, reason = kv.Value }).ToList(),
        };
    }

    /// <summary>
    /// Probe one deployment host's /health (HTTPS :8443 then HTTP :8080,
    /// concurrently under the shared budget, self-signed accepted — the
    /// <see cref="VersionEndpoints.ProbeEndpointVersionAsync"/> posture) and
    /// return its capability report. Never throws; a dead host resolves in
    /// ~1.5s as <c>reachable: false</c>.
    /// </summary>
    public static async Task<HostCapabilityReport> ProbeHostAsync(string host)
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
            return new HostCapabilityReport(host, false, null, null, null, null);
        }

        return ParseHealth(host, body);
    }

    /// <summary>Map a /health body onto a <see cref="HostCapabilityReport"/>
    /// (reachable = true). A body without <c>services</c> (pre-0.28.202) yields
    /// a report with no capability knowledge; a malformed body likewise.</summary>
    public static HostCapabilityReport ParseHealth(string host, string body)
    {
        try
        {
            using var doc = JsonDocument.Parse(body);
            var root = doc.RootElement;
            var version = root.TryGetProperty("version", out var v) && v.ValueKind == JsonValueKind.String
                ? v.GetString()
                : null;

            if (!root.TryGetProperty("services", out var services) || services.ValueKind != JsonValueKind.Object)
            {
                // Pre-0.28.202 endpoint: reachable, but it cannot self-report yet.
                return new HostCapabilityReport(host, true, version, null, null, null);
            }

            var (supported, unsupported) = MapServices(services);
            return new HostCapabilityReport(host, true, version, services.Clone(), supported, unsupported);
        }
        catch (JsonException)
        {
            return new HostCapabilityReport(host, true, null, null, null, null);
        }
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
