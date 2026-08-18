using System.Net;
using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.Data.Entities;

namespace Networker.Tests;

/// <summary>
/// The durable canary dispatch history (V051 <c>canary_dispatch</c> +
/// <c>GET /api/admin/canary/history</c>) and the live GitHub runs proxy
/// (<c>GET /api/admin/canary/runs</c>) against the real Postgres + booted
/// control plane. The history is the piece that must work when GitHub is
/// unreachable, so these tests never talk to GitHub: no
/// <c>CANARY_GITHUB_TOKEN</c> is set in CI, and /runs must degrade to an
/// empty list with <c>configured=false</c> rather than erroring.
/// </summary>
public sealed class CanaryHistoryTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fixture;

    public CanaryHistoryTests(ControlPlaneFixture fixture) => _fixture = fixture;

    // ── RBAC: platform-admin only, like the rest of /api/admin ──────────────

    [Fact]
    public async Task Canary_history_routes_require_global_admin()
    {
        var anon = _fixture.CreateClient();
        Assert.Equal(HttpStatusCode.Unauthorized,
            (await anon.GetAsync("/api/admin/canary/history")).StatusCode);
        Assert.Equal(HttpStatusCode.Unauthorized,
            (await anon.GetAsync("/api/admin/canary/runs")).StatusCode);

        var viewer = _fixture.CreateViewerClient();
        Assert.Equal(HttpStatusCode.Forbidden,
            (await viewer.GetAsync("/api/admin/canary/history")).StatusCode);
        Assert.Equal(HttpStatusCode.Forbidden,
            (await viewer.GetAsync("/api/admin/canary/runs")).StatusCode);
    }

    // ── History: DB-backed, newest-first, inputs round-trip ─────────────────

    [Fact]
    public async Task History_lists_persisted_dispatches_newest_first()
    {
        var older = Guid.NewGuid();
        var newer = Guid.NewGuid();
        await using (var db = _fixture.NewDbContext())
        {
            db.CanaryDispatches.AddRange(
                new CanaryDispatch
                {
                    Id = older,
                    RequestedBy = "admin@example.com",
                    RequestedAt = DateTime.UtcNow.AddMinutes(-10),
                    GitRef = "main",
                    Inputs = """{"apibench":"1","windows":"0"}""",
                    RunId = 12345,
                    RunUrl = "https://github.com/irlm/networker-tester/actions/runs/12345",
                    RunStatus = "completed",
                    Conclusion = "success",
                    UpdatedAt = DateTime.UtcNow.AddMinutes(-2),
                },
                new CanaryDispatch
                {
                    Id = newer,
                    RequestedBy = "admin@example.com",
                    RequestedAt = DateTime.UtcNow.AddMinutes(-1),
                    GitRef = "main",
                    Inputs = """{"apibench":"0"}""",
                    UpdatedAt = DateTime.UtcNow.AddMinutes(-1),
                });
            await db.SaveChangesAsync();
        }

        var admin = _fixture.CreateAdminClient();
        var resp = await admin.GetAsync("/api/admin/canary/history");
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);

        var body = JsonDocument.Parse(await resp.Content.ReadAsStringAsync()).RootElement;
        var items = body.GetProperty("items").EnumerateArray().ToList();
        var ids = items.Select(i => i.GetProperty("id").GetGuid()).ToList();

        // Both rows are present and the newer dispatch sorts first.
        Assert.Contains(older, ids);
        Assert.Contains(newer, ids);
        Assert.True(ids.IndexOf(newer) < ids.IndexOf(older));

        var completed = items.Single(i => i.GetProperty("id").GetGuid() == older);
        Assert.Equal("success", completed.GetProperty("conclusion").GetString());
        Assert.Equal(12345, completed.GetProperty("run_id").GetInt64());
        // Inputs jsonb round-trips as a JSON object, not a string.
        Assert.Equal("1", completed.GetProperty("inputs").GetProperty("apibench").GetString());

        var pending = items.Single(i => i.GetProperty("id").GetGuid() == newer);
        Assert.Equal(JsonValueKind.Null, pending.GetProperty("run_id").ValueKind);
        Assert.Equal(JsonValueKind.Null, pending.GetProperty("conclusion").ValueKind);
    }

    [Fact]
    public async Task History_respects_the_limit_parameter()
    {
        await using (var db = _fixture.NewDbContext())
        {
            for (var i = 0; i < 3; i++)
            {
                db.CanaryDispatches.Add(new CanaryDispatch
                {
                    Id = Guid.NewGuid(),
                    RequestedAt = DateTime.UtcNow.AddSeconds(-i),
                    GitRef = "main",
                    Inputs = "{}",
                    UpdatedAt = DateTime.UtcNow,
                });
            }
            await db.SaveChangesAsync();
        }

        var admin = _fixture.CreateAdminClient();
        var resp = await admin.GetAsync("/api/admin/canary/history?limit=2");
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        var body = JsonDocument.Parse(await resp.Content.ReadAsStringAsync()).RootElement;
        Assert.Equal(2, body.GetProperty("items").GetArrayLength());
    }

    // ── Live runs: degrade cleanly with no token (the CI/lab state) ─────────

    [Fact]
    public async Task Runs_without_a_token_report_unconfigured_not_an_error()
    {
        var admin = _fixture.CreateAdminClient();
        var resp = await admin.GetAsync("/api/admin/canary/runs");
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);

        var body = JsonDocument.Parse(await resp.Content.ReadAsStringAsync()).RootElement;
        Assert.False(body.GetProperty("configured").GetBoolean());
        Assert.Equal(0, body.GetProperty("runs").GetArrayLength());
    }

    // ── Dispatch still 409s cleanly with no token (now that it takes a DbContext) ──

    [Fact]
    public async Task Dispatch_without_a_token_is_a_clear_409_and_persists_nothing()
    {
        int before;
        await using (var db = _fixture.NewDbContext())
        {
            before = await db.CanaryDispatches.CountAsync();
        }

        var admin = _fixture.CreateAdminClient();
        var resp = await admin.PostAsync("/api/admin/canary/dispatch", null);
        Assert.Equal(HttpStatusCode.Conflict, resp.StatusCode);

        await using (var db2 = _fixture.NewDbContext())
        {
            Assert.Equal(before, await db2.CanaryDispatches.CountAsync());
        }
    }
}
