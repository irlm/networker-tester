using System.Security.Cryptography.X509Certificates;
using Microsoft.AspNetCore.Server.Kestrel.Core;

var builder = WebApplication.CreateBuilder(args);

// Configure Kestrel for HTTPS on port 8443 with HTTP/1.1 and HTTP/2
builder.WebHost.ConfigureKestrel(options =>
{
    var certDir  = Environment.GetEnvironmentVariable("BENCH_CERT_DIR") ?? "/opt/bench";
    var certPath = $"{certDir}/cert.pem";
    var keyPath  = $"{certDir}/key.pem";
    var port     = int.Parse(Environment.GetEnvironmentVariable("BENCH_PORT") ?? "8443");

    var cert = X509Certificate2.CreateFromPemFile(certPath, keyPath);

    options.ListenAnyIP(port, listenOptions =>
    {
        listenOptions.UseHttps(cert);
        listenOptions.Protocols = HttpProtocols.Http1AndHttp2;
    });
});

var app = builder.Build();

// GET /health — runtime identity and version
app.MapGet("/health", () => Results.Json(new
{
    status  = "ok",
    runtime = "csharp-net10",
    version = Environment.Version.ToString()
}));

// GET /download/{size} — stream `size` bytes of 0x42 in 8 KiB chunks
app.MapGet("/download/{size}", async (long size, HttpContext ctx) =>
{
    if (size <= 0)
    {
        ctx.Response.StatusCode = 400;
        return;
    }

    ctx.Response.ContentType   = "application/octet-stream";
    ctx.Response.ContentLength = size;

    const int chunkSize = 8192;
    var buffer = new byte[chunkSize];
    Array.Fill(buffer, (byte)0x42);

    var remaining = size;
    while (remaining > 0)
    {
        var toWrite = (int)Math.Min(remaining, chunkSize);
        await ctx.Response.Body.WriteAsync(buffer.AsMemory(0, toWrite));
        remaining -= toWrite;
    }
});

// POST /upload — consume full request body, return byte count
app.MapPost("/upload", async (HttpContext ctx) =>
{
    const int bufferSize = 8192;
    var buffer = new byte[bufferSize];
    long totalBytes = 0;

    int bytesRead;
    while ((bytesRead = await ctx.Request.Body.ReadAsync(buffer)) > 0)
    {
        totalBytes += bytesRead;
    }

    return Results.Json(new { bytes_received = totalBytes });
});

app.Run();
