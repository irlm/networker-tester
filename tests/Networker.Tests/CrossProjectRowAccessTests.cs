using System.Net;
using Networker.Data.Entities;

namespace Networker.Tests;

/// <summary>
/// IDOR / cross-project isolation on the FLAT per-row routes
/// (<c>/api/v2/test-configs/{id}</c>, <c>/api/v2/schedules/{id}</c>) — the routes
/// that carry no <c>{projectId}</c> in the path and must therefore resolve the
/// row first, then gate on the row's own project. Isolation is *implemented*
/// across the endpoints but was *auto-tested* on only a handful of modules
/// (coverage-controlplane-2026-07.md): a future dropped <c>project_id</c> check
/// on a flat route would leak another project's data and pass CI silently.
///
/// Each test seeds a SECOND project's row and confirms the seeded operator (a
/// member of <see cref="ControlPlaneFixture.SeededProjectId"/> ONLY) gets a flat
/// 404 — never the foreign row. A positive control reads the operator's OWN
/// config through the same route, so a 404 proves *denial*, not a broken route.
/// </summary>
public sealed class CrossProjectRowAccessTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fixture;

    public CrossProjectRowAccessTests(ControlPlaneFixture fixture) => _fixture = fixture;

    // Seed a fresh foreign project + a config + a schedule in it (all fresh
    // GUIDs / a unique 14-char project id) that the seeded operator has no
    // membership in.
    private (Guid configId, Guid scheduleId) SeedForeignProjectRows()
    {
        var pid = "pf" + Guid.NewGuid().ToString("N")[..12];
        var configId = Guid.NewGuid();
        var scheduleId = Guid.NewGuid();
        var now = new DateTime(2026, 1, 1, 0, 0, 0, DateTimeKind.Utc);

        using var ctx = _fixture.NewDbContext();
        ctx.Projects.Add(new Project
        {
            ProjectId = pid,
            Name = "Foreign Project",
            Slug = pid,
            Settings = "{}",
            CreatedAt = now,
            UpdatedAt = now,
            DeleteProtection = false,
        });
        ctx.TestConfigs.Add(new TestConfig
        {
            Id = configId,
            ProjectId = pid,
            Name = "foreign-config",
            EndpointKind = "network",
            EndpointRef = "{}",
            Workload = "{}",
            MaxDurationSecs = 60,
            CreatedAt = now,
            UpdatedAt = now,
        });
        ctx.TestSchedules.Add(new TestSchedule
        {
            Id = scheduleId,
            TestConfigId = configId,
            ProjectId = pid,
            CronExpr = "0 0 * * *",
            Timezone = "UTC",
            Enabled = true,
            CreatedAt = now,
        });
        ctx.SaveChanges();
        return (configId, scheduleId);
    }

    [Fact]
    public async Task Foreign_test_config_is_404_via_the_flat_route()
    {
        var (foreignConfigId, _) = SeedForeignProjectRows();
        var client = _fixture.CreateAuthenticatedClient(); // member of SeededProjectId only

        var resp = await client.GetAsync($"/api/v2/test-configs/{foreignConfigId}");

        Assert.Equal(HttpStatusCode.NotFound, resp.StatusCode);
    }

    [Fact]
    public async Task Foreign_schedule_is_404_via_the_flat_route()
    {
        var (_, foreignScheduleId) = SeedForeignProjectRows();
        var client = _fixture.CreateAuthenticatedClient();

        var resp = await client.GetAsync($"/api/v2/schedules/{foreignScheduleId}");

        Assert.Equal(HttpStatusCode.NotFound, resp.StatusCode);
    }

    [Fact]
    public async Task Own_test_config_is_readable_via_the_flat_route()
    {
        // Positive control: the SAME route returns the operator's own config, so
        // the 404s above are cross-project DENIAL, not a route that 404s always.
        var client = _fixture.CreateAuthenticatedClient();

        var resp = await client.GetAsync($"/api/v2/test-configs/{ControlPlaneFixture.SeededConfigId}");

        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
    }
}
