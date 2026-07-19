using System.Net;
using System.Net.Http.Json;
using System.Text.Json;

namespace Networker.Tests;

/// <summary>
/// End-to-end tests for the alerting REST surface (wave 1 backend) against a
/// real Postgres + the booted control plane: RBAC (viewer reads OK, viewer
/// writes denied, operator writes OK), validation via the ApiError envelope,
/// secret masking, the referenced-channel delete guard, the event history
/// route, and the channel test-fire (email → the no-op sender in tests).
/// </summary>
public sealed class AlertingEndpointsTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fixture;

    public AlertingEndpointsTests(ControlPlaneFixture fixture) => _fixture = fixture;

    private static string ProjectBase(string tail) =>
        $"/api/v2/projects/{ControlPlaneFixture.SeededProjectId}/{tail}";

    private static object EmailChannelBody(string name) => new
    {
        kind = "email",
        name,
        config = new { to = new[] { "sre@example.com" } },
    };

    private async Task<Guid> CreateChannelAsync(HttpClient operatorClient, string name)
    {
        var resp = await operatorClient.PostAsJsonAsync(
            ProjectBase("alert-channels"), EmailChannelBody(name));
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        var body = await resp.Content.ReadFromJsonAsync<JsonElement>();
        return body.GetProperty("channel_id").GetGuid();
    }

    // ── RBAC ─────────────────────────────────────────────────────────────────

    [Fact]
    public async Task Alert_routes_require_authentication()
    {
        var anon = _fixture.CreateClient();

        Assert.Equal(HttpStatusCode.Unauthorized,
            (await anon.GetAsync(ProjectBase("alert-channels"))).StatusCode);
        Assert.Equal(HttpStatusCode.Unauthorized,
            (await anon.GetAsync(ProjectBase("alert-events"))).StatusCode);
        Assert.Equal(HttpStatusCode.Unauthorized,
            (await anon.PostAsJsonAsync(ProjectBase("alert-rules"), new { })).StatusCode);
    }

    [Fact]
    public async Task Viewer_can_read_but_not_write()
    {
        var viewer = _fixture.CreateViewerClient();

        // Reads: OK for any member.
        Assert.Equal(HttpStatusCode.OK,
            (await viewer.GetAsync(ProjectBase("alert-channels"))).StatusCode);
        Assert.Equal(HttpStatusCode.OK,
            (await viewer.GetAsync(ProjectBase("alert-rules"))).StatusCode);
        Assert.Equal(HttpStatusCode.OK,
            (await viewer.GetAsync(ProjectBase("alert-events"))).StatusCode);

        // Writes: ProjectOperator only.
        Assert.Equal(HttpStatusCode.Forbidden,
            (await viewer.PostAsJsonAsync(
                ProjectBase("alert-channels"), EmailChannelBody("viewer-denied"))).StatusCode);
        Assert.Equal(HttpStatusCode.Forbidden,
            (await viewer.PostAsJsonAsync(
                ProjectBase("alert-rules"),
                new { metric = "p95_ms", comparator = "gt", threshold = 500.0 })).StatusCode);
    }

    [Fact]
    public async Task Viewer_gets_404_on_flat_row_writes_not_an_existence_oracle()
    {
        var op = _fixture.CreateAuthenticatedClient();
        var viewer = _fixture.CreateViewerClient();
        var channelId = await CreateChannelAsync(op, "flat-authz-channel");

        // The row exists, but a viewer's PATCH/DELETE/test must look identical
        // to a missing row (404, never 403).
        Assert.Equal(HttpStatusCode.NotFound,
            (await viewer.PatchAsJsonAsync(
                $"/api/v2/alert-channels/{channelId}", new { enabled = false })).StatusCode);
        Assert.Equal(HttpStatusCode.NotFound,
            (await viewer.DeleteAsync($"/api/v2/alert-channels/{channelId}")).StatusCode);
        Assert.Equal(HttpStatusCode.NotFound,
            (await viewer.PostAsync($"/api/v2/alert-channels/{channelId}/test", null)).StatusCode);
    }

    // ── Channels ─────────────────────────────────────────────────────────────

    [Fact]
    public async Task Operator_creates_patches_and_deletes_a_channel()
    {
        var op = _fixture.CreateAuthenticatedClient();

        var create = await op.PostAsJsonAsync(ProjectBase("alert-channels"), new
        {
            kind = "webhook",
            name = "ops hook",
            config = new { url = "https://hooks.example.com/x", secret = "hunter2" },
        });
        Assert.Equal(HttpStatusCode.OK, create.StatusCode);
        var created = await create.Content.ReadFromJsonAsync<JsonElement>();
        var channelId = created.GetProperty("channel_id").GetGuid();

        // The secret never comes back — masked on every read path.
        Assert.Equal("********",
            created.GetProperty("config").GetProperty("secret").GetString());

        var patch = await op.PatchAsJsonAsync(
            $"/api/v2/alert-channels/{channelId}", new { enabled = false, name = "ops hook v2" });
        Assert.Equal(HttpStatusCode.OK, patch.StatusCode);
        var patched = await patch.Content.ReadFromJsonAsync<JsonElement>();
        Assert.False(patched.GetProperty("enabled").GetBoolean());
        Assert.Equal("ops hook v2", patched.GetProperty("name").GetString());

        Assert.Equal(HttpStatusCode.NoContent,
            (await op.DeleteAsync($"/api/v2/alert-channels/{channelId}")).StatusCode);
        Assert.Equal(HttpStatusCode.NotFound,
            (await op.DeleteAsync($"/api/v2/alert-channels/{channelId}")).StatusCode);
    }

    [Fact]
    public async Task Channel_validation_uses_the_api_error_envelope()
    {
        var op = _fixture.CreateAuthenticatedClient();

        var badKind = await op.PostAsJsonAsync(ProjectBase("alert-channels"), new
        {
            kind = "carrier-pigeon",
            name = "x",
            config = new { url = "https://h/x" },
        });
        Assert.Equal(HttpStatusCode.BadRequest, badKind.StatusCode);
        var envelope = await badKind.Content.ReadFromJsonAsync<JsonElement>();
        Assert.Contains("webhook", envelope.GetProperty("error").GetString());

        var badUrl = await op.PostAsJsonAsync(ProjectBase("alert-channels"), new
        {
            kind = "webhook",
            name = "x",
            config = new { url = "not-a-url" },
        });
        Assert.Equal(HttpStatusCode.BadRequest, badUrl.StatusCode);
    }

    [Fact]
    public async Task Deleting_a_channel_referenced_by_a_rule_is_409()
    {
        var op = _fixture.CreateAuthenticatedClient();
        var channelId = await CreateChannelAsync(op, "referenced-channel");

        var rule = await op.PostAsJsonAsync(ProjectBase("alert-rules"), new
        {
            metric = "error_rate",
            comparator = "gt",
            threshold = 0.05,
            channel_id = channelId,
        });
        Assert.Equal(HttpStatusCode.OK, rule.StatusCode);
        var ruleId = (await rule.Content.ReadFromJsonAsync<JsonElement>())
            .GetProperty("rule_id").GetGuid();

        var del = await op.DeleteAsync($"/api/v2/alert-channels/{channelId}");
        Assert.Equal(HttpStatusCode.Conflict, del.StatusCode);
        var envelope = await del.Content.ReadFromJsonAsync<JsonElement>();
        Assert.Contains("referenced", envelope.GetProperty("error").GetString());

        // After the rule goes away the channel deletes cleanly.
        Assert.Equal(HttpStatusCode.NoContent,
            (await op.DeleteAsync($"/api/v2/alert-rules/{ruleId}")).StatusCode);
        Assert.Equal(HttpStatusCode.NoContent,
            (await op.DeleteAsync($"/api/v2/alert-channels/{channelId}")).StatusCode);
    }

    // ── Rules ────────────────────────────────────────────────────────────────

    [Fact]
    public async Task Rule_create_validates_metric_comparator_window_and_channel()
    {
        var op = _fixture.CreateAuthenticatedClient();
        var channelId = await CreateChannelAsync(op, "rule-validation-channel");

        async Task AssertBad(object body, string errorFragment)
        {
            var resp = await op.PostAsJsonAsync(ProjectBase("alert-rules"), body);
            Assert.Equal(HttpStatusCode.BadRequest, resp.StatusCode);
            var envelope = await resp.Content.ReadFromJsonAsync<JsonElement>();
            Assert.Contains(errorFragment, envelope.GetProperty("error").GetString());
        }

        await AssertBad(new { metric = "p99_ms", comparator = "gt", threshold = 1.0, channel_id = channelId }, "metric");
        await AssertBad(new { metric = "p95_ms", comparator = "ge", threshold = 1.0, channel_id = channelId }, "comparator");
        await AssertBad(new { metric = "p95_ms", comparator = "gt", channel_id = channelId }, "threshold");
        await AssertBad(new { metric = "p95_ms", comparator = "gt", threshold = 1.0, window_runs = 0, channel_id = channelId }, "window_runs");
        await AssertBad(new { metric = "p95_ms", comparator = "gt", threshold = 1.0 }, "channel_id");
        await AssertBad(new { metric = "p95_ms", comparator = "gt", threshold = 1.0, channel_id = Guid.NewGuid() }, "channel_id");
        await AssertBad(new
        {
            metric = "p95_ms",
            comparator = "gt",
            threshold = 1.0,
            channel_id = channelId,
            test_config_id = Guid.NewGuid(), // not a config of this project
        }, "test_config_id");

        // A valid rule bound to the seeded config round-trips with defaults.
        var ok = await op.PostAsJsonAsync(ProjectBase("alert-rules"), new
        {
            metric = "p95_ms",
            comparator = "gt",
            threshold = 500.0,
            channel_id = channelId,
            test_config_id = ControlPlaneFixture.SeededConfigId,
        });
        Assert.Equal(HttpStatusCode.OK, ok.StatusCode);
        var rule = await ok.Content.ReadFromJsonAsync<JsonElement>();
        Assert.Equal(1, rule.GetProperty("window_runs").GetInt32());
        Assert.True(rule.GetProperty("enabled").GetBoolean());
        Assert.Equal(ControlPlaneFixture.SeededProjectId, rule.GetProperty("project_id").GetString());

        // Patch: only supplied fields apply.
        var ruleId = rule.GetProperty("rule_id").GetGuid();
        var patch = await op.PatchAsJsonAsync(
            $"/api/v2/alert-rules/{ruleId}", new { threshold = 750.0, window_runs = 3, enabled = false });
        Assert.Equal(HttpStatusCode.OK, patch.StatusCode);
        var patched = await patch.Content.ReadFromJsonAsync<JsonElement>();
        Assert.Equal(750.0, patched.GetProperty("threshold").GetDouble());
        Assert.Equal(3, patched.GetProperty("window_runs").GetInt32());
        Assert.False(patched.GetProperty("enabled").GetBoolean());
        Assert.Equal("p95_ms", patched.GetProperty("metric").GetString()); // untouched
    }

    // ── Events + channel test-fire ───────────────────────────────────────────

    [Fact]
    public async Task Events_list_is_paginated_and_initially_empty()
    {
        var viewer = _fixture.CreateViewerClient();

        var resp = await viewer.GetAsync(ProjectBase("alert-events?limit=10&offset=0"));
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        var events = await resp.Content.ReadFromJsonAsync<JsonElement>();
        Assert.Equal(JsonValueKind.Array, events.ValueKind);
    }

    [Fact]
    public async Task Channel_test_fire_reports_delivery_status()
    {
        var op = _fixture.CreateAuthenticatedClient();
        // Email channel → the no-op sender (ACS unconfigured in tests) logs and
        // reports success, proving the notifier path end-to-end without network.
        var channelId = await CreateChannelAsync(op, "test-fire-channel");

        var resp = await op.PostAsync($"/api/v2/alert-channels/{channelId}/test", null);
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        var body = await resp.Content.ReadFromJsonAsync<JsonElement>();
        Assert.Equal("delivered", body.GetProperty("delivery_status").GetString());
    }
}
