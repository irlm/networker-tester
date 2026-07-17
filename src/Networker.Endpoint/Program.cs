using System.Net;
using System.Text.Json;
using Microsoft.AspNetCore.Server.Kestrel.Core;
using Networker.Endpoint;

// ─────────────────────────────────────────────────────────────────────────────
// Configuration
//
// Faithful port of the Rust `main.rs`: ports resolve from CLI flags, then an
// optional JSON --config file, then defaults (8080 / 8443 / 9999 / 9998).
// Environment variables of the same name (ENDPOINT_HTTP_PORT etc.) are also
// honoured for containerized deployment (the Rust binary is configured via
// flags/JSON; env is added here for parity with the rest of the C# stack).
// ─────────────────────────────────────────────────────────────────────────────

var cliArgs = ParseArgs(args);

ConfigFile fileCfg = new();
if (cliArgs.TryGetValue("config", out var configPath) && !string.IsNullOrEmpty(configPath))
{
    var json = File.ReadAllText(configPath);
    fileCfg = JsonSerializer.Deserialize<ConfigFile>(json, new JsonSerializerOptions
    {
        PropertyNameCaseInsensitive = true,
    }) ?? new ConfigFile();
}

ushort httpPort = ResolvePort("http_port", "ENDPOINT_HTTP_PORT", fileCfg.http_port, 8080);
ushort httpsPort = ResolvePort("https_port", "ENDPOINT_HTTPS_PORT", fileCfg.https_port, 8443);
ushort udpPort = ResolvePort("udp_port", "ENDPOINT_UDP_PORT", fileCfg.udp_port, 9999);
ushort udpTpPort = ResolvePort("udp_throughput_port", "ENDPOINT_UDP_THROUGHPUT_PORT", fileCfg.udp_throughput_port, 9998);

// HTTP/3 is on by default (mirrors the Rust `http3` feature). ENDPOINT_HTTP3=0
// disables the Alt-Svc advertisement + QUIC listener.
Http3.Enabled = ReadBoolEnv("ENDPOINT_HTTP3", defaultValue: true);

var builder = WebApplication.CreateBuilder(args);

// Self-signed cert (dev) so HTTPS + HTTP/2 (+ HTTP/3) work out of the box,
// matching the Rust server which generates an rcgen self-signed cert at startup.
var cert = SelfSignedCert.Generate();

builder.WebHost.ConfigureKestrel(options =>
{
    // Plain HTTP — HTTP/1.1 + HTTP/2 (h2c prior-knowledge). TCP_NODELAY is on by
    // default in Kestrel, matching the Rust NoDelayAcceptor.
    options.ListenAnyIP(httpPort, listen =>
    {
        listen.Protocols = HttpProtocols.Http1AndHttp2;
    });

    // HTTPS — HTTP/1.1 + HTTP/2 via ALPN, and HTTP/3 when enabled.
    options.ListenAnyIP(httpsPort, listen =>
    {
        listen.Protocols = Http3.Enabled
            ? HttpProtocols.Http1AndHttp2AndHttp3
            : HttpProtocols.Http1AndHttp2;
        listen.UseHttps(cert);
    });

    // Match the Rust 2 GiB body cap.
    options.Limits.MaxRequestBodySize = 2L * 1024 * 1024 * 1024;
});

// App state (shared singleton, mirrors the Rust AppState threaded via with_state).
var appState = new AppState
{
    H3Port = Http3.Enabled ? httpsPort : null,
    HttpPort = httpPort,
    HttpsPort = httpsPort,
    UdpPort = udpPort,
    UdpThroughputPort = udpTpPort,
    SystemMeta = SystemMeta.Collect(),
};
builder.Services.AddSingleton(appState);

// UDP echo + throughput background listeners (mirror the two tokio::spawn tasks).
builder.Services.AddSingleton<IHostedService>(sp =>
    new UdpEchoService(udpPort, sp.GetRequiredService<ILogger<UdpEchoService>>()));
