using System.Net;
using System.Text.Json;
using Microsoft.Extensions.DependencyInjection;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.Tests;

/// <summary>
/// The runner LIST must carry the installed version.
///
/// <para><c>src/api/testers.ts</c> declares <c>installer_version</c> on the list
/// row type and <c>TesterRegionGroup</c> renders
/// <c>v{installer_version ?? '?'}</c> — but the field lived only on the DETAIL
/// DTO. So every row on the Infrastructure page read "v?" while opening the
/// drawer for the same runner showed the real version (prod 2026-08-24: the two
/// installed runners were 0.28.293 and 0.28.259 the whole time).</para>
///
/// <para>TypeScript could not catch this: the type promised a field the server
/// never sent, and nothing validates JSON at that boundary. A server-side test
/// is the only thing that holds the contract.</para>
/// </summary>
public sealed class TesterListVersionTests(ControlPlaneFixture fx) : IClassFixture<ControlPlaneFixture>
{
    private Guid SeedTester(string name, string? installerVersion)
    {
        using var scope = fx.Services.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
        var now = DateTime.UtcNow;
        var id = Guid.NewGuid();
        db.ProjectTesters.Add(new ProjectTester
        {
            TesterId = id,
            ProjectId = ControlPlaneFixture.SeededProjectId,
            Name = name,
            Cloud = "azure",
            Region = "eastus",
            VmSize = "Standard_B2s",
            SshUser = "azureuser",
            PowerState = "stopped",
            Allocation = "on-demand",
            InstallerVersion = installerVersion,
            LastInstalledAt = installerVersion is null ? null : now,
            AutoShutdownEnabled = false,
            AutoShutdownLocalHour = 0,
            ShutdownDeferralCount = 0,
            AutoProbeEnabled = false,
            BenchmarkRunCount = 0,
            CreatedAt = now,
            UpdatedAt = now,
        });
        db.SaveChanges();
        return id;
    }

    private async Task<JsonElement> ListRowAsync(Guid testerId)
    {
        var resp = await fx.CreateAuthenticatedClient()
            .GetAsync($"/api/projects/{ControlPlaneFixture.SeededProjectId}/testers");
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);

        var doc = JsonDocument.Parse(await resp.Content.ReadAsStringAsync());
        return doc.RootElement.EnumerateArray()
            .Single(e => e.GetProperty("tester_id").GetGuid() == testerId).Clone();
    }

    [Fact]
    public async Task An_installed_runner_reports_its_version_in_the_list()
    {
        var id = SeedTester($"ver-installed-{Guid.NewGuid():N}", "0.28.293");

        var row = await ListRowAsync(id);

        Assert.True(row.TryGetProperty("installer_version", out var v),
            "installer_version is missing from the tester LIST payload — the UI renders 'v?'");
        Assert.Equal("0.28.293", v.GetString());
        Assert.NotEqual(JsonValueKind.Null, row.GetProperty("last_installed_at").ValueKind);
    }

    [Fact]
    public async Task A_never_installed_runner_still_reports_null_not_a_missing_field()
    {
        // "v?" is the CORRECT display for a runner that has never been installed
        // (prod had one). The distinction that matters is present-and-null versus
        // absent — absent is what made every row look uninstalled.
        var id = SeedTester($"ver-fresh-{Guid.NewGuid():N}", null);

        var row = await ListRowAsync(id);

        Assert.True(row.TryGetProperty("installer_version", out var v));
        Assert.Equal(JsonValueKind.Null, v.ValueKind);
    }
}
