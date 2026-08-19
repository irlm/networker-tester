using System.Diagnostics;
using LagHound.Endpoint;
using LagHound.Example;

// Minimal ASP.NET app that mounts LagHound at /laghound and serves a small,
// human-readable demo page. Token: LAGHOUND_TOKEN env (default
// 'demo-token-laghound'). Port: PORT (default 8081).

var builder = WebApplication.CreateBuilder(args);

string token = Environment.GetEnvironmentVariable("LAGHOUND_TOKEN") ?? "demo-token-laghound";
string port = Environment.GetEnvironmentVariable("PORT") ?? "8081";
bool publicDemo = Environment.GetEnvironmentVariable("LAGHOUND_PUBLIC_DEMO") == "1";
builder.WebHost.UseUrls($"http://0.0.0.0:{port}");

builder.Services.AddLagHound(o =>
{
    o.Token = token;
    o.Prefix = "/laghound";
    o.AppName = "laghound-csharp-demo";

    // The public reference deployment only needs health/echo/info. Keeping the
    // transfer routes off prevents a known demo token from becoming a free
    // bandwidth relay. Local/conformance use keeps the full contract enabled.
    o.EnableDownload = !publicDemo;
    o.EnableUpload = !publicDemo;
    if (publicDemo)
    {
        o.RatePerIpRps = 2;
        o.RatePerIpBurst = 5;
        o.RateGlobalRps = 10;
        o.RateGlobalBurst = 20;
    }
});

var app = builder.Build();

app.UseLagHound();

// Human-facing reference page. The SDK routes remain token-gated and invisible
// to unauthenticated requests; this page intentionally contains no token.
app.MapGet("/", () => Results.Content(DemoPage.Html, "text/html; charset=utf-8"));

// App route 2: ~30 ms of simulated work.
app.MapGet("/work", async () =>
{
    var sw = Stopwatch.StartNew();
    await Task.Delay(30);
    return Results.Text($"C# handler completed in {sw.ElapsedMilliseconds} ms");
});

app.Run();
