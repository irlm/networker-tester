using LagHound.Endpoint;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.Http;
using Microsoft.AspNetCore.TestHost;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Hosting;

namespace LagHound.Endpoint.Tests;

/// <summary>
/// Spins up an in-memory ASP.NET Core host with LagHound mounted and two app
/// routes ("/" and "/work") behind it, so tests can assert both LagHound
/// behavior and 404-invisibility / pass-through to the host app.
/// </summary>
internal static class TestHost
{
    internal const string Token = "test-token-0123456789";

    internal static async Task<IHost> StartAsync(Action<LagHoundOptions>? configure = null)
    {
        var host = await new HostBuilder()
            .ConfigureWebHost(web =>
            {
                web.UseTestServer();
                web.ConfigureServices(services =>
                {
                    services.AddLagHound(o =>
                    {
                        o.Token = Token;
                        configure?.Invoke(o);
                    });
                });
                web.Configure(app =>
                {
                    app.UseLagHound();

                    // Host app routes: prove pass-through and that a non-LagHound
                    // unknown path is the framework's own 404, indistinguishable
                    // from the LagHound bad-token 404.
                    app.Run(async ctx =>
                    {
                        if (ctx.Request.Path == "/")
                        {
                            await ctx.Response.WriteAsync("app ok");
                        }
                        else
                        {
                            ctx.Response.StatusCode = StatusCodes.Status404NotFound;
                        }
                    });
                });
            })
            .StartAsync();

        return host;
    }

    internal static HttpClient Client(IHost host) => host.GetTestClient();

    internal static HttpRequestMessage Authed(HttpMethod method, string path)
    {
        var req = new HttpRequestMessage(method, path);
        req.Headers.Add("X-LagHound-Token", Token);
        return req;
    }

    internal static HttpRequestMessage Bearer(HttpMethod method, string path, string token)
    {
        var req = new HttpRequestMessage(method, path);
        req.Headers.Add("Authorization", "Bearer " + token);
        return req;
    }
}
