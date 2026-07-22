using System.Net;
using System.Net.Http.Headers;
using Microsoft.Extensions.DependencyInjection;
using Networker.ControlPlane.Auth;
using Networker.Data.Entities;

namespace Networker.Tests;

/// <summary>
/// End-to-end enforcement of <see cref="UserStatusMiddleware"/> — the "trust the
/// DB over the token" gate that runs on every authenticated request. Untested
/// until now, yet it is the control that stops a DELETED, DISABLED, PENDING, or
/// must-change user from acting on a still-valid JWT. A regression here (e.g. the
/// middleware silently dropped from the pipeline, or a status branch inverted)
/// would let a revoked account keep full access for the token's lifetime — with
/// no crash and no log. Each test mints a real JWT for a freshly-seeded user and
/// asserts the middleware's own text/plain 403 body (which distinguishes it from
/// an authorization 403).
///
/// Runs against the shared real-Postgres fixture; fresh per-user GUIDs avoid the
/// middleware's 10 s status cache colliding between tests.
/// </summary>
public sealed class UserStatusMiddlewareTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fixture;

    public UserStatusMiddlewareTests(ControlPlaneFixture fixture) => _fixture = fixture;

    // A non-profile, authenticated route — the middleware runs BEFORE
    // authorization, so its 403 fires regardless of project membership.
    private static readonly string ProtectedRoute = $"/api/projects/{ControlPlaneFixture.SeededProjectId}";
    private const string ProfileRoute = "/api/auth/profile";

    /// Seed a dash_user with the given status (unless insert=false, which
    /// simulates a deleted / never-existed user) and return a client bearing a
    /// valid JWT minted by the app's own signing key.
    private HttpClient ClientForUser(
        Guid userId, string status, bool mustChange = false, bool insert = true, string role = "viewer")
    {
        if (insert)
        {
            using var ctx = _fixture.NewDbContext();
            ctx.DashUsers.Add(new DashUser
            {
                UserId = userId,
                Email = $"{userId:N}@usm.local",
                Role = role,
                Status = status,
                AuthProvider = "local",
                IsPlatformAdmin = false,
                MustChangePassword = mustChange,
                SsoOnly = false,
                CreatedAt = new DateTime(2026, 1, 1, 0, 0, 0, DateTimeKind.Utc),
            });
            ctx.SaveChanges();
        }

        var client = _fixture.CreateClient();
        var tokens = _fixture.Services.GetRequiredService<JwtTokenService>();
        var jwt = tokens.CreateToken(userId, $"{userId:N}@usm.local", role, isPlatformAdmin: false);
        client.DefaultRequestHeaders.Authorization = new AuthenticationHeaderValue("Bearer", jwt);
        return client;
    }

    [Fact]
    public async Task Disabled_user_is_forbidden_even_with_a_valid_token()
    {
        var client = ClientForUser(Guid.NewGuid(), status: "disabled");

        var resp = await client.GetAsync(ProtectedRoute);

        Assert.Equal(HttpStatusCode.Forbidden, resp.StatusCode);
        Assert.Contains("Account is not active", await resp.Content.ReadAsStringAsync());
    }

    [Fact]
    public async Task Deleted_user_with_no_row_is_forbidden_fail_closed()
    {
        // Valid signature, but the sub has no dash_user row (deleted account /
        // forged sub). The token's role claim must NOT be honored.
        var client = ClientForUser(Guid.NewGuid(), status: "active", insert: false);

        var resp = await client.GetAsync(ProtectedRoute);

        Assert.Equal(HttpStatusCode.Forbidden, resp.StatusCode);
        Assert.Contains("Account is not active", await resp.Content.ReadAsStringAsync());
    }

    [Fact]
    public async Task Pending_user_is_forbidden_on_a_normal_route()
    {
        var client = ClientForUser(Guid.NewGuid(), status: "pending");

        var resp = await client.GetAsync(ProtectedRoute);

        Assert.Equal(HttpStatusCode.Forbidden, resp.StatusCode);
        Assert.Contains("pending_approval", await resp.Content.ReadAsStringAsync());
    }

    [Fact]
    public async Task Pending_user_may_still_reach_profile()
    {
        // The one allowance: a pending user can read /auth/profile (and change
        // their password) so the approval flow is reachable.
        var client = ClientForUser(Guid.NewGuid(), status: "pending");

        var resp = await client.GetAsync(ProfileRoute);

        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
    }

    [Fact]
    public async Task Must_change_password_user_is_forbidden_on_a_normal_route()
    {
        var client = ClientForUser(Guid.NewGuid(), status: "active", mustChange: true);

        var resp = await client.GetAsync(ProtectedRoute);

        Assert.Equal(HttpStatusCode.Forbidden, resp.StatusCode);
        Assert.Contains("Password change required", await resp.Content.ReadAsStringAsync());
    }

    [Fact]
    public async Task Must_change_password_user_may_still_reach_profile()
    {
        var client = ClientForUser(Guid.NewGuid(), status: "active", mustChange: true);

        var resp = await client.GetAsync(ProfileRoute);

        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
    }

    [Fact]
    public async Task Active_user_passes_the_status_gate()
    {
        // Positive control: an ordinary active user is not blocked by the gate.
        var client = ClientForUser(Guid.NewGuid(), status: "active");

        var resp = await client.GetAsync(ProfileRoute);

        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
    }
}
