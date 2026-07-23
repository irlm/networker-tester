using System.Net;

namespace Networker.Tests;

/// <summary>
/// Read-surface smoke coverage: hit every GET endpoint against a REAL Postgres
/// (via <see cref="ControlPlaneFixture"/>) through the real HTTP + auth + EF
/// pipeline and assert the response is not a 5xx.
///
/// <para><b>Why this exists:</b> the 2026-07 perf sweep found two endpoints
/// (<c>GET /members</c>, and <c>GET /projects</c> for non-admins) that 500'd on
/// EVERY call — an EF query that ordered by a client-constructed record after a
/// Join, untranslatable to SQL. The unit-test seams never executed that query
/// against a relational provider, so CI stayed green while the pages were
/// broken. A per-route smoke test that runs the actual query against real
/// Postgres catches that entire class (untranslatable LINQ, bad column map,
/// dangling projection) before it ships.</para>
///
/// <para>The seeded admin (<see cref="ControlPlaneFixture.CreateAdminClient"/>)
/// is a project-admin member with global role <c>admin</c>, so it clears both
/// the ProjectAdmin and GlobalAdmin gates and actually reaches each handler
/// rather than bouncing at 403 — the point is to execute the query.</para>
///
/// <para><b>Excluded</b> (documented, not silently dropped): endpoints backed by
/// raw SQL against tables outside the EF model (<c>/api/logs*</c>,
/// <c>/api/perf-log*</c>, <c>/api/admin/metrics</c> — perf_log / log_entry aren't
/// mapped, so the fixture's model-DDL doesn't create them; and raw SQL can't
/// have the EF-translation bug anyway), and streaming endpoints
/// (<c>/api/events/approval</c>, <c>/commands/{id}/stream</c>) that hold the
/// connection open. Detail-by-id routes with no seeded row use a fixed
/// not-found GUID: a clean 404 still proves the query executed without a 500.</para>
/// </summary>
public sealed class EndpointSmokeTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fixture;

    public EndpointSmokeTests(ControlPlaneFixture fixture) => _fixture = fixture;

    private const string Pid = ControlPlaneFixture.SeededProjectId;
    private static readonly string Cfg = ControlPlaneFixture.SeededConfigId.ToString();
    private static readonly string Tester = ControlPlaneFixture.SeededTesterId.ToString();
    private const string NotFound = "00000000-0000-0000-0000-0000000000ff";

    public static IEnumerable<object[]> GetRoutes()
    {
        var routes = new[]
        {
            // ── util / anonymous ──────────────────────────────────────────────
            "/api/health",
            "/api/health/ready",
            "/api/health/background",
            "/api/system/health",
            "/api/version",
            "/api/modes",
            "/api/zones",

            // ── global read surface (admin role clears the global gates) ──────
            "/api/projects",
            "/api/users",
            "/api/users/pending",
            "/api/me/pending-projects",
            "/api/admin/workspaces",
            "/api/admin/system-config/smtp",
            "/api/leaderboard",
            "/api/leaderboard/grouped",
            "/api/leaderboard/runs",
            "/api/sso-providers",
            "/api/bench-tokens",

            // token lookups — a non-existent token must 404, not 500
            $"/api/invite/{NotFound}",
            $"/api/share/{NotFound}",

            // ── project-scoped list endpoints (the ORDER BY / JOIN surface) ───
            $"/api/projects/{Pid}",
            $"/api/projects/{Pid}/agents",
            $"/api/projects/{Pid}/benchmark-catalog",
            $"/api/projects/{Pid}/cloud-accounts",
            $"/api/projects/{Pid}/cloud-connections",
            $"/api/projects/{Pid}/cloud/status",
            $"/api/projects/{Pid}/command-approvals",
            $"/api/projects/{Pid}/command-approvals/count",
            $"/api/projects/{Pid}/dashboard/summary",
            $"/api/projects/{Pid}/deployments",
            $"/api/projects/{Pid}/inventory",
            $"/api/projects/{Pid}/invites",
            $"/api/projects/{Pid}/members",
            $"/api/projects/{Pid}/reports/app-network",
            $"/api/projects/{Pid}/reports/perf-per-cost",
            $"/api/projects/{Pid}/sdk-endpoints",
            $"/api/projects/{Pid}/share-links",
            $"/api/projects/{Pid}/testers",
            $"/api/projects/{Pid}/testers/regions",
            $"/api/projects/{Pid}/tls-profiles",
            $"/api/projects/{Pid}/url-tests",
            $"/api/projects/{Pid}/visibility-rules",
            $"/api/projects/{Pid}/vm-history",
            $"/api/v2/projects/{Pid}/alert-channels",
            $"/api/v2/projects/{Pid}/alert-events",
            $"/api/v2/projects/{Pid}/alert-rules",
            $"/api/v2/projects/{Pid}/comparison-groups",
            $"/api/v2/projects/{Pid}/schedules",
            $"/api/v2/projects/{Pid}/test-configs",
            $"/api/v2/projects/{Pid}/test-runs",

            // ── detail-by-id, seeded rows (200 path) ──────────────────────────
            $"/api/projects/{Pid}/testers/{Tester}",
            $"/api/projects/{Pid}/testers/{Tester}/cost_estimate",
            $"/api/projects/{Pid}/testers/{Tester}/queue",
            $"/api/v2/test-configs/{Cfg}",

            // ── detail-by-id, no row (must 404 cleanly, not 500) ──────────────
            $"/api/projects/{Pid}/agents/{NotFound}",
            $"/api/projects/{Pid}/cloud-accounts/{NotFound}",
            $"/api/projects/{Pid}/cloud-connections/{NotFound}",
            $"/api/projects/{Pid}/sdk-endpoints/{NotFound}",
            $"/api/projects/{Pid}/deployments/{NotFound}",
            $"/api/projects/{Pid}/commands/{NotFound}",
            $"/api/projects/{Pid}/tls-profiles/{NotFound}",
            $"/api/projects/{Pid}/url-tests/{NotFound}",
            $"/api/projects/{Pid}/url-tests/{NotFound}/sections",
            $"/api/v2/test-runs/{NotFound}",
            $"/api/v2/test-runs/{NotFound}/attempts",
            $"/api/v2/test-runs/{NotFound}/artifact",
            $"/api/v2/schedules/{NotFound}",
            $"/api/v2/comparison-groups/{NotFound}",
        };
        return routes.Select(r => new object[] { r });
    }

    [Theory]
    [MemberData(nameof(GetRoutes))]
    public async Task Get_endpoint_does_not_return_5xx(string route)
    {
        var client = _fixture.CreateAdminClient();

        var resp = await client.GetAsync(route);

        Assert.True(
            (int)resp.StatusCode < 500,
            $"GET {route} returned {(int)resp.StatusCode} {resp.StatusCode} — a server error " +
            "(likely an untranslatable EF query or bad column map). Body: " +
            await resp.Content.ReadAsStringAsync());
    }
}
