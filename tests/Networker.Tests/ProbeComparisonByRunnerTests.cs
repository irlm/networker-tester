using System.Net;
using System.Text;
using System.Text.Json;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.EntityFrameworkCore;
using Networker.Data;
using Networker.Data.Entities;
using Npgsql;

namespace Networker.Tests;

/// <summary>
/// The comparison report against a REAL Postgres, because the parts changed here
/// exist only in SQL: the runner join, the <c>$6</c> series key, and the appended
/// runner columns. Unit tests over <c>ProbeComparisonLogic</c> cannot see any of
/// it — they start from rows the query already produced.
///
/// <para>Why the runner dimension exists: until v0.28.298 the report grouped by
/// URL alone, so samples from every vantage point were pooled. On production
/// 2026-08-24 <c>www.microsoft.com</c>'s median came from 349 attempts across two
/// runners whose own medians were 35 ms and 76 ms — and 321 of the 349 were the
/// slower one, so the published number described the runner, not the site.</para>
/// </summary>
public sealed class ProbeComparisonByRunnerTests(ControlPlaneFixture fx) : IClassFixture<ControlPlaneFixture>
{
    private const string Url = "https://runner-split.example/";
    private static readonly Guid FastTester = Guid.Parse("aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa");
    private static readonly Guid SlowTester = Guid.Parse("bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb");

