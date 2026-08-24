using System.Net;
using System.Net.Http.Headers;
using System.Text.Json;
using Microsoft.AspNetCore.Mvc.Testing;
using Microsoft.Extensions.DependencyInjection;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Endpoints;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.Tests;

/// <summary>
/// <c>GET /api/admin/secrets</c> — the read-only half of secret rotation.
///
/// <para>Two properties are load-bearing and both are asserted here: the route
/// is platform-admin only, and the response contains NO secret material. The
/// panel exists so an operator can see that a key is 400 days old; it must never
/// become a way to read the key itself, which is why rotation lives in
/// scripts/rotate-secrets.sh and not behind a button.</para>
///
/// <para>The host is booted with a sentinel signing key so the leak assertion is
/// real rather than decorative — if the endpoint ever serialised a configured
/// secret, that exact string would appear in the body.</para>
/// </summary>
public sealed class SecretPanelEndpointTests(ControlPlaneFixture shared)
    : IClassFixture<ControlPlaneFixture>, IDisposable
{
    private const string SentinelJwtSecret = "sentinel-jwt-signing-key-must-never-be-served-0123456789";

    private readonly WebApplicationFactory<Program> _app = shared.WithWebHostBuilder(b =>
        b.UseSetting("DASHBOARD_JWT_SECRET", SentinelJwtSecret));

    public void Dispose() => _app.Dispose();

    // The fixture seeds no platform admin, and a token cannot grant the flag on
    // its own: UserStatusMiddleware re-reads role/is_platform_admin from
    // dash_user on every request and overwrites whatever the JWT claimed. So the
    // admin has to exist in the DATABASE. (Worth knowing — it means a stolen or
    // forged token cannot escalate itself to platform admin.)
    private static readonly Guid PanelAdminId = Guid.Parse("55555555-5555-4555-8555-555555555555");
    private const string PanelAdminEmail = "itest-secret-panel-admin@networker.local";

    private void EnsurePlatformAdmin()
    {
        using var scope = _app.Services.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
        if (db.DashUsers.Any(u => u.UserId == PanelAdminId))
        {
            return;
        }
        db.DashUsers.Add(new DashUser
        {
            UserId = PanelAdminId,
            Email = PanelAdminEmail,
            Role = "admin",
            Status = "active",
            AuthProvider = "local",
            IsPlatformAdmin = true,
            MustChangePassword = false,
            SsoOnly = false,
            CreatedAt = DateTime.UtcNow,
        });
        db.SaveChanges();
    }

    private HttpClient Client(bool platformAdmin)
    {
        var client = _app.CreateClient();
        var tokens = _app.Services.GetRequiredService<JwtTokenService>();
        string jwt;
        if (platformAdmin)
        {
            EnsurePlatformAdmin();
            jwt = tokens.CreateToken(PanelAdminId, PanelAdminEmail, "admin", isPlatformAdmin: true);
        }
        else
        {
            jwt = tokens.CreateToken(
                ControlPlaneFixture.SeededUserId, ControlPlaneFixture.SeededUserEmail,
                "operator", isPlatformAdmin: false);
        }
        client.DefaultRequestHeaders.Authorization = new AuthenticationHeaderValue("Bearer", jwt);
        return client;
    }

    [Fact]
    public async Task Unauthenticated_callers_get_401()
    {
        var resp = await _app.CreateClient().GetAsync("/api/admin/secrets");
        Assert.Equal(HttpStatusCode.Unauthorized, resp.StatusCode);
    }

    [Fact]
    public async Task A_non_platform_admin_gets_403()
    {
        var resp = await Client(platformAdmin: false).GetAsync("/api/admin/secrets");
        Assert.Equal(HttpStatusCode.Forbidden, resp.StatusCode);
    }

    [Fact]
    public async Task Every_inventoried_secret_is_reported()
    {
        var resp = await Client(platformAdmin: true).GetAsync("/api/admin/secrets");
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);

        using var doc = JsonDocument.Parse(await resp.Content.ReadAsStringAsync());
        var keys = doc.RootElement.GetProperty("secrets").EnumerateArray()
            .Select(e => e.GetProperty("key").GetString()).ToHashSet(StringComparer.Ordinal);

        foreach (var spec in SecretsEndpoints.Inventory)
        {
            Assert.Contains(spec.Key, keys);
        }
    }

    [Fact]
    public async Task A_secret_with_no_recorded_rotation_reads_never_and_counts_as_needing_attention()
    {
        // This fixture materializes the schema from the EF model and skips the
        // V0NN chain, so secret_rotation does not exist here. That is the
        // pre-V055 case, and the truthful answer is "nothing has been recorded"
        // — NOT an error page. Asserting it here pins the degradation path too.
        var resp = await Client(platformAdmin: true).GetAsync("/api/admin/secrets");
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);

        using var doc = JsonDocument.Parse(await resp.Content.ReadAsStringAsync());
        var items = doc.RootElement.GetProperty("secrets").EnumerateArray().ToList();

        Assert.All(items, e =>
        {
            Assert.Equal("never", e.GetProperty("status").GetString());
            Assert.Equal(JsonValueKind.Null, e.GetProperty("rotated_at").ValueKind);
            Assert.Equal(JsonValueKind.Null, e.GetProperty("age_days").ValueKind);
        });
        Assert.Equal(items.Count, doc.RootElement.GetProperty("needs_attention").GetInt32());
    }

    [Fact]
    public async Task The_response_carries_no_secret_material()
    {
        var body = await (await Client(platformAdmin: true).GetAsync("/api/admin/secrets"))
            .Content.ReadAsStringAsync();

        // The host was booted with this exact signing key. If the endpoint ever
        // serialises a configured secret, this is the assertion that catches it.
        Assert.DoesNotContain(SentinelJwtSecret, body, StringComparison.Ordinal);

        // Nor should it ship a value under any plausible field name.
        foreach (var forbidden in new[] { "\"value\"", "\"secret\"", "\"token\"", "\"password\"" })
        {
            Assert.DoesNotContain(forbidden, body, StringComparison.OrdinalIgnoreCase);
        }
    }
}
