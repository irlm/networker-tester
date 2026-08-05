using System.Net;
using System.Net.Http.Json;
using Xunit;

namespace Networker.Tests;

/// <summary>
/// Audit P1-6: RBAC was only spot-checked — two viewer-negatives across a
/// 132-route surface, and the dashboard's `*.rbac.test.tsx` files mock the API
/// client, so they prove what the UI hides, never what the SERVER enforces.
/// (Hiding a button is not access control.)
///
/// <para>This drives the real routes with real JWTs for each principal and
/// asserts the exact expected outcome per (role × route). A viewer who can
/// mutate is a security defect; an operator who cannot do their job is a
/// functional one — both directions matter, so both are asserted.</para>
/// </summary>
public class RbacRouteMatrixTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fx;

    public RbacRouteMatrixTests(ControlPlaneFixture fx) => _fx = fx;

    private const string Pid = ControlPlaneFixture.SeededProjectId;

    private HttpClient ClientFor(string role) => role switch
    {
        "viewer" => _fx.CreateViewerClient(),
        "operator" => _fx.CreateAuthenticatedClient(),
        "admin" => _fx.CreateAdminClient(),
        "anonymous" => _fx.CreateClient(),
        _ => throw new ArgumentOutOfRangeException(nameof(role), role, "unknown principal"),
    };

    private static bool IsDenied(HttpStatusCode code) =>
        code is HttpStatusCode.Unauthorized or HttpStatusCode.Forbidden or HttpStatusCode.NotFound;

    // ── Writes that a VIEWER must never be able to perform ────────────────
    public static TheoryData<string, string> ViewerForbiddenWrites() => new()
    {
        { "POST", $"/api/v2/projects/{Pid}/test-configs" },
        { "POST", $"/api/projects/{Pid}/share-links" },
        { "POST", $"/api/v2/projects/{Pid}/comparison-groups" },
        { "POST", $"/api/projects/{Pid}/deployments" },
        { "POST", $"/api/v2/projects/{Pid}/schedules" },
        { "POST", $"/api/v2/projects/{Pid}/alert-channels" },
    };

    [Theory]
    [MemberData(nameof(ViewerForbiddenWrites))]
    public async Task Viewer_cannot_perform_project_writes(string method, string route)
    {
        using var client = ClientFor("viewer");
        var req = new HttpRequestMessage(new HttpMethod(method), route)
        {
            Content = JsonContent.Create(new { name = "rbac-probe" }),
        };

        var resp = await client.SendAsync(req);

        Assert.True(IsDenied(resp.StatusCode),
            $"VIEWER was allowed {method} {route} → {(int)resp.StatusCode} {resp.StatusCode}. "
            + "A read-only member must never mutate project state.");
    }

    [Theory]
    [MemberData(nameof(ViewerForbiddenWrites))]
    public async Task Anonymous_cannot_perform_project_writes(string method, string route)
    {
        using var client = ClientFor("anonymous");
        var req = new HttpRequestMessage(new HttpMethod(method), route)
        {
            Content = JsonContent.Create(new { name = "rbac-probe" }),
        };

        var resp = await client.SendAsync(req);

        Assert.True(IsDenied(resp.StatusCode),
            $"ANONYMOUS was allowed {method} {route} → {(int)resp.StatusCode}");
    }

    // ── Reads a viewer legitimately HAS to be able to do ──────────────────
    [Theory]
    // Project-scoped: there is no flat /api/v2/test-runs route (a request to
    // one honestly 404s — verified against TestRunsEndpoints).
    [InlineData("/api/v2/projects/" + Pid + "/test-runs?limit=5")]
    [InlineData("/api/projects")]
    [InlineData("/api/modes")]
    public async Task Viewer_can_still_read(string route)
    {
        using var client = ClientFor("viewer");
        var resp = await client.GetAsync(route);

        // The failure this guards: over-tightening authz until read-only
        // members can't use the product at all.
        Assert.True(resp.IsSuccessStatusCode,
            $"VIEWER was denied read {route} → {(int)resp.StatusCode}");
    }

    // ── Platform-admin-only surfaces ──────────────────────────────────────
    [Theory]
    [InlineData("/api/admin/metrics")]
    public async Task Admin_only_routes_reject_viewer_and_operator(string route)
    {
        foreach (var role in new[] { "viewer", "operator" })
        {
            using var client = ClientFor(role);
            var resp = await client.GetAsync(route);
            Assert.True(IsDenied(resp.StatusCode),
                $"{role.ToUpperInvariant()} reached admin-only {route} → {(int)resp.StatusCode}");
        }
    }

    [Fact]
    public async Task Bench_tokens_are_user_scoped_rather_than_admin_gated()
    {
        // FINDING (2026-08 audit follow-up): the dashboard gates /bench-tokens*
        // behind PLATFORM ADMIN, but the server route is only
        // RequireAuthorization() — any authenticated principal reaches it. That
        // is not a leak, because the handler runs FilterTokensForUser, so a
        // caller only ever sees their OWN tokens. This test pins the property
        // that actually matters (no cross-user disclosure) and documents the
        // UI/server divergence so a future reader doesn't "fix" one side alone.
        using var viewer = ClientFor("viewer");
        var resp = await viewer.GetAsync("/api/bench-tokens");

        if (resp.StatusCode == HttpStatusCode.OK)
        {
            var body = await resp.Content.ReadAsStringAsync();
            Assert.DoesNotContain(ControlPlaneFixture.SeededAdminEmail, body, StringComparison.OrdinalIgnoreCase);
        }
        else
        {
            Assert.True(IsDenied(resp.StatusCode), $"unexpected status {(int)resp.StatusCode}");
        }
    }

    [Fact]
    public async Task Operator_can_create_a_test_config()
    {
        // The positive direction: authz must not be so tight that the role
        // which exists to run tests cannot create one.
        using var client = ClientFor("operator");
        var resp = await client.PostAsJsonAsync(
            $"/api/v2/projects/{Pid}/test-configs",
            new
            {
                name = $"rbac-operator-{Guid.NewGuid():N}",
                endpoint = new { kind = "network", host = "10.0.0.9", port = 8444 },
                workload = new { modes = new[] { "http1" }, runs = 1 },
            });

        Assert.True(resp.IsSuccessStatusCode,
            $"OPERATOR could not create a config → {(int)resp.StatusCode} "
            + $"{await resp.Content.ReadAsStringAsync()}");
    }

    [Fact]
    public async Task Cross_project_access_is_denied_for_every_role()
    {
        // A project id the seeded principals are not members of.
        const string foreignPid = "proj-not-a-member";
        foreach (var role in new[] { "viewer", "operator", "admin" })
        {
            using var client = ClientFor(role);
            var resp = await client.GetAsync($"/api/projects/{foreignPid}/share-links");
            Assert.True(IsDenied(resp.StatusCode),
                $"{role.ToUpperInvariant()} reached a foreign project → {(int)resp.StatusCode}");
        }
    }
}
