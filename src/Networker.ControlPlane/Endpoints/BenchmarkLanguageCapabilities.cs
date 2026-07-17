namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Per-language protocol/workload capability matrix for the benchmark
/// reference implementations (audit C5 / P1#14) — the single source of truth
/// consumed by <c>GET /api/modes</c> and the Application Benchmark wizard.
///
/// <para>Every entry was verified against the implementation source, not the
/// audit snapshot:</para>
/// <list type="bullet">
///   <item><b>http2/http3</b> mean the server negotiates the protocol
///   <i>directly</i> (direct-mode benchmarks). Proxy-fronted topologies can
///   still run h2/h3 modes against any backend — the proxy terminates.</item>
///   <item><b>apibench</b> means the server implements the measured
///   <c>/api/*</c> suite frozen in <c>benchmarks/shared/API-SPEC.md</c> §4-5
///   (request shapes in <c>benchmarks/configs/apibench.json</c>).</item>
/// </list>
///
/// <para>Evidence per row (file references relative to
/// <c>benchmarks/reference-apis/</c> unless noted):</para>
/// <list type="bullet">
///   <item><c>rust</c>: networker-endpoint — axum h1/h2 + quinn h3 feature
///   (crates/networker-endpoint), full /api/* (routes.rs).</item>
///   <item><c>go</c>: net/http TLS auto-h2 + quic-go http3.Server
///   (go/main.go).</item>
///   <item><c>cpp</c>: Boost.Beast is HTTP/1.1-only (cpp/server.cpp).</item>
///   <item><c>nodejs</c>: http2.createSecureServer with allowHTTP1; no h3
///   runtime (nodejs/server.js).</item>
///   <item><c>python</c>: uvicorn — HTTP/1.1 only, no h2, no h3
///   (python/Dockerfile, server.py).</item>
///   <item><c>ruby</c>: puma is HTTP/1.1-only (ruby/puma.rb).</item>
///   <item><c>php</c>: Swoole HTTP server without open_http2_protocol
///   (php/server.php).</item>
///   <item><c>java</c>: com.sun.net.httpserver is HTTP/1.1-only
///   (java/Server.java) — audit C5's "Java h1-only".</item>
///   <item><c>nginx</c>: `http2 on` + `listen 8443 quic` (nginx/nginx.conf),
///   but transport-only: /api/* returns 501 (nginx/README.md) — never
///   apibench-eligible.</item>
///   <item><c>csharp-net48</c>: HttpListener, HTTP/1.1 only, Windows-only
///   (csharp-net48/Server.cs).</item>
///   <item><c>csharp-net6</c>: Kestrel Http1AndHttp2 — h3 gated on
///   NET7_0_OR_GREATER in the shared template (csharp-template/Program.cs).</item>
///   <item><c>csharp-net7</c>+ (incl. -aot): Kestrel Http1AndHttp2AndHttp3
///   (csharp-template/Program.cs; variants generated from the template).</item>
/// </list>
/// </summary>
public static class BenchmarkLanguageCapabilities
{
    public sealed record LanguageCapability(
        string Language, bool Http1, bool Http2, bool Http3, bool Apibench);

    /// <summary>
    /// All known benchmark languages, including csharp-net6/net7 (absent from
    /// the wizard catalog but still detectable on a VM by the SSH probe's
    /// csharp-net* sweep — audit C4). Order matches the wizard's grouping:
    /// systems, managed, scripting, static baseline.
    /// </summary>
    public static readonly IReadOnlyList<LanguageCapability> All =
    [
        // Systems
        new("rust", Http1: true, Http2: true, Http3: true, Apibench: true),
        new("go", Http1: true, Http2: true, Http3: true, Apibench: true),
        new("cpp", Http1: true, Http2: false, Http3: false, Apibench: true),
        // Managed — C# runtime ladder + Java
        new("csharp-net48", Http1: true, Http2: false, Http3: false, Apibench: true),
        new("csharp-net6", Http1: true, Http2: true, Http3: false, Apibench: true),
        new("csharp-net7", Http1: true, Http2: true, Http3: true, Apibench: true),
        new("csharp-net8", Http1: true, Http2: true, Http3: true, Apibench: true),
        new("csharp-net8-aot", Http1: true, Http2: true, Http3: true, Apibench: true),
        new("csharp-net9", Http1: true, Http2: true, Http3: true, Apibench: true),
        new("csharp-net9-aot", Http1: true, Http2: true, Http3: true, Apibench: true),
        new("csharp-net10", Http1: true, Http2: true, Http3: true, Apibench: true),
        new("csharp-net10-aot", Http1: true, Http2: true, Http3: true, Apibench: true),
        new("java", Http1: true, Http2: false, Http3: false, Apibench: true),
        // Scripting
        new("nodejs", Http1: true, Http2: true, Http3: false, Apibench: true),
        new("python", Http1: true, Http2: false, Http3: false, Apibench: true),
        new("ruby", Http1: true, Http2: false, Http3: false, Apibench: true),
        new("php", Http1: true, Http2: false, Http3: false, Apibench: true),
        // Static baseline — transport only, no /api/* suite (API-SPEC.md §9)
        new("nginx", Http1: true, Http2: true, Http3: true, Apibench: false),
    ];

    /// <summary>Lookup by language id; null for unknown ids.</summary>
    public static LanguageCapability? Find(string language) =>
        All.FirstOrDefault(c => c.Language == language);
}
