using System.Net;
using System.Net.Http.Json;
using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.Data.Entities;

namespace Networker.Tests;

/// <summary>
/// WRITE-surface coverage for the control plane (POST/PUT/PATCH/DELETE): the
/// write-path analogue of <see cref="ControlPlaneIntegrationTests"/>. Each safe,
/// pure-DB write endpoint is exercised with a well-formed body against the real
/// Postgres + in-process app (<see cref="ControlPlaneFixture"/>), asserting the
/// EXACT success status the handler returns and — where cheap — the persisted
/// side effect via <see cref="ControlPlaneFixture.NewDbContext"/>. A green
/// compile is not enough: this catches an endpoint that 500s on a valid request
/// (EF save failure, untranslatable read-back, null-ref, bad column map).
///
/// Every test is self-contained: rows are created with a fresh Guid / unique
/// name so the shared container DB doesn't collide across the class's
/// sequentially-run methods. DELETE/PUT/PATCH tests create their target row in
/// the same test before mutating it.
///
/// <para><b>Auth.</b> Writes use <c>CreateAdminClient()</c> — the seeded user is
/// global <c>admin</c> AND project-admin of <see cref="ControlPlaneFixture.SeededProjectId"/>,
/// so it clears the ProjectOperator / ProjectAdmin / GlobalAdmin gates. Two
/// authz-negatives use <c>CreateViewerClient()</c> to prove operator/admin
/// writes are 403 for a read-only member.</para>
///
/// <para><b>Excluded (with rationale) — NOT tested here:</b></para>
/// <list type="bullet">
///   <item>Provisioning / cloud / external side effects: POST /testers (+
///   /precheck), deployments/{id}/start|stop|check, cloud-accounts|connections/{id}/validate,
///   benchmark-catalog/{vmId}/detect, agents/{id}/commands, /api/admin/smoke-test
///   — they shell out to az/aws/gcloud or an agent, absent in CI.</item>
///   <item>Dispatch / side-effectful or already-covered: test-configs/{id}/launch
///   (covered by ControlPlaneIntegrationTests.Launch_creates_a_queued_run...),
///   comparison-groups/{id}/launch (202 shell — no DB effect to assert),
///   schedules/{id}/trigger, test-runs/{id}/cancel — dispatch-side.</item>
///   <item>501-by-design: /api/update/dashboard, /api/update/tester,
///   testers/{id}/upgrade — covered by the honest-501 tests already.</item>
///   <item>Fixture-breaking mutations of shared seed state: DELETE
///   /api/projects/{pid} + /api/admin/workspaces/* (would soft-delete/suspend the
///   seeded project the whole class depends on); /api/auth/change|forgot|reset-password
///   and /api/users/{id}/approve|deny|disable|role (mutate seeded auth/users other
///   tests rely on). POST /api/projects (create) is exercised indirectly via the
///   throwaway project seeded for the members role-change test, but the raw create
///   endpoint is GlobalAdmin-only and its ID generator is unit-tested separately.</item>
///   <item>cloud-accounts create — already covered by
///   ControlPlaneIntegrationTests.Cloud_account_create_encrypts_credentials...</item>
///   <item>members/import + members/send-invites — pure-DB but their bodies mint
///   placeholder users / workspace-invite rows into shared tables; add-member +
///   role-change below cover the members write path with tighter, non-colliding
///   assertions.</item>
/// </list>
/// </summary>
public sealed class WriteEndpointCoverageTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fixture;

    public WriteEndpointCoverageTests(ControlPlaneFixture fixture) => _fixture = fixture;

    private const string Pid = ControlPlaneFixture.SeededProjectId;

    private static string Uniq(string prefix) => $"{prefix}-{Guid.NewGuid():N}";

    private static async Task<string> Body(HttpResponseMessage resp) =>
        await resp.Content.ReadAsStringAsync();

    // ── Helper: create a test config via the API, return its id ────────────────
    private async Task<Guid> CreateTestConfigAsync(HttpClient client, string name)
    {
        var body = new
        {
            name,
            endpoint = new { kind = "network", host = "https://example.com" },
            workload = new { modes = new[] { "http11" }, runs = 3 },
            max_duration_secs = 60,
        };
        var resp = await client.PostAsJsonAsync($"/api/v2/projects/{Pid}/test-configs", body);
        Assert.True(resp.StatusCode == HttpStatusCode.OK,
            $"POST /api/v2/projects/{{pid}}/test-configs (helper) → {(int)resp.StatusCode}; body: {await Body(resp)}");
        using var doc = JsonDocument.Parse(await Body(resp));
        return doc.RootElement.GetProperty("id").GetGuid();
    }

    // ── Helper: create an alert channel via the API, return its id ─────────────
    private async Task<Guid> CreateAlertChannelAsync(HttpClient client, string name)
    {
        var body = new
        {
            kind = "webhook",
            name,
            config = new { url = "https://hooks.example.com/x" },
            enabled = true,
        };
        var resp = await client.PostAsJsonAsync($"/api/v2/projects/{Pid}/alert-channels", body);
        Assert.True(resp.StatusCode == HttpStatusCode.OK,
            $"POST /api/v2/projects/{{pid}}/alert-channels (helper) → {(int)resp.StatusCode}; body: {await Body(resp)}");
        using var doc = JsonDocument.Parse(await Body(resp));
        return doc.RootElement.GetProperty("channel_id").GetGuid();
    }

    // ═══════════════════════════════════════════════════════════════════════════
    //  TEST CONFIGS — POST 200 / PATCH 200 / DELETE 204
    // ═══════════════════════════════════════════════════════════════════════════

    [Fact]
    public async Task Post_test_config_creates_row_and_returns_200()
    {
        var client = _fixture.CreateAdminClient();
        var name = Uniq("cfg-create");
        var body = new
        {
            name,
            description = "created by write-coverage test",
            endpoint = new { kind = "network", host = "https://example.com" },
            workload = new { modes = new[] { "http11" }, runs = 5 },
            max_duration_secs = 90,
        };

        var resp = await client.PostAsJsonAsync($"/api/v2/projects/{Pid}/test-configs", body);

        Assert.True(resp.StatusCode == HttpStatusCode.OK,
            $"POST /api/v2/projects/{{pid}}/test-configs → {(int)resp.StatusCode}; body: {await Body(resp)}");

        await using var ctx = _fixture.NewDbContext();
        var row = await ctx.TestConfigs.FirstOrDefaultAsync(c => c.Name == name);
        Assert.True(row is not null, $"test config '{name}' not persisted");
        Assert.Equal("network", row!.EndpointKind);
    }

    [Fact]
    public async Task Patch_test_config_updates_field_and_returns_200()
    {
        var client = _fixture.CreateAdminClient();
        var id = await CreateTestConfigAsync(client, Uniq("cfg-patch"));
        var newName = Uniq("cfg-patched");

        var resp = await client.PatchAsJsonAsync(
            $"/api/v2/test-configs/{id}",
            new { name = newName, max_duration_secs = 123 });

        Assert.True(resp.StatusCode == HttpStatusCode.OK,
            $"PATCH /api/v2/test-configs/{{id}} → {(int)resp.StatusCode}; body: {await Body(resp)}");

        await using var ctx = _fixture.NewDbContext();
        var row = await ctx.TestConfigs.FirstAsync(c => c.Id == id);
        Assert.Equal(newName, row.Name);
        Assert.Equal(123, row.MaxDurationSecs);
    }

    [Fact]
    public async Task Delete_test_config_removes_row_and_returns_204()
    {
        var client = _fixture.CreateAdminClient();
        var id = await CreateTestConfigAsync(client, Uniq("cfg-delete"));

        var resp = await client.DeleteAsync($"/api/v2/test-configs/{id}");

        Assert.True(resp.StatusCode == HttpStatusCode.NoContent,
            $"DELETE /api/v2/test-configs/{{id}} → {(int)resp.StatusCode}; body: {await Body(resp)}");

        await using var ctx = _fixture.NewDbContext();
        Assert.False(await ctx.TestConfigs.AnyAsync(c => c.Id == id),
            "test config row still present after DELETE 204");
    }

    // ═══════════════════════════════════════════════════════════════════════════
    //  SCHEDULES — POST 200 / PATCH 200 / DELETE 204
    // ═══════════════════════════════════════════════════════════════════════════

    [Fact]
    public async Task Post_schedule_creates_row_and_returns_200()
    {
        var client = _fixture.CreateAdminClient();
        var body = new
        {
            test_config_id = ControlPlaneFixture.SeededConfigId.ToString(),
            cron_expr = "0 * * * *",
            timezone = "UTC",
            enabled = true,
        };

        var resp = await client.PostAsJsonAsync($"/api/v2/projects/{Pid}/schedules", body);

        Assert.True(resp.StatusCode == HttpStatusCode.OK,
            $"POST /api/v2/projects/{{pid}}/schedules → {(int)resp.StatusCode}; body: {await Body(resp)}");

        using var doc = JsonDocument.Parse(await Body(resp));
        var id = doc.RootElement.GetProperty("id").GetGuid();
        await using var ctx = _fixture.NewDbContext();
        Assert.True(await ctx.TestSchedules.AnyAsync(s => s.Id == id),
            "schedule row not persisted");
    }

    [Fact]
    public async Task Patch_schedule_updates_field_and_returns_200()
    {
        var client = _fixture.CreateAdminClient();
        var createResp = await client.PostAsJsonAsync(
            $"/api/v2/projects/{Pid}/schedules",
            new
            {
                test_config_id = ControlPlaneFixture.SeededConfigId.ToString(),
                cron_expr = "0 0 * * *",
            });
        Assert.True(createResp.StatusCode == HttpStatusCode.OK,
            $"POST schedule (setup) → {(int)createResp.StatusCode}; body: {await Body(createResp)}");
        using var createdDoc = JsonDocument.Parse(await Body(createResp));
        var id = createdDoc.RootElement.GetProperty("id").GetGuid();

        var resp = await client.PatchAsJsonAsync(
            $"/api/v2/schedules/{id}",
            new { cron_expr = "30 2 * * *", enabled = false });

        Assert.True(resp.StatusCode == HttpStatusCode.OK,
            $"PATCH /api/v2/schedules/{{id}} → {(int)resp.StatusCode}; body: {await Body(resp)}");

        await using var ctx = _fixture.NewDbContext();
        var row = await ctx.TestSchedules.FirstAsync(s => s.Id == id);
        Assert.Equal("30 2 * * *", row.CronExpr);
        Assert.False(row.Enabled);
    }

    [Fact]
    public async Task Delete_schedule_removes_row_and_returns_204()
    {
        var client = _fixture.CreateAdminClient();
        var createResp = await client.PostAsJsonAsync(
            $"/api/v2/projects/{Pid}/schedules",
            new
            {
                test_config_id = ControlPlaneFixture.SeededConfigId.ToString(),
                cron_expr = "0 0 * * *",
            });
        Assert.True(createResp.StatusCode == HttpStatusCode.OK,
            $"POST schedule (setup) → {(int)createResp.StatusCode}; body: {await Body(createResp)}");
        using var createdDoc = JsonDocument.Parse(await Body(createResp));
        var id = createdDoc.RootElement.GetProperty("id").GetGuid();

        var resp = await client.DeleteAsync($"/api/v2/schedules/{id}");

        Assert.True(resp.StatusCode == HttpStatusCode.NoContent,
            $"DELETE /api/v2/schedules/{{id}} → {(int)resp.StatusCode}; body: {await Body(resp)}");

        await using var ctx = _fixture.NewDbContext();
        Assert.False(await ctx.TestSchedules.AnyAsync(s => s.Id == id),
            "schedule row still present after DELETE 204");
    }

    // ═══════════════════════════════════════════════════════════════════════════
    //  ALERT CHANNELS — POST 200 / PATCH 200 / DELETE 204
    // ═══════════════════════════════════════════════════════════════════════════

    [Fact]
    public async Task Post_alert_channel_creates_row_and_returns_200()
    {
        var client = _fixture.CreateAdminClient();
        var name = Uniq("chan-create");
        var body = new
        {
            kind = "webhook",
            name,
            config = new { url = "https://hooks.example.com/create" },
            enabled = true,
        };

        var resp = await client.PostAsJsonAsync($"/api/v2/projects/{Pid}/alert-channels", body);

        Assert.True(resp.StatusCode == HttpStatusCode.OK,
            $"POST /api/v2/projects/{{pid}}/alert-channels → {(int)resp.StatusCode}; body: {await Body(resp)}");

        await using var ctx = _fixture.NewDbContext();
        Assert.True(await ctx.AlertChannels.AnyAsync(c => c.Name == name),
            "alert channel not persisted");
    }

    [Fact]
    public async Task Patch_alert_channel_updates_field_and_returns_200()
    {
        var client = _fixture.CreateAdminClient();
        var id = await CreateAlertChannelAsync(client, Uniq("chan-patch"));
        var newName = Uniq("chan-patched");

        var resp = await client.PatchAsJsonAsync(
            $"/api/v2/alert-channels/{id}",
            new { name = newName, enabled = false });

        Assert.True(resp.StatusCode == HttpStatusCode.OK,
            $"PATCH /api/v2/alert-channels/{{id}} → {(int)resp.StatusCode}; body: {await Body(resp)}");

        await using var ctx = _fixture.NewDbContext();
        var row = await ctx.AlertChannels.FirstAsync(c => c.ChannelId == id);
        Assert.Equal(newName, row.Name);
        Assert.False(row.Enabled);
    }

    [Fact]
    public async Task Delete_alert_channel_removes_row_and_returns_204()
    {
        var client = _fixture.CreateAdminClient();
        var id = await CreateAlertChannelAsync(client, Uniq("chan-delete"));

        var resp = await client.DeleteAsync($"/api/v2/alert-channels/{id}");

        Assert.True(resp.StatusCode == HttpStatusCode.NoContent,
            $"DELETE /api/v2/alert-channels/{{id}} → {(int)resp.StatusCode}; body: {await Body(resp)}");

        await using var ctx = _fixture.NewDbContext();
        Assert.False(await ctx.AlertChannels.AnyAsync(c => c.ChannelId == id),
            "alert channel still present after DELETE 204");
    }

    // ═══════════════════════════════════════════════════════════════════════════
    //  ALERT RULES — POST 200 / PATCH 200 / DELETE 204
    // ═══════════════════════════════════════════════════════════════════════════

    [Fact]
    public async Task Post_alert_rule_creates_row_and_returns_200()
    {
        var client = _fixture.CreateAdminClient();
        var channelId = await CreateAlertChannelAsync(client, Uniq("chan-for-rule"));
        var body = new
        {
            metric = "p95_ms",
            comparator = "gt",
            threshold = 250.0,
            window_runs = 3,
            channel_id = channelId,
            enabled = true,
        };

        var resp = await client.PostAsJsonAsync($"/api/v2/projects/{Pid}/alert-rules", body);

        Assert.True(resp.StatusCode == HttpStatusCode.OK,
            $"POST /api/v2/projects/{{pid}}/alert-rules → {(int)resp.StatusCode}; body: {await Body(resp)}");

        using var doc = JsonDocument.Parse(await Body(resp));
        var ruleId = doc.RootElement.GetProperty("rule_id").GetGuid();
        await using var ctx = _fixture.NewDbContext();
        Assert.True(await ctx.AlertRules.AnyAsync(r => r.RuleId == ruleId),
            "alert rule not persisted");
    }

    [Fact]
    public async Task Patch_alert_rule_updates_field_and_returns_200()
    {
        var client = _fixture.CreateAdminClient();
        var channelId = await CreateAlertChannelAsync(client, Uniq("chan-for-rule-patch"));
        var createResp = await client.PostAsJsonAsync(
            $"/api/v2/projects/{Pid}/alert-rules",
            new
            {
                metric = "p95_ms",
                comparator = "gt",
                threshold = 100.0,
                window_runs = 1,
                channel_id = channelId,
            });
        Assert.True(createResp.StatusCode == HttpStatusCode.OK,
            $"POST alert-rule (setup) → {(int)createResp.StatusCode}; body: {await Body(createResp)}");
        using var createdDoc = JsonDocument.Parse(await Body(createResp));
        var ruleId = createdDoc.RootElement.GetProperty("rule_id").GetGuid();

        var resp = await client.PatchAsJsonAsync(
            $"/api/v2/alert-rules/{ruleId}",
            new { threshold = 999.0, comparator = "lt", enabled = false });

        Assert.True(resp.StatusCode == HttpStatusCode.OK,
            $"PATCH /api/v2/alert-rules/{{id}} → {(int)resp.StatusCode}; body: {await Body(resp)}");

        await using var ctx = _fixture.NewDbContext();
        var row = await ctx.AlertRules.FirstAsync(r => r.RuleId == ruleId);
        Assert.Equal(999.0, row.Threshold);
        Assert.Equal("lt", row.Comparator);
        Assert.False(row.Enabled);
    }

    [Fact]
    public async Task Delete_alert_rule_removes_row_and_returns_204()
    {
        var client = _fixture.CreateAdminClient();
        var channelId = await CreateAlertChannelAsync(client, Uniq("chan-for-rule-del"));
        var createResp = await client.PostAsJsonAsync(
            $"/api/v2/projects/{Pid}/alert-rules",
            new
            {
                metric = "error_rate",
                comparator = "gt",
                threshold = 0.1,
                window_runs = 2,
                channel_id = channelId,
            });
        Assert.True(createResp.StatusCode == HttpStatusCode.OK,
            $"POST alert-rule (setup) → {(int)createResp.StatusCode}; body: {await Body(createResp)}");
        using var createdDoc = JsonDocument.Parse(await Body(createResp));
        var ruleId = createdDoc.RootElement.GetProperty("rule_id").GetGuid();

        var resp = await client.DeleteAsync($"/api/v2/alert-rules/{ruleId}");

        Assert.True(resp.StatusCode == HttpStatusCode.NoContent,
            $"DELETE /api/v2/alert-rules/{{id}} → {(int)resp.StatusCode}; body: {await Body(resp)}");

        await using var ctx = _fixture.NewDbContext();
        Assert.False(await ctx.AlertRules.AnyAsync(r => r.RuleId == ruleId),
            "alert rule still present after DELETE 204");
    }

    // ═══════════════════════════════════════════════════════════════════════════
    //  COMPARISON GROUPS — POST 200
    // ═══════════════════════════════════════════════════════════════════════════

    [Fact]
    public async Task Post_comparison_group_creates_row_and_returns_200()
    {
        var client = _fixture.CreateAdminClient();
        var name = Uniq("cmp-create");
        var body = new
        {
            name,
            base_workload = new { modes = new[] { "http11" }, runs = 3 },
            cells = new[]
            {
                new { endpoint = new { kind = "network", host = "https://a.example.com" } },
                new { endpoint = new { kind = "network", host = "https://b.example.com" } },
            },
        };

        var resp = await client.PostAsJsonAsync($"/api/v2/projects/{Pid}/comparison-groups", body);

        Assert.True(resp.StatusCode == HttpStatusCode.OK,
            $"POST /api/v2/projects/{{pid}}/comparison-groups → {(int)resp.StatusCode}; body: {await Body(resp)}");

        await using var ctx = _fixture.NewDbContext();
        Assert.True(await ctx.ComparisonGroups.AnyAsync(g => g.Name == name),
            "comparison group not persisted");
    }

    // ═══════════════════════════════════════════════════════════════════════════
    //  SDK ENDPOINTS — POST 200 / DELETE 204
    // ═══════════════════════════════════════════════════════════════════════════

    [Fact]
    public async Task Post_sdk_endpoint_creates_row_and_returns_200()
    {
        var client = _fixture.CreateAdminClient();
        var name = Uniq("sdk-create");
        var body = new
        {
            name,
            url = "https://sdk.example.com",
            token = "s3cr3t-token",
            runs = 5,
        };

        var resp = await client.PostAsJsonAsync($"/api/projects/{Pid}/sdk-endpoints", body);

        Assert.True(resp.StatusCode == HttpStatusCode.OK,
            $"POST /api/projects/{{pid}}/sdk-endpoints → {(int)resp.StatusCode}; body: {await Body(resp)}");

        using var doc = JsonDocument.Parse(await Body(resp));
        var id = doc.RootElement.GetProperty("id").GetGuid();
        await using var ctx = _fixture.NewDbContext();
        var row = await ctx.TestConfigs.FirstOrDefaultAsync(c => c.Id == id);
        Assert.True(row is not null, "SDK endpoint (test config) not persisted");
        Assert.True(row!.TokenEnc is { Length: > 0 }, "SDK token not encrypted at rest");
    }

    [Fact]
    public async Task Delete_sdk_endpoint_removes_row_and_returns_204()
    {
        var client = _fixture.CreateAdminClient();
        var createResp = await client.PostAsJsonAsync(
            $"/api/projects/{Pid}/sdk-endpoints",
            new
            {
                name = Uniq("sdk-delete"),
                url = "https://sdk.example.com",
                token = "tok",
            });
        Assert.True(createResp.StatusCode == HttpStatusCode.OK,
            $"POST sdk-endpoint (setup) → {(int)createResp.StatusCode}; body: {await Body(createResp)}");
        using var createdDoc = JsonDocument.Parse(await Body(createResp));
        var id = createdDoc.RootElement.GetProperty("id").GetGuid();

        var resp = await client.DeleteAsync($"/api/projects/{Pid}/sdk-endpoints/{id}");

        Assert.True(resp.StatusCode == HttpStatusCode.NoContent,
            $"DELETE /api/projects/{{pid}}/sdk-endpoints/{{id}} → {(int)resp.StatusCode}; body: {await Body(resp)}");

        await using var ctx = _fixture.NewDbContext();
        Assert.False(await ctx.TestConfigs.AnyAsync(c => c.Id == id),
            "SDK endpoint still present after DELETE 204");
    }

    // ═══════════════════════════════════════════════════════════════════════════
    //  SHARE LINKS — POST 200 / PUT 200 / DELETE 200
    // ═══════════════════════════════════════════════════════════════════════════

    [Fact]
    public async Task Post_share_link_creates_row_and_returns_200()
    {
        var client = _fixture.CreateAdminClient();
        var body = new
        {
            resource_type = "run",
            resource_id = Guid.NewGuid(),
            label = "write-coverage share",
            expires_in_days = 7,
        };

        var resp = await client.PostAsJsonAsync($"/api/projects/{Pid}/share-links", body);

        Assert.True(resp.StatusCode == HttpStatusCode.OK,
            $"POST /api/projects/{{pid}}/share-links → {(int)resp.StatusCode}; body: {await Body(resp)}");

        using var doc = JsonDocument.Parse(await Body(resp));
        var linkId = Guid.Parse(doc.RootElement.GetProperty("link_id").GetString()!);
        await using var ctx = _fixture.NewDbContext();
        Assert.True(await ctx.ShareLinks.AnyAsync(s => s.LinkId == linkId),
            "share link not persisted");
    }

    [Fact]
    public async Task Put_share_link_revoke_updates_row_and_returns_200()
    {
        var client = _fixture.CreateAdminClient();
        var createResp = await client.PostAsJsonAsync(
            $"/api/projects/{Pid}/share-links",
            new { resource_type = "run", resource_id = Guid.NewGuid(), expires_in_days = 7 });
        Assert.True(createResp.StatusCode == HttpStatusCode.OK,
            $"POST share-link (setup) → {(int)createResp.StatusCode}; body: {await Body(createResp)}");
        using var createdDoc = JsonDocument.Parse(await Body(createResp));
        var linkId = Guid.Parse(createdDoc.RootElement.GetProperty("link_id").GetString()!);

        var resp = await client.PutAsJsonAsync(
            $"/api/projects/{Pid}/share-links/{linkId}",
            new { action = "revoke" });

        Assert.True(resp.StatusCode == HttpStatusCode.OK,
            $"PUT /api/projects/{{pid}}/share-links/{{id}} → {(int)resp.StatusCode}; body: {await Body(resp)}");

        await using var ctx = _fixture.NewDbContext();
        var row = await ctx.ShareLinks.FirstAsync(s => s.LinkId == linkId);
        Assert.True(row.Revoked, "share link not revoked after PUT action=revoke");
    }

    [Fact]
    public async Task Delete_share_link_removes_row_and_returns_200()
    {
        var client = _fixture.CreateAdminClient();
        var createResp = await client.PostAsJsonAsync(
            $"/api/projects/{Pid}/share-links",
            new { resource_type = "run", resource_id = Guid.NewGuid(), expires_in_days = 7 });
        Assert.True(createResp.StatusCode == HttpStatusCode.OK,
            $"POST share-link (setup) → {(int)createResp.StatusCode}; body: {await Body(createResp)}");
        using var createdDoc = JsonDocument.Parse(await Body(createResp));
        var linkId = Guid.Parse(createdDoc.RootElement.GetProperty("link_id").GetString()!);

        // DELETE share-links returns 200 { deleted: true } (NOT 204) — read the handler.
        var resp = await client.DeleteAsync($"/api/projects/{Pid}/share-links/{linkId}");

        Assert.True(resp.StatusCode == HttpStatusCode.OK,
            $"DELETE /api/projects/{{pid}}/share-links/{{id}} → {(int)resp.StatusCode}; body: {await Body(resp)}");

        await using var ctx = _fixture.NewDbContext();
        Assert.False(await ctx.ShareLinks.AnyAsync(s => s.LinkId == linkId),
            "share link still present after DELETE");
    }

    // ═══════════════════════════════════════════════════════════════════════════
    //  VISIBILITY RULES — POST 200 / DELETE 200
    // ═══════════════════════════════════════════════════════════════════════════

    [Fact]
    public async Task Post_visibility_rule_creates_row_and_returns_200()
    {
        var client = _fixture.CreateAdminClient();
        var body = new
        {
            resource_type = "run",
            resource_id = Guid.NewGuid(),
            // user_id omitted → rule applies to every member (Rust allows null).
        };

        var resp = await client.PostAsJsonAsync($"/api/projects/{Pid}/visibility-rules", body);

        Assert.True(resp.StatusCode == HttpStatusCode.OK,
            $"POST /api/projects/{{pid}}/visibility-rules → {(int)resp.StatusCode}; body: {await Body(resp)}");

        using var doc = JsonDocument.Parse(await Body(resp));
        var ruleId = doc.RootElement.GetProperty("rule_id").GetGuid();
        await using var ctx = _fixture.NewDbContext();
        Assert.True(await ctx.TestVisibilityRules.AnyAsync(r => r.RuleId == ruleId),
            "visibility rule not persisted");
    }

    [Fact]
    public async Task Delete_visibility_rule_removes_row_and_returns_200()
    {
        var client = _fixture.CreateAdminClient();
        var resourceId = Guid.NewGuid();
        var createResp = await client.PostAsJsonAsync(
            $"/api/projects/{Pid}/visibility-rules",
            new { resource_type = "run", resource_id = resourceId });
        Assert.True(createResp.StatusCode == HttpStatusCode.OK,
            $"POST visibility-rule (setup) → {(int)createResp.StatusCode}; body: {await Body(createResp)}");
        using var createdDoc = JsonDocument.Parse(await Body(createResp));
        var ruleId = createdDoc.RootElement.GetProperty("rule_id").GetGuid();

        // DELETE visibility-rules returns 200 { deleted: true } (NOT 204) — read the handler.
        var resp = await client.DeleteAsync($"/api/projects/{Pid}/visibility-rules/{ruleId}");

        Assert.True(resp.StatusCode == HttpStatusCode.OK,
            $"DELETE /api/projects/{{pid}}/visibility-rules/{{id}} → {(int)resp.StatusCode}; body: {await Body(resp)}");

        await using var ctx = _fixture.NewDbContext();
        Assert.False(await ctx.TestVisibilityRules.AnyAsync(r => r.RuleId == ruleId),
            "visibility rule still present after DELETE");
    }

    // ═══════════════════════════════════════════════════════════════════════════
    //  CLOUD CONNECTIONS — POST 200 / PUT 200 / DELETE 200 (ProjectAdmin)
    // ═══════════════════════════════════════════════════════════════════════════

    [Fact]
    public async Task Post_cloud_connection_creates_row_and_returns_200()
    {
        var client = _fixture.CreateAdminClient();
        var name = Uniq("conn-create");
        var body = new
        {
            name,
            provider = "azure",
            config = new { subscription_id = "sub-123" },
        };

        var resp = await client.PostAsJsonAsync($"/api/projects/{Pid}/cloud-connections", body);

        Assert.True(resp.StatusCode == HttpStatusCode.OK,
            $"POST /api/projects/{{pid}}/cloud-connections → {(int)resp.StatusCode}; body: {await Body(resp)}");

        using var doc = JsonDocument.Parse(await Body(resp));
        var connId = Guid.Parse(doc.RootElement.GetProperty("connection_id").GetString()!);
        await using var ctx = _fixture.NewDbContext();
        Assert.True(await ctx.CloudConnections.AnyAsync(c => c.ConnectionId == connId),
            "cloud connection not persisted");
    }

    [Fact]
    public async Task Put_cloud_connection_updates_row_and_returns_200()
    {
        var client = _fixture.CreateAdminClient();
        var createResp = await client.PostAsJsonAsync(
            $"/api/projects/{Pid}/cloud-connections",
            new { name = Uniq("conn-put"), provider = "azure", config = new { subscription_id = "sub-a" } });
        Assert.True(createResp.StatusCode == HttpStatusCode.OK,
            $"POST cloud-connection (setup) → {(int)createResp.StatusCode}; body: {await Body(createResp)}");
        using var createdDoc = JsonDocument.Parse(await Body(createResp));
        var connId = Guid.Parse(createdDoc.RootElement.GetProperty("connection_id").GetString()!);
        var newName = Uniq("conn-put-updated");

        var resp = await client.PutAsJsonAsync(
            $"/api/projects/{Pid}/cloud-connections/{connId}",
            new { name = newName, config = new { subscription_id = "sub-b" } });

        Assert.True(resp.StatusCode == HttpStatusCode.OK,
            $"PUT /api/projects/{{pid}}/cloud-connections/{{id}} → {(int)resp.StatusCode}; body: {await Body(resp)}");

        await using var ctx = _fixture.NewDbContext();
        var row = await ctx.CloudConnections.FirstAsync(c => c.ConnectionId == connId);
        Assert.Equal(newName, row.Name);
    }

    [Fact]
    public async Task Delete_cloud_connection_removes_row_and_returns_200()
    {
        var client = _fixture.CreateAdminClient();
        var createResp = await client.PostAsJsonAsync(
            $"/api/projects/{Pid}/cloud-connections",
            new { name = Uniq("conn-del"), provider = "gcp", config = new { project_id = "gcp-x" } });
        Assert.True(createResp.StatusCode == HttpStatusCode.OK,
            $"POST cloud-connection (setup) → {(int)createResp.StatusCode}; body: {await Body(createResp)}");
        using var createdDoc = JsonDocument.Parse(await Body(createResp));
        var connId = Guid.Parse(createdDoc.RootElement.GetProperty("connection_id").GetString()!);

        // DELETE cloud-connections returns 200 { deleted: true } (NOT 204) — read the handler.
        var resp = await client.DeleteAsync($"/api/projects/{Pid}/cloud-connections/{connId}");

        Assert.True(resp.StatusCode == HttpStatusCode.OK,
            $"DELETE /api/projects/{{pid}}/cloud-connections/{{id}} → {(int)resp.StatusCode}; body: {await Body(resp)}");

        await using var ctx = _fixture.NewDbContext();
        Assert.False(await ctx.CloudConnections.AnyAsync(c => c.ConnectionId == connId),
            "cloud connection still present after DELETE");
    }

    // ═══════════════════════════════════════════════════════════════════════════
    //  PROJECT SETTINGS — PUT 200
    // ═══════════════════════════════════════════════════════════════════════════

    [Fact]
    public async Task Put_project_updates_settings_and_returns_200()
    {
        var client = _fixture.CreateAdminClient();
        // Only touch name/settings on the SEEDED project (no destructive fields);
        // restore the name at the end so other tests still see the known value.
        const string originalName = "Integration Test Project";
        var probeName = Uniq("proj-put");

        var resp = await client.PutAsJsonAsync(
            $"/api/projects/{Pid}",
            new { name = probeName, settings = new { test_visibility = "all" } });

        Assert.True(resp.StatusCode == HttpStatusCode.OK,
            $"PUT /api/projects/{{pid}} → {(int)resp.StatusCode}; body: {await Body(resp)}");

        await using (var ctx = _fixture.NewDbContext())
        {
            var row = await ctx.Projects.FirstAsync(p => p.ProjectId == Pid);
            Assert.Equal(probeName, row.Name);
            Assert.Contains("test_visibility", row.Settings);
        }

        // Restore the seeded name so the class stays order-independent.
        var restore = await client.PutAsJsonAsync($"/api/projects/{Pid}", new { name = originalName });
        Assert.True(restore.StatusCode == HttpStatusCode.OK,
            $"PUT /api/projects/{{pid}} (restore) → {(int)restore.StatusCode}; body: {await Body(restore)}");
    }

    // ═══════════════════════════════════════════════════════════════════════════
    //  MEMBERS — POST add 201 / PUT role-change 200
    //  (both against a throwaway project + throwaway user seeded in-test, so the
    //   seeded members the rest of the class depends on are never touched.)
    // ═══════════════════════════════════════════════════════════════════════════

    [Fact]
    public async Task Post_member_add_returns_201_and_persists_membership()
    {
        var client = _fixture.CreateAdminClient();

        // Seed a fresh target user (add-member resolves by email → needs an
        // existing dash_user). All NOT-NULL columns set explicitly.
        var targetEmail = $"{Guid.NewGuid():N}@write-coverage.local";
        Guid targetUserId;
        await using (var seed = _fixture.NewDbContext())
        {
            targetUserId = Guid.NewGuid();
            seed.DashUsers.Add(new DashUser
            {
                UserId = targetUserId,
                Email = targetEmail,
                Role = "viewer",
                Status = "active",
                AuthProvider = "local",
                IsPlatformAdmin = false,
                MustChangePassword = false,
                SsoOnly = false,
                CreatedAt = DateTime.UtcNow,
            });
            await seed.SaveChangesAsync();
        }

        var resp = await client.PostAsJsonAsync(
            $"/api/projects/{Pid}/members",
            new { email = targetEmail, role = "operator" });

        Assert.True(resp.StatusCode == HttpStatusCode.Created,
            $"POST /api/projects/{{pid}}/members → {(int)resp.StatusCode}; body: {await Body(resp)}");

        await using var ctx = _fixture.NewDbContext();
        var member = await ctx.ProjectMembers
            .FirstOrDefaultAsync(m => m.ProjectId == Pid && m.UserId == targetUserId);
        Assert.True(member is not null, "membership not persisted after add");
        Assert.Equal("operator", member!.Role);
    }

    [Fact]
    public async Task Put_member_role_change_returns_200_and_updates_role()
    {
        var client = _fixture.CreateAdminClient();

        // Seed a throwaway user + an ACTIVE membership directly (avoid touching
        // the class's seeded members). Then flip the role via the API.
        var email = $"{Guid.NewGuid():N}@write-coverage.local";
        var userId = Guid.NewGuid();
        await using (var seed = _fixture.NewDbContext())
        {
            seed.DashUsers.Add(new DashUser
            {
                UserId = userId,
                Email = email,
                Role = "viewer",
                Status = "active",
                AuthProvider = "local",
                IsPlatformAdmin = false,
                MustChangePassword = false,
                SsoOnly = false,
                CreatedAt = DateTime.UtcNow,
            });
            seed.ProjectMembers.Add(new ProjectMember
            {
                ProjectId = Pid,
                UserId = userId,
                Role = "viewer",
                Status = "active",
                JoinedAt = DateTime.UtcNow,
            });
            await seed.SaveChangesAsync();
        }

        var resp = await client.PutAsJsonAsync(
            $"/api/projects/{Pid}/members/{userId}",
            new { role = "operator" });

        Assert.True(resp.StatusCode == HttpStatusCode.OK,
            $"PUT /api/projects/{{pid}}/members/{{uid}} → {(int)resp.StatusCode}; body: {await Body(resp)}");

        await using var ctx = _fixture.NewDbContext();
        var member = await ctx.ProjectMembers.FirstAsync(m => m.ProjectId == Pid && m.UserId == userId);
        Assert.Equal("operator", member.Role);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    //  AUTHZ NEGATIVES — a read-only viewer must be 403 on writes
    // ═══════════════════════════════════════════════════════════════════════════

    [Fact]
    public async Task Viewer_cannot_create_test_config_403()
    {
        var client = _fixture.CreateViewerClient();
        var body = new
        {
            name = Uniq("cfg-viewer-denied"),
            endpoint = new { kind = "network", host = "https://example.com" },
            workload = new { modes = new[] { "http11" }, runs = 1 },
        };

        var resp = await client.PostAsJsonAsync($"/api/v2/projects/{Pid}/test-configs", body);

        Assert.True(resp.StatusCode == HttpStatusCode.Forbidden,
            $"viewer POST /api/v2/projects/{{pid}}/test-configs → {(int)resp.StatusCode} (want 403); body: {await Body(resp)}");
    }

    [Fact]
    public async Task Viewer_cannot_create_alert_channel_403()
    {
        var client = _fixture.CreateViewerClient();
        var body = new
        {
            kind = "webhook",
            name = Uniq("chan-viewer-denied"),
            config = new { url = "https://hooks.example.com/x" },
        };

        var resp = await client.PostAsJsonAsync($"/api/v2/projects/{Pid}/alert-channels", body);

        Assert.True(resp.StatusCode == HttpStatusCode.Forbidden,
            $"viewer POST /api/v2/projects/{{pid}}/alert-channels → {(int)resp.StatusCode} (want 403); body: {await Body(resp)}");
    }
}
