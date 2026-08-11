using System.Net.Http.Headers;
using Microsoft.Extensions.DependencyInjection;
using Networker.ControlPlane.Auth;

namespace Networker.Tests;

/// <summary>
/// Regression pin for the 2026-08-11 prod incident: parallel COLD requests
/// to <c>/api/system/health</c> right after a deploy raced EF Core 10.0.0's
/// first query compilation and 500'd with
/// <c>KeyNotFoundException: 'EmptyProjectionMember'</c>. Single-threaded
/// smoke tests never see it — the race needs a fresh app instance (cold
/// compiled-query cache) and concurrent first hits, which is exactly what
/// every deploy produces when the dashboard's polling panels reconnect.
/// Fixed by the aggregate+join query shape + the EF 10.0.11 patch bump;
/// this test keeps both honest.
/// </summary>
public sealed class SystemHealthColdStartTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fixture;

    public SystemHealthColdStartTests(ControlPlaneFixture fixture) => _fixture = fixture;

    [Fact]
    public async Task Parallel_cold_first_requests_do_not_500()
    {
        // A derived factory builds a NEW host on first use — cold EF caches,
        // same Postgres container.
        using var cold = _fixture.WithWebHostBuilder(_ => { });
        var tokens = cold.Services.GetRequiredService<JwtTokenService>();
        var jwt = tokens.CreateToken(
            ControlPlaneFixture.SeededAdminUserId,
            ControlPlaneFixture.SeededAdminEmail,
            "admin",
            isPlatformAdmin: true);

        var clients = Enumerable.Range(0, 8).Select(_ =>
        {
            var client = cold.CreateClient();
            client.DefaultRequestHeaders.Authorization = new AuthenticationHeaderValue("Bearer", jwt);
            return client;
        }).ToArray();

        var responses = await Task.WhenAll(
            clients.Select(c => c.GetAsync("/api/system/health")));

        Assert.All(responses, r => Assert.True(
            (int)r.StatusCode < 500,
            $"cold parallel /api/system/health returned {(int)r.StatusCode} — the deploy-window race is back"));

        foreach (var c in clients) c.Dispose();
    }
}
