using System.Net;
using System.Net.Http.Json;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.Http;
using Microsoft.AspNetCore.TestHost;
using Microsoft.Extensions.Hosting;
using Networker.ControlPlane;

namespace Networker.Tests;

/// <summary>
/// The control plane's global 500 contract (<see cref="ErrorEnvelope"/>):
/// an unhandled exception anywhere below the first middleware must surface as
/// the same <c>{ "error": "..." }</c> envelope the 4xx surface already uses —
/// with a fixed, non-leaking message — instead of Kestrel's undefined empty
/// 500. Exercised through a real in-memory server (TestServer) running the
/// exact middleware Program.cs installs.
///
/// <para>The "4xx envelopes are untouched" half of the contract is covered
/// end-to-end by <see cref="ControlPlaneIntegrationTests"/> (401/403/etc.
/// against the full app), since the handler only fires on unhandled
/// exceptions.</para>
/// </summary>
public sealed class ErrorEnvelopeTests
{
    private static Task<IHost> StartHostAsync(Action<IApplicationBuilder> configure) =>
        new HostBuilder()
            .ConfigureWebHost(web => web.UseTestServer().Configure(configure))
            .StartAsync();

    [Fact]
    public async Task Unhandled_exception_returns_the_uniform_500_envelope()
    {
        using var host = await StartHostAsync(app =>
        {
            app.UseErrorEnvelope();
            app.Run(_ => throw new InvalidOperationException(
                "secret connection string / stack details that must never leak"));
        });
        var client = host.GetTestServer().CreateClient();

        var resp = await client.GetAsync("/api/anything");

        Assert.Equal(HttpStatusCode.InternalServerError, resp.StatusCode);
        Assert.StartsWith("application/json", resp.Content.Headers.ContentType?.MediaType);

        var body = await resp.Content.ReadFromJsonAsync<ErrorBody>();
        Assert.Equal(ErrorEnvelope.InternalErrorMessage, body!.Error);

        // The exception's details must not leak into the response body.
        var raw = await resp.Content.ReadAsStringAsync();
        Assert.DoesNotContain("secret connection string", raw);
        Assert.DoesNotContain("InvalidOperationException", raw);
    }

    [Fact]
    public async Task Handled_responses_pass_through_untouched()
    {
        using var host = await StartHostAsync(app =>
        {
            app.UseErrorEnvelope();
            app.Run(async ctx =>
            {
                ctx.Response.StatusCode = StatusCodes.Status404NotFound;
                await ctx.Response.WriteAsJsonAsync(new { error = "config not found" });
            });
        });
        var client = host.GetTestServer().CreateClient();

        var resp = await client.GetAsync("/api/anything");

        // A handled 4xx (the existing envelope contract) is not rewritten.
        Assert.Equal(HttpStatusCode.NotFound, resp.StatusCode);
        var body = await resp.Content.ReadFromJsonAsync<ErrorBody>();
        Assert.Equal("config not found", body!.Error);
    }

    private sealed record ErrorBody(string Error);
}
