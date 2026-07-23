using Microsoft.EntityFrameworkCore;
using Networker.Data;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Global (non-project-scoped) read endpoints, mirroring the Rust dashboard's
/// <c>api/zones.rs</c> and <c>api/modes.rs</c>. In the Rust router both are in
/// the <c>protected_flat</c> group (valid JWT required, no project scope), so
/// they use <c>.RequireAuthorization()</c> with no named policy.
/// </summary>
public static class PlatformEndpoints
{
    public static IEndpointRouteBuilder MapPlatformEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/zones — sovereignty/deployment zones ordered by code.
        // Mirrors SovereigntyZone from crates/networker-dashboard/src/db/zones.rs
        // (that DB row omits auto_detect, so we match it here for parity).
        app.MapGet("/api/zones", async (NetworkerDbContext db) =>
        {
            var zones = await db.SovereigntyZones
                .AsNoTracking()
                .OrderBy(z => z.Code)
                .Select(z => new
                {
                    code = z.Code,
                    parent_code = z.ParentCode,
                    name = z.Name,
                    display = z.Display,
                    legal_note = z.LegalNote,
                    compliance_level = z.ComplianceLevel,
                    fallback_zone = z.FallbackZone,
                    requires_approval = z.RequiresApproval,
                    requires_mfa = z.RequiresMfa,
                    status = z.Status,
                    created_at = z.CreatedAt,
                })
                .ToListAsync();

            return Results.Ok(zones);
        })
        .RequireAuthorization();

        // GET /api/modes — supported deployment/probe modes grouped by category.
        // Replicates the static list from Protocol::all_modes() in
        // crates/networker-tester/src/metrics.rs and the group-detail text from
        // api/modes.rs, producing { "groups": [ { label, detail, modes: [...] } ] }.
        // Extended (audit C5): "language_capabilities" carries the per-language
        // protocol/workload matrix (BenchmarkLanguageCapabilities) so the
        // Application Benchmark wizard can gate mode × language combos.
        app.MapGet("/api/modes", () => Results.Ok(BuildModes()))
            .RequireAuthorization();

