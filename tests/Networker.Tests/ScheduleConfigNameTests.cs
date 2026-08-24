using System.Net;
using System.Text;
using System.Text.Json;
using Microsoft.Extensions.DependencyInjection;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.Tests;

/// <summary>
/// The schedules API must say WHAT each schedule runs.
///
/// <para>Its DTO never carried <c>config_name</c>, and the schedules page
/// renders <c>config_name || 'Unnamed'</c> — so every schedule on every
/// deployment displayed as "Unnamed" and an operator could not tell one from
/// another (reported from prod 2026-08-24, where two active schedules were
/// indistinguishable). The field was simply absent from the payload, which is
/// why no amount of looking at the UI explained it.</para>
/// </summary>
public sealed class ScheduleConfigNameTests(ControlPlaneFixture fx) : IClassFixture<ControlPlaneFixture>
{
    // test_config carries a UNIQUE (project_id, name) constraint, so each test
    // seeds its own name — two tests sharing one collided with 23505.
    private static string UniqueName(string suffix) => $"nightly-latency-sweep-{suffix}";

    private (Guid ScheduleId, string ConfigName) SeedScheduleForNamedConfig(string suffix)
    {
        var configName = UniqueName(suffix);
        using var scope = fx.Services.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
        var now = DateTime.UtcNow;

        var configId = Guid.NewGuid();
        db.TestConfigs.Add(new TestConfig
        {
            Id = configId,
            ProjectId = ControlPlaneFixture.SeededProjectId,
            Name = configName,
            EndpointKind = "network",
            EndpointRef = "{}",
            Workload = "{}",
            MaxDurationSecs = 60,
            CreatedAt = now,
            UpdatedAt = now,
        });
        var scheduleId = Guid.NewGuid();
        db.TestSchedules.Add(new TestSchedule
        {
            Id = scheduleId,
            TestConfigId = configId,
            ProjectId = ControlPlaneFixture.SeededProjectId,
            CronExpr = "0 3 * * *",
            Timezone = "UTC",
            Enabled = true,
            CreatedAt = now,
        });
        db.SaveChanges();
        return (scheduleId, configName);
    }

    [Fact]
    public async Task The_list_names_the_config_each_schedule_runs()
    {
        var (scheduleId, configName) = SeedScheduleForNamedConfig("list");
        var client = fx.CreateAuthenticatedClient();

        var resp = await client.GetAsync(
            $"/api/v2/projects/{ControlPlaneFixture.SeededProjectId}/schedules");
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);

        using var doc = JsonDocument.Parse(await resp.Content.ReadAsStringAsync());
        var mine = doc.RootElement.EnumerateArray()
            .Single(e => e.GetProperty("id").GetGuid() == scheduleId);

        // The field must be PRESENT and populated — absent is what produced
        // "Unnamed", and an absent field reads the same as an empty one in JS.
        Assert.True(mine.TryGetProperty("config_name", out var name),
            "config_name is missing from the schedules payload");
        Assert.Equal(configName, name.GetString());
    }

    [Fact]
    public async Task The_detail_route_names_it_too()
    {
        var (scheduleId, configName) = SeedScheduleForNamedConfig("detail");
        var client = fx.CreateAuthenticatedClient();

        var resp = await client.GetAsync($"/api/v2/schedules/{scheduleId}");
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);

        using var doc = JsonDocument.Parse(await resp.Content.ReadAsStringAsync());
        Assert.Equal(configName, doc.RootElement.GetProperty("config_name").GetString());
    }

    [Fact]
    public async Task A_freshly_created_schedule_comes_back_named()
    {
        // Otherwise the row the UI just added renders "Unnamed" until the next
        // list refresh — the create and patch handlers hold a schedule row but
        // no projection, so they need their own lookup.
        var client = fx.CreateAuthenticatedClient();
        Guid configId;
        using (var scope = fx.Services.CreateScope())
        {
            var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
            configId = Guid.NewGuid();
            var now = DateTime.UtcNow;
            db.TestConfigs.Add(new TestConfig
            {
                Id = configId,
                ProjectId = ControlPlaneFixture.SeededProjectId,
                Name = "created-by-test",
                EndpointKind = "network",
                EndpointRef = "{}",
                Workload = "{}",
                MaxDurationSecs = 60,
                CreatedAt = now,
                UpdatedAt = now,
            });
            db.SaveChanges();
        }

        var body = new StringContent(
            $$"""{"test_config_id":"{{configId}}","cron_expr":"15 4 * * *","timezone":"UTC"}""",
            Encoding.UTF8, "application/json");
        var resp = await client.PostAsync(
            $"/api/v2/projects/{ControlPlaneFixture.SeededProjectId}/schedules", body);
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);

        using var doc = JsonDocument.Parse(await resp.Content.ReadAsStringAsync());
        Assert.Equal("created-by-test", doc.RootElement.GetProperty("config_name").GetString());
    }
}
