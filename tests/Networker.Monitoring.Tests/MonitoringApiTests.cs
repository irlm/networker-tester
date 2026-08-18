using System.Net;
using System.Net.Http.Json;
using System.Text.Json;

namespace Networker.Monitoring.Tests;

[Collection(MonitoringCollection.Name)]
public sealed class MonitoringApiTests(MonitoringFixture fixture)
{
    [Fact]
    public async Task Health_is_public_and_reports_independent_database()
    {
        using var client = fixture.CreateClient();

        var response = await client.GetAsync("/health");
        var body = await response.Content.ReadFromJsonAsync<JsonElement>();

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        Assert.Equal("ok", body.GetProperty("status").GetString());
        Assert.Equal("ok", body.GetProperty("database").GetString());
        Assert.Equal("disabled", body.GetProperty("scheduler").GetString());
    }

    [Fact]
    public async Task Management_routes_require_monitoring_key()
    {
        using var client = fixture.CreateClient();

        var response = await client.GetAsync("/api/v1/locations");

        Assert.Equal(HttpStatusCode.Unauthorized, response.StatusCode);
    }

    [Fact]
    public async Task Create_list_update_and_delete_monitor_round_trip()
    {
        using var client = CreateAuthenticatedClient();
        var projectId = $"project-{Guid.NewGuid():N}";
        var create = await client.PostAsJsonAsync($"/api/v1/projects/{projectId}/monitors", new
        {
            name = "Public health",
            target_url = "https://example.com/health",
            method = "GET",
            assertions = new
            {
                status_min = 200,
                status_max = 299,
                warning_latency_ms = 400,
                critical_latency_ms = 1000,
            },
            interval_seconds = 60,
            timeout_ms = 5000,
            criticality = "reporting",
        });
        var created = await create.Content.ReadFromJsonAsync<JsonElement>();

        Assert.Equal(HttpStatusCode.Created, create.StatusCode);
        var monitorId = created.GetProperty("monitor_id").GetGuid();
        Assert.Equal("reporting", created.GetProperty("criticality").GetString());
        Assert.Equal("https://example.com/health", created.GetProperty("target_url").GetString());

        var list = await client.GetFromJsonAsync<JsonElement>($"/api/v1/projects/{projectId}/monitors");
        Assert.Single(list.GetProperty("items").EnumerateArray());

        var update = await client.PatchAsJsonAsync($"/api/v1/monitors/{monitorId}", new
        {
            enabled = false,
            criticality = "critical",
        });
        var updated = await update.Content.ReadFromJsonAsync<JsonElement>();
        Assert.Equal(HttpStatusCode.OK, update.StatusCode);
        Assert.False(updated.GetProperty("enabled").GetBoolean());
        Assert.Equal("critical", updated.GetProperty("criticality").GetString());

        var delete = await client.DeleteAsync($"/api/v1/monitors/{monitorId}");
        Assert.Equal(HttpStatusCode.NoContent, delete.StatusCode);
        var missing = await client.GetAsync($"/api/v1/monitors/{monitorId}");
        Assert.Equal(HttpStatusCode.NotFound, missing.StatusCode);
    }

    [Fact]
    public async Task Create_rejects_unsafe_shape_before_scheduling()
    {
        using var client = CreateAuthenticatedClient();

        var response = await client.PostAsJsonAsync(
            $"/api/v1/projects/project-{Guid.NewGuid():N}/monitors",
            new
            {
                name = "Bad monitor",
                target_url = "ftp://user:secret@example.com/file",
                method = "POST",
                interval_seconds = 5,
            });

        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
    }

    private HttpClient CreateAuthenticatedClient()
    {
        var client = fixture.CreateClient();
        client.DefaultRequestHeaders.Add(Networker.Monitoring.MonitoringApiKey.HeaderName, MonitoringFixture.ApiKey);
        return client;
    }
}
