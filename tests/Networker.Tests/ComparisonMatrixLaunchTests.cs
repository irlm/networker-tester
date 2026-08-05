using System.Net;
using System.Net.Http.Json;
using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Xunit;

namespace Networker.Tests;

/// <summary>
/// P0-1 (2026-08 audit): the comparison-group MATRIX LAUNCH — the flow behind
/// the Full Stack Benchmark, and the one that turned out to have never worked
/// end-to-end — had no executing test at all. Its only coverage was 4
/// <c>ParseCells</c> JSON-parsing facts, and it was explicitly excluded from
/// the write-endpoint coverage sweep as a "202 shell with no DB effect" —
/// which is factually wrong: the handler writes a TestConfig per cell and
/// dispatches a TestRun per cell.
///
/// <para>These tests execute the real HTTP route against real Postgres and
/// assert the DB effects the campaign's bugs violated: one config + one run
/// per cell, per-cell names unique (VM-name collisions came from shared name
/// prefixes), a re-launch producing a fresh set rather than tripping
/// UNIQUE(project_id, name), the unsupported-combo gate reporting real
/// reasons, and per-cell failure isolation not aborting the matrix.</para>
/// </summary>
public class ComparisonMatrixLaunchTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fx;

    public ComparisonMatrixLaunchTests(ControlPlaneFixture fx) => _fx = fx;

    private static object PendingCell(string label, string os, string stack) => new
    {
        label,
        endpoint = new
        {
            kind = "pending",
            cloud_account_id = "00000000-0000-4000-8000-0000000000aa",
            region = "eastus",
            vm_size = "Standard_B2s",
            os,
            proxy_stack = stack,
        },
    };

    private async Task<Guid> CreateGroupAsync(HttpClient client, string name, object[] cells)
    {
        var resp = await client.PostAsJsonAsync(
            $"/api/v2/projects/{ControlPlaneFixture.SeededProjectId}/comparison-groups",
            new
            {
                name,
                base_workload = new { runs = 2, modes = new[] { "http1", "download" } },
                methodology = new { preset = "quick" },
                cells,
            });
        Assert.Equal(HttpStatusCode.Created, resp.StatusCode);
        var body = await resp.Content.ReadFromJsonAsync<JsonElement>();
        return body.GetProperty("id").GetGuid();
    }

    [Fact]
    public async Task Multi_cell_launch_creates_one_config_and_one_run_per_cell()
    {
        using var client = _fx.CreateAuthenticatedClient();

        // 6 supported cells across both OSes and several stacks — the shape
        // that collided on VM names before v0.28.129.
        object[] cells =
        [
            PendingCell("Azure/eastus linux · nginx", "linux", "nginx"),
            PendingCell("Azure/eastus linux · caddy", "linux", "caddy"),
            PendingCell("Azure/eastus linux · traefik", "linux", "traefik"),
            PendingCell("Azure/eastus linux · haproxy", "linux", "haproxy"),
            PendingCell("Azure/eastus windows · iis", "windows", "iis"),
            PendingCell("Azure/eastus windows · caddy", "windows", "caddy"),
        ];
        var groupId = await CreateGroupAsync(client, $"matrix-{Guid.NewGuid():N}", cells);

        var launch = await client.PostAsync($"/api/v2/comparison-groups/{groupId}/launch", null);
        Assert.Equal(HttpStatusCode.Accepted, launch.StatusCode);
        var result = await launch.Content.ReadFromJsonAsync<JsonElement>();

        Assert.Equal(6, result.GetProperty("total").GetInt32());
        Assert.Equal(6, result.GetProperty("launched").GetInt32());
        Assert.Equal(0, result.GetProperty("failed").GetInt32());

        await using var db = _fx.NewDbContext();
        var runs = await db.TestRuns.AsNoTracking()
            .Where(r => r.ComparisonGroupId == groupId)
            .ToListAsync();
        Assert.Equal(6, runs.Count);

        var configIds = runs.Select(r => r.TestConfigId).Distinct().ToList();
        Assert.Equal(6, configIds.Count); // one config per cell, never shared

        var names = await db.TestConfigs.AsNoTracking()
            .Where(c => configIds.Contains(c.Id))
            .Select(c => c.Name)
            .ToListAsync();
        // Per-cell names must be distinct — the VM label is derived per run,
        // but a shared name would still collide the UNIQUE(project_id,name).
        Assert.Equal(6, names.Distinct().Count());
    }

    [Fact]
    public async Task Relaunching_the_same_group_succeeds_with_a_fresh_set()
    {
        using var client = _fx.CreateAuthenticatedClient();
        object[] cells =
        [
            PendingCell("Azure/eastus linux · nginx", "linux", "nginx"),
            PendingCell("Azure/eastus linux · caddy", "linux", "caddy"),
        ];
        var groupId = await CreateGroupAsync(client, $"relaunch-{Guid.NewGuid():N}", cells);

        var first = await client.PostAsync($"/api/v2/comparison-groups/{groupId}/launch", null);
        Assert.Equal(HttpStatusCode.Accepted, first.StatusCode);

        // The v0.28.129 regression: cell config names were deterministic per
        // (group, index), so a re-launch tripped UNIQUE(project_id, name) on
        // EVERY cell. A per-launch nonce fixed it — this pins that.
        var second = await client.PostAsync($"/api/v2/comparison-groups/{groupId}/launch", null);
        Assert.Equal(HttpStatusCode.Accepted, second.StatusCode);
        var secondBody = await second.Content.ReadFromJsonAsync<JsonElement>();
        Assert.Equal(2, secondBody.GetProperty("launched").GetInt32());
        Assert.Equal(0, secondBody.GetProperty("failed").GetInt32());

        await using var db = _fx.NewDbContext();
        var runs = await db.TestRuns.AsNoTracking()
            .Where(r => r.ComparisonGroupId == groupId)
            .ToListAsync();
        Assert.Equal(4, runs.Count); // 2 + 2, no collisions
    }

    [Fact]
    public async Task Unsupported_windows_combos_fail_at_launch_with_real_reasons()
    {
        using var client = _fx.CreateAuthenticatedClient();
        object[] cells =
        [
            PendingCell("Azure/eastus linux · nginx", "linux", "nginx"),
            PendingCell("Azure/eastus windows · haproxy", "windows", "haproxy"),
            PendingCell("Azure/eastus windows · apache", "windows", "apache"),
        ];
        var groupId = await CreateGroupAsync(client, $"gated-{Guid.NewGuid():N}", cells);

        var launch = await client.PostAsync($"/api/v2/comparison-groups/{groupId}/launch", null);
        Assert.Equal(HttpStatusCode.Accepted, launch.StatusCode);
        var result = await launch.Content.ReadFromJsonAsync<JsonElement>();

        // The supported cell still launches — per-cell isolation, the matrix
        // is never aborted by a bad cell.
        Assert.Equal(3, result.GetProperty("total").GetInt32());
        Assert.Equal(1, result.GetProperty("launched").GetInt32());
        Assert.Equal(2, result.GetProperty("failed").GetInt32());

        var errors = result.GetProperty("errors").EnumerateArray()
            .Select(e => e.GetString() ?? "").ToList();
        Assert.Contains(errors, e => e.Contains("haproxy", StringComparison.OrdinalIgnoreCase)
            && e.Contains("Windows", StringComparison.OrdinalIgnoreCase));
        Assert.Contains(errors, e => e.Contains("apache", StringComparison.OrdinalIgnoreCase)
            && e.Contains("Windows", StringComparison.OrdinalIgnoreCase));

        await using var db = _fx.NewDbContext();
        var runs = await db.TestRuns.AsNoTracking()
            .CountAsync(r => r.ComparisonGroupId == groupId);
        Assert.Equal(1, runs); // gated cells burn no run, no VM, no timeout
    }

    [Fact]
    public async Task Cell_deadline_scales_with_the_group_workload()
    {
        using var client = _fx.CreateAuthenticatedClient();
        // A big workload must NOT get the old hardcoded 900s (v0.28.136/139:
        // every cell was killed at ~16 min against a hours-long workload).
        var resp = await client.PostAsJsonAsync(
            $"/api/v2/projects/{ControlPlaneFixture.SeededProjectId}/comparison-groups",
            new
            {
                name = $"deadline-{Guid.NewGuid():N}",
                base_workload = new
                {
                    runs = 100,
                    modes = new[]
                    {
                        "http1", "http2", "http3", "download", "upload", "tcp", "dns",
                        "tls", "curl", "pageload", "pageload2", "pageload3",
                    },
                },
                cells = new[] { PendingCell("Azure/eastus linux · nginx", "linux", "nginx") },
            });
        Assert.Equal(HttpStatusCode.Created, resp.StatusCode);
        var groupId = (await resp.Content.ReadFromJsonAsync<JsonElement>()).GetProperty("id").GetGuid();

        var launch = await client.PostAsync($"/api/v2/comparison-groups/{groupId}/launch", null);
        Assert.Equal(HttpStatusCode.Accepted, launch.StatusCode);

        await using var db = _fx.NewDbContext();
        var run = await db.TestRuns.AsNoTracking()
            .FirstAsync(r => r.ComparisonGroupId == groupId);
        var cfg = await db.TestConfigs.AsNoTracking().FirstAsync(c => c.Id == run.TestConfigId);
        // 100 runs × 12 modes × 8s + 600 = 10200s — far past the old 900 floor.
        Assert.True(cfg.MaxDurationSecs > 900,
            $"cell deadline did not scale with the workload: {cfg.MaxDurationSecs}s");
    }
}
