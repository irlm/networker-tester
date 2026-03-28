using System.Text.Json.Serialization;
using Microsoft.AspNetCore.Server.Kestrel.Core;

// --- AOT-compatible JSON serializer context ---
[JsonSerializable(typeof(HealthResponse))]
[JsonSerializable(typeof(UploadResponse))]
internal partial class AppJsonContext : JsonSerializerContext { }

record HealthResponse(string status, string runtime, string version);
record UploadResponse(long bytes_received);

// --- Application ---
var builder = WebApplication.CreateSlimBuilder(args);
builder.WebHost.ConfigureKestrel(options =>
{
    options.ListenAnyIP(8443, listenOptions =>
    {
        listenOptions.UseHttps("/opt/bench/cert.pem", "/opt/bench/key.pem");
        listenOptions.Protocols = HttpProtocols.Http1AndHttp2;
    });
});

var app = builder.Build();

app.MapGet("/health", () => Results.Json(
    new HealthResponse("ok", "csharp-net9-aot", Environment.Version.ToString()),
    AppJsonContext.Default.HealthResponse));

app.MapGet("/download/{size:long}", (long size) =>
{
    return Results.Stream(async stream =>
    {
        var buffer = new byte[8192];
        Array.Fill(buffer, (byte)0x42);
        long remaining = size;
        while (remaining > 0)
        {
            int chunk = (int)Math.Min(remaining, buffer.Length);
            await stream.WriteAsync(buffer.AsMemory(0, chunk));
            remaining -= chunk;
        }
    }, "application/octet-stream");
});

app.MapPost("/upload", async (HttpRequest req) =>
{
    long total = 0;
    var buffer = new byte[8192];
    int read;
    while ((read = await req.Body.ReadAsync(buffer)) > 0)
        total += read;
    return Results.Json(
        new UploadResponse(total),
        AppJsonContext.Default.UploadResponse);
});

app.Run();