builder.Services.AddSingleton<IHostedService>(sp =>
    new UdpThroughputService(udpTpPort, sp.GetRequiredService<ILogger<UdpThroughputService>>()));

var app = builder.Build();

// Allow bodies up to 2 GiB on the endpoints that read them.
app.Use(async (ctx, next) =>
{
    var feature = ctx.Features.Get<Microsoft.AspNetCore.Http.Features.IHttpMaxRequestBodySizeFeature>();
    if (feature is not null && !feature.IsReadOnly)
        feature.MaxRequestBodySize = 2L * 1024 * 1024 * 1024;
    await next();
});

// Middleware ordering mirrors the Rust layer stack (outermost first):
//   request logging (Kestrel/ASP.NET) -> server timestamp -> bench auth -> handler
app.UseMiddleware<ServerTimestampMiddleware>();
app.UseMiddleware<BenchAuthMiddleware>();

Endpoints.MapEndpoints(app);

// Fatal-at-startup dataset policy (API-SPEC.md §2): a misconfigured
// BENCH_DATA_PATH or corrupt dataset must kill the process, not silently
// fall back to PRNG data.
BenchData.EnsureLoaded();

var log = app.Services.GetRequiredService<ILogger<Program>>();
log.LogInformation("networker-endpoint v{Version}", ServerInfo.Version);
log.LogInformation("HTTP  -> http://0.0.0.0:{Port}", httpPort);
log.LogInformation("HTTPS -> https://0.0.0.0:{Port}  (self-signed, use --insecure)", httpsPort);
log.LogInformation("UDP echo       -> 0.0.0.0:{Port}", udpPort);
log.LogInformation("UDP throughput -> 0.0.0.0:{Port}", udpTpPort);
if (Http3.Enabled)
    log.LogInformation("HTTP/3 QUIC -> udp://0.0.0.0:{Port}  (self-signed, use --insecure)", httpsPort);

app.Run();

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

ushort ResolvePort(string cliKey, string envKey, ushort? fileVal, ushort dflt)
{
    if (cliArgs.TryGetValue(cliKey, out var s) && ushort.TryParse(s, out var v)) return v;
    var env = Environment.GetEnvironmentVariable(envKey);
    if (!string.IsNullOrEmpty(env) && ushort.TryParse(env, out var ev)) return ev;
    return fileVal ?? dflt;
}

static bool ReadBoolEnv(string key, bool defaultValue)
{
    var v = Environment.GetEnvironmentVariable(key);
    if (string.IsNullOrEmpty(v)) return defaultValue;
    return v is not ("0" or "false" or "no" or "off");
}

static Dictionary<string, string> ParseArgs(string[] argv)
{
    var map = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
    for (var i = 0; i < argv.Length; i++)
    {
        var a = argv[i];
        if (a.StartsWith("--", StringComparison.Ordinal))
        {
            var key = a[2..].Replace('-', '_');
            var eq = key.IndexOf('=');
            if (eq >= 0)
            {
                map[key[..eq]] = key[(eq + 1)..];
            }
            else if (i + 1 < argv.Length && !argv[i + 1].StartsWith("--", StringComparison.Ordinal))
            {
                map[key] = argv[++i];
            }
        }
        else if (a is "-c" && i + 1 < argv.Length)
        {
            map["config"] = argv[++i];
        }
    }
    return map;
}

/// <summary>JSON config file shape, mirroring the Rust <c>ConfigFile</c> struct.</summary>
internal sealed class ConfigFile
{
    public ushort? http_port { get; set; }
    public ushort? https_port { get; set; }
    public ushort? udp_port { get; set; }
    public ushort? udp_throughput_port { get; set; }
    public string? log_level { get; set; }
}

/// <summary>Exposed so the test host (WebApplicationFactory) can target this assembly.</summary>
public partial class Program;
