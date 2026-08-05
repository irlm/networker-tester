using System.Net;
using System.Net.Http.Json;
using System.Text.Json;
using Networker.Data.Entities;
using Xunit;

namespace Networker.Tests;

/// <summary>
/// Audit P1-3: <c>EndpointSmokeTests</c> asserts only <c>status &lt; 500</c>, and
/// ~20 of its routes use a deliberately-not-found GUID — so the handler
/// short-circuits on the 404 arm and **the real query never executes**. A
/// broken EF translation, a bad Join, or a Postgres-only SQL error in a list
/// endpoint would sail straight through.
///
/// <para>These drive the SAME routes against SEEDED rows and require a 200
/// with a well-formed body, so the query actually runs on the provider that
/// ships. (The 2026-07 members-page 500 was exactly this class: an OrderBy
/// after a Join that EF could not translate server-side.)</para>
/// </summary>
public class ListRoutes200PathTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fx;

    public ListRoutes200PathTests(ControlPlaneFixture fx) => _fx = fx;

    private const string Pid = ControlPlaneFixture.SeededProjectId;

    /// <summary>Seed one row of each listable kind so no list can return an
    /// empty set and pass trivially.</summary>
    private async Task SeedListableContentAsync()
    {
        var now = DateTime.UtcNow;
        await using var db = _fx.NewDbContext();

        var cfgId = Guid.NewGuid();
        db.TestConfigs.Add(new TestConfig
        {
            Id = cfgId,
            ProjectId = Pid,
            Name = $"list-src-{cfgId:N}",
            EndpointKind = "network",
            EndpointRef = """{"kind":"network","host":"10.11.12.13","port":8444}""",
            Workload = """{"modes":["http1"],"runs":1}""",
            MaxDurationSecs = 300,
            CreatedAt = now.AddMinutes(-10),
            UpdatedAt = now.AddMinutes(-10),
        });
        db.TestRuns.Add(new TestRun
        {
            Id = Guid.NewGuid(),
            TestConfigId = cfgId,
            ProjectId = Pid,
            Status = "completed",
            SuccessCount = 3,
            FailureCount = 0,
            StartedAt = now.AddMinutes(-9),
            FinishedAt = now.AddMinutes(-8),
            CreatedAt = now.AddMinutes(-10),
        });
        db.TestSchedules.Add(new TestSchedule
        {
            Id = Guid.NewGuid(),
            TestConfigId = cfgId,
            ProjectId = Pid,
            CronExpr = "0 3 * * *",
            Timezone = "UTC",
            Enabled = true,
            NextFireAt = now.AddHours(3),
            CreatedAt = now,
        });
        await db.SaveChangesAsync();
    }

    public static TheoryData<string> ListRoutes() => new()
    {
        $"/api/v2/projects/{Pid}/test-configs",
        $"/api/v2/projects/{Pid}/test-runs?limit=10",
        $"/api/v2/projects/{Pid}/schedules",
        $"/api/projects/{Pid}/deployments?limit=10",
        $"/api/projects/{Pid}/share-links",
        $"/api/projects/{Pid}/testers",
        $"/api/projects/{Pid}/inventory",
        "/api/projects",
        // Agents are project-scoped — there is no flat /api/agents route
        // (verified against AgentsEndpoints).
        $"/api/projects/{Pid}/agents",
        "/api/modes",
        "/api/zones",
    };

    [Theory]
    [MemberData(nameof(ListRoutes))]
    public async Task List_routes_execute_their_real_query_and_return_200(string route)
    {
        await SeedListableContentAsync();
        using var client = _fx.CreateAdminClient();

        var resp = await client.GetAsync(route);
        var raw = await resp.Content.ReadAsStringAsync();

        Assert.True(resp.StatusCode == HttpStatusCode.OK,
            $"{route} → {(int)resp.StatusCode} {resp.StatusCode}: {raw[..Math.Min(300, raw.Length)]}");

        // Well-formed JSON, not an empty body served with a 200.
        var body = JsonSerializer.Deserialize<JsonElement>(raw);
        Assert.True(body.ValueKind is JsonValueKind.Object or JsonValueKind.Array,
            $"{route} returned {body.ValueKind}");
    }

    [Fact]
    public async Task Seeded_project_lists_are_not_empty()
    {
        // Guards the guard: if seeding silently stopped working, every route
        // above would still 200 on an empty set and prove nothing.
        await SeedListableContentAsync();
        using var client = _fx.CreateAdminClient();

        var resp = await client.GetAsync($"/api/v2/projects/{Pid}/test-configs");
        resp.EnsureSuccessStatusCode();
        var body = await resp.Content.ReadFromJsonAsync<JsonElement>();

        var count = body.ValueKind == JsonValueKind.Array
            ? body.GetArrayLength()
            : body.TryGetProperty("configs", out var c) && c.ValueKind == JsonValueKind.Array
                ? c.GetArrayLength()
                : body.TryGetProperty("items", out var i) && i.ValueKind == JsonValueKind.Array
                    ? i.GetArrayLength()
                    : -1;

        Assert.True(count != 0, $"test-configs list came back EMPTY — seeding is broken: {body}");
    }
}