        return app;
    }

    // Requires = the target capability a mode needs (canonical values in
    // shared/modes.json's `requires` field, guarded by ModesManifestTests):
    // "any" | "networker-endpoint" | "sdk-endpoint" | "reference-apis".
    private record ModeInfo(string Id, string Name, string Description, string Detail, string Group, string Requires);

    // Mirror of Protocol::all_modes() — single source of truth for the mode
    // pickers. Order is preserved so grouping matches the Rust output exactly.
    private static readonly ModeInfo[] AllModes =
    [
        // Network
        new("tcp", "TCP", "Connect", "TCP 3-way handshake timing to measure raw connection latency", "Network", "any"),
        new("dns", "DNS", "Resolve", "DNS resolution timing for the target hostname", "Network", "any"),
        new("tls", "TLS", "Handshake", "TLS handshake via rustls — reports version, cipher, ALPN, cert chain", "Network", "any"),
        new("tlsresume", "TLS Resume", "Warm handshake", "Two fresh TLS handshakes with a real HTTP request; the second should resume", "Network", "any"),
        new("native", "Native TLS", "OS TLS stack", "Uses SChannel (Win), SecureTransport (macOS), or OpenSSL (Linux)", "Network", "any"),
        new("udp", "UDP", "Round-trip", "UDP echo probe — measures RTT, jitter, and packet loss", "Network", "any"),
        // HTTP
        new("http1", "HTTP/1.1", "Single request", "Full HTTP/1.1 request: DNS + TCP + TLS + request/response", "HTTP", "any"),
        new("http2", "HTTP/2", "Multiplexed", "HTTP/2 over TLS with ALPN h2 negotiation", "HTTP", "any"),
        new("http3", "HTTP/3", "QUIC", "HTTP/3 over QUIC (UDP) — 0-RTT capable", "HTTP", "any"),
        new("curl", "Curl", "Via curl CLI", "Spawns curl binary, captures per-phase timing from --write-out", "HTTP", "any"),
        new("sdkprobe", "SDK Probe", "Server split", "Probes a customer-embedded LagHound endpoint — splits total time into DNS, TCP, TLS, network transfer, and server processing via Server-Timing", "HTTP", "sdk-endpoint"),
        // Page Load (Native)
        new("pageload", "H1", "6 parallel connections", "Fetches page manifest + assets using 6 parallel HTTP/1.1 connections (browser-like)", "Page Load (Native)", "any"),
        new("pageload2", "H2", "Multiplexed", "Same assets multiplexed over a single TLS/HTTP2 connection", "Page Load (Native)", "any"),
        new("pageload3", "H3", "QUIC", "Same assets multiplexed over a single QUIC connection", "Page Load (Native)", "any"),
        // Page Load (Browser)
        new("browser1", "H1", "Chrome HTTP/1.1", "Chrome headless with HTTP/2 disabled — forces HTTP/1.1", "Page Load (Browser)", "any"),
        new("browser2", "H2", "Chrome HTTP/2", "Chrome headless with QUIC disabled — forces HTTP/2", "Page Load (Browser)", "any"),
        new("browser3", "H3", "Chrome QUIC", "Chrome headless with QUIC forced via origin flag + SPKI cert pinning", "Page Load (Browser)", "any"),
        // Throughput
        new("download", "Download", "Server→client", "Large payload download via HTTP — measures sustained throughput", "Throughput", "networker-endpoint"),
        new("upload", "Upload", "Client→server", "Large payload upload via HTTP POST — measures sustained throughput", "Throughput", "networker-endpoint"),
        new("download1", "Download H1", "H1 download", "Throughput download forced over HTTP/1.1", "Throughput", "networker-endpoint"),
        new("download2", "Download H2", "H2 download", "Throughput download over HTTP/2 multiplexed stream", "Throughput", "networker-endpoint"),
        new("download3", "Download H3", "H3 download", "Throughput download over QUIC/HTTP3", "Throughput", "networker-endpoint"),
        new("upload1", "Upload H1", "H1 upload", "Throughput upload forced over HTTP/1.1", "Throughput", "networker-endpoint"),
        new("upload2", "Upload H2", "H2 upload", "Throughput upload over HTTP/2", "Throughput", "networker-endpoint"),
        new("upload3", "Upload H3", "H3 upload", "Throughput upload over QUIC/HTTP3", "Throughput", "networker-endpoint"),
        new("webdownload", "Web Download", "HTTP GET", "Download via /download endpoint route", "Throughput", "networker-endpoint"),
        new("webupload", "Web Upload", "HTTP POST", "Upload via /upload endpoint route", "Throughput", "networker-endpoint"),
        new("udpdownload", "UDP Download", "UDP bulk DL", "Bulk download via UDP throughput server (port 9998)", "Throughput", "networker-endpoint"),
        new("udpupload", "UDP Upload", "UDP bulk UL", "Bulk upload via UDP throughput server (port 9998)", "Throughput", "networker-endpoint"),
        // API Workloads — orchestrator-level mode, NOT a tester Protocol
        // variant: the orchestrator expands it into one tester run per
        // workload in benchmarks/configs/apibench.json (API-SPEC.md §4).
        new("apibench", "API Workloads", "Server compute", "Measured /api/* JSON workload suite: api-users, api-transform, api-aggregate, api-search, api-compress (benchmarks/configs/apibench.json). nginx is skipped — it serves no /api/* endpoints.", "API Workloads", "reference-apis"),
    ];

    // Mode id → required target capability, built from AllModes (whose Requires
    // values are guarded byte-for-byte against shared/modes.json's `requires`
    // field by ModesManifestTests). Single source for the server-side
    // mode↔target compatibility gate (ModeTargetCompatibility) — mirrors the
    // frontend requirementOf() in dashboard/src/lib/mode-capabilities.ts.
    private static readonly IReadOnlyDictionary<string, string> ModeRequirements =
        AllModes.ToDictionary(m => m.Id, m => m.Requires, StringComparer.OrdinalIgnoreCase);

    /// <summary>
    /// The target capability a mode needs: <c>"any"</c> | <c>"networker-endpoint"</c>
    /// | <c>"sdk-endpoint"</c> | <c>"reference-apis"</c>. Unknown modes default to
    /// <c>"any"</c>, matching the frontend <c>requirementOf()</c>.
    /// </summary>
    public static string RequirementOf(string mode) =>
        !string.IsNullOrWhiteSpace(mode) && ModeRequirements.TryGetValue(mode, out var r) ? r : "any";

    private static string GroupDetail(string label) => label switch
    {
        "Network" => "Low-level connection probes. Measures DNS resolution, TCP handshake, TLS negotiation, and UDP round-trip independently — isolates each layer of the network stack.",
        "HTTP" => "Full HTTP request timing across protocol versions. Each probe does DNS + TCP + TLS + HTTP request/response and reports TTFB, total duration, and negotiated protocol.",
        "Page Load (Native)" => "Loads a page with multiple assets using the Rust HTTP stack (no browser). Compares H1 (6 parallel connections), H2 (multiplexed), and H3 (QUIC). Fastest, no rendering overhead.",
        "Page Load (Browser)" => "Same page load test but using real Chrome headless. Includes rendering, JavaScript, and browser networking. Measures what users actually experience — DOM loaded, full load, bytes transferred.",
        "Throughput" => "Sustained transfer speed tests with configurable payload sizes. Measures download and upload bandwidth across different HTTP versions and transport protocols.",
        "API Workloads" => "Per-request server computation benchmarks against the spec-measured /api/* endpoints (API-SPEC.md §4): dataset sorting, SHA-256 hashing, percentile aggregation, regex search, and zlib compression. Request shapes are frozen in benchmarks/configs/apibench.json so every language measures identical work.",
        _ => "",
    };

    private static object BuildModes()
    {
        var groups = new List<object>();
        string currentGroup = string.Empty;
        var currentModes = new List<object>();

        void Flush()
        {
            if (currentGroup.Length == 0)
            {
                return;
            }

            groups.Add(new
            {
                label = currentGroup,
                detail = GroupDetail(currentGroup),
                modes = currentModes.ToArray(),
            });
        }

        foreach (var mode in AllModes)
        {
            if (mode.Group != currentGroup)
            {
                Flush();
                currentModes = new List<object>();
                currentGroup = mode.Group;
            }

            currentModes.Add(new
            {
                id = mode.Id,
                name = mode.Name,
                desc = mode.Description,
                detail = mode.Detail,
                requires = mode.Requires,
            });
        }

        Flush();

        var languageCapabilities = BenchmarkLanguageCapabilities.All
            .Select(c => new
            {
                language = c.Language,
                http1 = c.Http1,
                http2 = c.Http2,
                http3 = c.Http3,
                apibench = c.Apibench,
            })
            .ToArray();

        return new { groups, language_capabilities = languageCapabilities };
    }
}
