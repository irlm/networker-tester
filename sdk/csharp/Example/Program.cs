using System.Diagnostics;
using LagHound.Endpoint;

// Minimal ASP.NET app that mounts LagHound at /laghound and adds two app routes.
// Token: LAGHOUND_TOKEN env (default 'demo-token-laghound'). Port: PORT (default 8081).

var builder = WebApplication.CreateBuilder(args);

string token = Environment.GetEnvironmentVariable("LAGHOUND_TOKEN") ?? "demo-token-laghound";
string port = Environment.GetEnvironmentVariable("PORT") ?? "8081";
builder.WebHost.UseUrls($"http://0.0.0.0:{port}");

builder.Services.AddLagHound(o =>
{
    o.Token = token;
    o.Prefix = "/laghound";
    o.AppName = "csharp-sample";
});

var app = builder.Build();

app.UseLagHound();

// App route 1: liveness sanity.
app.MapGet("/", () => "csharp sample ok");

// App route 2: ~30 ms of simulated work.
app.MapGet("/work", async () =>
{
    var sw = Stopwatch.StartNew();
    await Task.Delay(30);
    return Results.Text($"worked for {sw.ElapsedMilliseconds}ms");
});

app.Run();
