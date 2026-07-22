using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.Http;
using Microsoft.AspNetCore.TestHost;
using Microsoft.Extensions.Hosting;
using Networker.ControlPlane.Security;

namespace Networker.Tests;

/// <summary>
/// The baseline response-hardening headers (<see cref="SecurityHeaders"/>,
/// websec audit 2026-07 P1-3): every control-plane response carries
/// X-Content-Type-Options / X-Frame-Options / Referrer-Policy / CSP, and HSTS is
/// emitted only on genuinely-HTTPS requests (nginx forwards
/// <c>X-Forwarded-Proto: https</c>). Exercised through a real in-memory pipeline.
/// </summary>
public sealed class SecurityHeadersTests
{
    private static Task<IHost> StartHostAsync() =>
        new HostBuilder()
            .ConfigureWebHost(web => web.UseTestServer().Configure(app =>
            {
                app.UseSecurityHeaders();
                app.Run(async ctx => await ctx.Response.WriteAsync("ok"));
            }))
            .StartAsync();

    [Fact]
    public async Task Baseline_headers_are_present_on_every_response()
    {
        using var host = await StartHostAsync();
        var client = host.GetTestServer().CreateClient();

        var resp = await client.GetAsync("/api/anything");

        Assert.Equal("nosniff", resp.Headers.GetValues("X-Content-Type-Options").Single());
        Assert.Equal("DENY", resp.Headers.GetValues("X-Frame-Options").Single());
        Assert.Equal("no-referrer", resp.Headers.GetValues("Referrer-Policy").Single());
        Assert.Contains("default-src 'none'", resp.Headers.GetValues("Content-Security-Policy").Single());
    }

    [Fact]
    public async Task Hsts_is_emitted_only_for_https_forwarded_requests()
    {
        using var host = await StartHostAsync();
        var client = host.GetTestServer().CreateClient();

        // Plain HTTP (no forwarded-proto) → no HSTS.
        var plain = await client.GetAsync("/api/anything");
        Assert.False(plain.Headers.Contains("Strict-Transport-Security"));

        // nginx-terminated HTTPS → HSTS present.
        var req = new HttpRequestMessage(HttpMethod.Get, "/api/anything");
        req.Headers.Add("X-Forwarded-Proto", "https");
        var secure = await client.SendAsync(req);
        Assert.Contains("max-age=", secure.Headers.GetValues("Strict-Transport-Security").Single());
    }
}