    /// <summary>Two runners probing the SAME url, with deliberately different
    /// latencies, spread over enough buckets to clear the coverage floor.</summary>
    private async Task SeedAsync()
    {
        using var scope = fx.Services.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
        if (db.ProjectTesters.Any(t => t.TesterId == FastTester))
        {
            return;
        }
        var now = DateTime.UtcNow;

        foreach (var (id, name) in new[] { (FastTester, "fast-runner"), (SlowTester, "slow-runner") })
        {
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
                AutoShutdownEnabled = false,
                AutoShutdownLocalHour = 0,
                ShutdownDeferralCount = 0,
                AutoProbeEnabled = false,
                BenchmarkRunCount = 0,
                CreatedAt = now,
                UpdatedAt = now,
            });
        }

        var configId = Guid.NewGuid();
        db.TestConfigs.Add(new TestConfig
        {
            Id = configId,
            ProjectId = ControlPlaneFixture.SeededProjectId,
            Name = $"runner-split-{configId:N}",
            TestKind = "url_probe",
            EndpointKind = "network",
            EndpointRef = "{}",
            Workload = "{}",
            MaxDurationSecs = 60,
            CreatedAt = now,
            UpdatedAt = now,
        });

        var runs = new List<(Guid RunId, Guid Tester, double Ms)>();
        foreach (var (tester, ms) in new[] { (FastTester, 30.0), (SlowTester, 90.0) })
        {
            for (var h = 0; h < 12; h++)
            {
                var runId = Guid.NewGuid();
                db.TestRuns.Add(new TestRun
                {
                    Id = runId,
                    TestConfigId = configId,
                    ProjectId = ControlPlaneFixture.SeededProjectId,
                    Status = "completed",
                    TesterId = tester,
                    CreatedAt = now.AddHours(-h),
                });
                runs.Add((runId, tester, ms));
            }
        }
        db.SaveChanges();

        // RequestAttempt is the tester-owned schema — not in the EF model, so it
        // is seeded with raw SQL. Use the app's own NpgsqlDataSource: the EF
        // connection string comes back with the password stripped once connected.
        var dataSource = scope.ServiceProvider.GetRequiredService<NpgsqlDataSource>();
        await using var conn = await dataSource.OpenConnectionAsync();

        // This fixture boots with NETWORKER_RUN_MIGRATIONS=0 and materialises the
        // schema from the EF model, which does NOT include the tester-owned probe
        // tables — and the startup bootstrap that would create them lives inside
        // that same opted-out block. Apply the shipped schema here, exactly as
        // AttemptPersister does. Every statement is idempotent.
        // Read the SAME embedded copy the control plane applies, rather than a
        // file path: shared/ is not copied into this test project's output, and
        // a second hand-written copy of the DDL is exactly the drift that
        // install.sh caused (v0.28.293).
        var cpAssembly = typeof(Networker.ControlPlane.Realtime.RawWs.AttemptPersister).Assembly;
        await using (var stream = cpAssembly.GetManifestResourceStream(
            "Networker.ControlPlane.shared.tester-schema.postgres.sql")!)
        using (var sr = new StreamReader(stream))
        {
            await using var ddl = new NpgsqlCommand(await sr.ReadToEndAsync(), conn);
            await ddl.ExecuteNonQueryAsync();
        }
        var hour = 0;
        foreach (var (runId, _, ms) in runs)
        {
            var started = now.AddHours(-(hour++ % 12)).AddMinutes(-1);
            await using var tx = await conn.BeginTransactionAsync();
            await using (var mk = new NpgsqlCommand(
                "INSERT INTO TestRun (RunId, StartedAt, TargetUrl, TargetHost, Modes, ClientOs, ClientVersion) "
                + "VALUES (@r, @s, @u, 'runner-split.example', 'http2', 'linux', 'test') ON CONFLICT DO NOTHING", conn, tx))
            {
                mk.Parameters.AddWithValue("r", runId);
                mk.Parameters.AddWithValue("s", started);
                mk.Parameters.AddWithValue("u", Url);
                await mk.ExecuteNonQueryAsync();
            }
            // 4 samples per bucket clears the default min_samples of 3.
            for (var i = 0; i < 4; i++)
            {
                await using var ins = new NpgsqlCommand(
                    "INSERT INTO RequestAttempt (AttemptId, RunId, Protocol, SequenceNum, StartedAt, FinishedAt, Success, TargetUrl) "
                    + "VALUES (@a, @r, 'http2', @n, @s, @f, TRUE, @u)", conn, tx);
                ins.Parameters.AddWithValue("a", Guid.NewGuid());
                ins.Parameters.AddWithValue("r", runId);
                ins.Parameters.AddWithValue("n", i);
                ins.Parameters.AddWithValue("s", started);
                ins.Parameters.AddWithValue("f", started.AddMilliseconds(ms));
                ins.Parameters.AddWithValue("u", Url);
                await ins.ExecuteNonQueryAsync();
            }
            await tx.CommitAsync();
        }
    }

    private async Task<JsonDocument> GetAsync(string query)
    {
        var resp = await fx.CreateAuthenticatedClient().GetAsync(
            $"/api/projects/{ControlPlaneFixture.SeededProjectId}/reports/probe-comparison?{query}");
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        return JsonDocument.Parse(await resp.Content.ReadAsStringAsync());
    }

    [Fact]
    public async Task Group_by_runner_races_the_two_runners_for_one_url()
    {
        await SeedAsync();
        using var doc = await GetAsync(
            $"urls={Uri.EscapeDataString(Url)}&group_by=runner&window=24h&bucket=1h");

        var http2 = doc.RootElement.GetProperty("modes").EnumerateArray()
            .Single(m => m.GetProperty("mode").GetString() == "http2");

        var labels = http2.GetProperty("coverage").EnumerateArray()
            .Select(c => c.GetProperty("url").GetString()).ToHashSet(StringComparer.Ordinal);

        // The compared entities are RUNNERS, not urls — this is the whole point.
        Assert.Contains("fast-runner", labels);
        Assert.Contains("slow-runner", labels);
        Assert.DoesNotContain(Url, labels);

        // And the faster runner wins on median, as seeded (30 ms vs 90 ms).
        var fastest = http2.GetProperty("crowns").GetProperty("fastest").GetString();
        Assert.Equal("fast-runner", fastest);
    }

    [Fact]
    public async Task Group_by_url_still_compares_urls_and_reports_that_it_blended()
    {
        await SeedAsync();
        using var doc = await GetAsync(
            $"urls={Uri.EscapeDataString(Url)},{Uri.EscapeDataString("https://absent.example/")}"
            + "&window=24h&bucket=1h");

        var http2 = doc.RootElement.GetProperty("modes").EnumerateArray()
            .Single(m => m.GetProperty("mode").GetString() == "http2");
        var labels = http2.GetProperty("coverage").EnumerateArray()
            .Select(c => c.GetProperty("url").GetString()).ToList();

        // Default behaviour is unchanged: series are urls.
        Assert.Contains(Url, labels);
    }

    [Fact]
    public async Task Runner_mode_refuses_more_than_one_url_instead_of_averaging_across_sites()
    {
        // Pooling two SITES into one per-runner series would recreate the very
        // blending this mode exists to fix, just rotated 90 degrees.
        var resp = await fx.CreateAuthenticatedClient().GetAsync(
            $"/api/projects/{ControlPlaneFixture.SeededProjectId}/reports/probe-comparison"
            + $"?urls={Uri.EscapeDataString(Url)},{Uri.EscapeDataString("https://other.example/")}&group_by=runner");
        Assert.Equal(HttpStatusCode.BadRequest, resp.StatusCode);
        Assert.Contains("exactly one", await resp.Content.ReadAsStringAsync(), StringComparison.OrdinalIgnoreCase);
    }

    [Fact]
    public async Task An_unknown_group_by_is_rejected()
    {
        var resp = await fx.CreateAuthenticatedClient().GetAsync(
            $"/api/projects/{ControlPlaneFixture.SeededProjectId}/reports/probe-comparison?group_by=sideways");
        Assert.Equal(HttpStatusCode.BadRequest, resp.StatusCode);
    }

    // ── Hiding URLs from the picker ───────────────────────────────────────
    // Reversible by construction: hiding writes a list into project.settings and
    // touches no probe data, so a URL hidden today still has its history next
    // month. Deleting the attempts would also silently rewrite every historical
    // report that covered them.

    private async Task<HttpResponseMessage> SetHiddenAsync(params string[] urls)
    {
        var body = new StringContent(
            JsonSerializer.Serialize(new { urls }), Encoding.UTF8, "application/json");
        return await fx.CreateAuthenticatedClient().PutAsync(
            $"/api/projects/{ControlPlaneFixture.SeededProjectId}/reports/probe-comparison/hidden", body);
    }

    private static List<string> AvailableUrls(JsonDocument doc) =>
        doc.RootElement.GetProperty("available").EnumerateArray()
           .Select(e => e.GetProperty("url").GetString()!).ToList();

    [Fact]
    public async Task A_hidden_url_leaves_the_picker_but_is_still_listed_as_hidden()
    {
        await SeedAsync();
        Assert.Equal(HttpStatusCode.OK, (await SetHiddenAsync(Url)).StatusCode);
        try
        {
            using var doc = await GetAsync("window=24h&bucket=1h");
            Assert.DoesNotContain(Url, AvailableUrls(doc));

            // Not pretended out of existence — the page can offer "show hidden".
            var hidden = doc.RootElement.GetProperty("hidden").EnumerateArray()
                .Select(e => e.GetString()).ToList();
            Assert.Contains(Url, hidden);
        }
        finally
        {
            await SetHiddenAsync();   // leave the shared fixture as we found it
        }
    }

    [Fact]
    public async Task include_hidden_brings_it_back()
    {
        await SeedAsync();
        await SetHiddenAsync(Url);
        try
        {
            using var doc = await GetAsync("window=24h&bucket=1h&include_hidden=true");
            Assert.Contains(Url, AvailableUrls(doc));
        }
        finally
        {
            await SetHiddenAsync();
        }
    }

    [Fact]
    public async Task Hiding_never_touches_the_probe_data()
    {
        // The property that makes this reversible. A hidden URL still reports in
        // full when explicitly compared, so hiding cannot blank a comparison
        // somebody has open, and unhiding restores everything.
        await SeedAsync();
        await SetHiddenAsync(Url);
        try
        {
            using var doc = await GetAsync(
                $"urls={Uri.EscapeDataString(Url)}&group_by=runner&window=24h&bucket=1h");
            var http2 = doc.RootElement.GetProperty("modes").EnumerateArray()
                .Single(m => m.GetProperty("mode").GetString() == "http2");
            Assert.Equal("fast-runner", http2.GetProperty("crowns").GetProperty("fastest").GetString());
        }
        finally
        {
            await SetHiddenAsync();
        }
    }

    [Fact]
    public async Task Unhiding_restores_the_picker_entry()
    {
        await SeedAsync();
        await SetHiddenAsync(Url);
        await SetHiddenAsync();      // empty list = nothing hidden
        using var doc = await GetAsync("window=24h&bucket=1h");
        Assert.Contains(Url, AvailableUrls(doc));
    }

    [Fact]
    public async Task Hiding_preserves_other_keys_in_project_settings()
    {
        // settings is a shared bag; rewriting it wholesale would drop whatever
        // else lives there.
        using (var scope = fx.Services.CreateScope())
        {
            var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
            var proj = db.Projects.First(p => p.ProjectId == ControlPlaneFixture.SeededProjectId);
            proj.Settings = """{"unrelated_key":"keep-me"}""";
            db.SaveChanges();
        }

        await SetHiddenAsync("https://x.example/");
        try
        {
            using var scope = fx.Services.CreateScope();
            var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
            var settings = db.Projects.First(p => p.ProjectId == ControlPlaneFixture.SeededProjectId).Settings;
            using var doc = JsonDocument.Parse(settings);
            Assert.Equal("keep-me", doc.RootElement.GetProperty("unrelated_key").GetString());
            Assert.Single(doc.RootElement.GetProperty("hidden_probe_urls").EnumerateArray());
        }
        finally
        {
            await SetHiddenAsync();
        }
    }
}
