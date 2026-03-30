using Microsoft.AspNetCore.Http;

using Microsoft.AspNetCore.Server.Kestrel.Core;
using System.Security.Cryptography.X509Certificates;

var builder = WebApplication.CreateBuilder(args);

var certDir  = Environment.GetEnvironmentVariable("BENCH_CERT_DIR") ?? "/opt/bench";
var certPath = $"{certDir}/cert.pem";
var keyPath  = $"{certDir}/key.pem";
var port     = int.Parse(Environment.GetEnvironmentVariable("BENCH_PORT") ?? "8443");

builder.WebHost.ConfigureKestrel(options =>
{
    var cert = X509Certificate2.CreateFromPemFile(certPath, keyPath);
    options.ListenAnyIP(port, listenOptions =>
    {
        listenOptions.UseHttps(cert);
        listenOptions.Protocols = HttpProtocols.Http1AndHttp2;
    });
});

var app = builder.Build();

app.MapGet("/health", () => Results.Json(new {
    status = "ok",
    runtime = "csharp-net10",
    version = Environment.Version.ToString()
}));

app.MapGet("/download/{size:long}", (long size) => Results.Stream(async stream => {
    var buffer = new byte[8192];
    Array.Fill(buffer, (byte)0x42);
    long remaining = size;
    while (remaining > 0) {
        int chunk = (int)Math.Min(remaining, buffer.Length);
        await stream.WriteAsync(buffer.AsMemory(0, chunk));
        remaining -= chunk;
    }
}, "application/octet-stream"));

app.MapPost("/upload", async (HttpRequest req) => {
    long total = 0;
    var buffer = new byte[8192];
    int read;
    while ((read = await req.Body.ReadAsync(buffer)) > 0)
        total += read;
    return Results.Json(new { bytes_received = total });
});

app.Run();
