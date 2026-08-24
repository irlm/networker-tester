using System.Net;
using System.Text.Json;
using System.Net.Http.Headers;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.Mvc.Testing;
using Networker.ControlPlane.Auth;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Observability;

namespace Networker.Tests;

/// <summary>
/// The <c>service_log</c> sink end to end against a real Postgres and the
/// booted app: the control plane creates the table, writes its own log lines
/// into it, and the pre-existing <c>/api/logs</c> reader — a straight port of
/// the Rust <c>api/logs.rs</c>, written years before this writer — returns them.
///
/// <para>That round trip is the whole point. Unit tests can prove the writer
/// builds the rows it intends to; only this proves the rows it writes are the
/// rows the reader already expects, in the table the Rust tester and endpoint
/// also write. A column-order slip or a level-encoding mistake fails here and
/// nowhere else.</para>
/// </summary>
public sealed class ServiceLogSinkIntegrationTests(ControlPlaneFixture shared)
    : IClassFixture<ControlPlaneFixture>, IDisposable
{
    // The shared fixture boots with the sink OFF (its default, and every other
    // suite depends on that). WithWebHostBuilder gives a second host over the
    // SAME container with the sink switched on — no duplicated seeding, and no
    // behaviour change for anyone else's tests.
    private readonly WebApplicationFactory<Program> _app = shared.WithWebHostBuilder(b =>
    {
        b.UseSetting(ServiceLogOptions.EnabledEnvVar, "1");
        b.UseSetting(ServiceLogOptions.ServiceEnvVar, "dashboard");
        // Flush promptly so the test polls briefly instead of waiting out the
        // 2 s production default.
        b.UseSetting(ServiceLogOptions.IntervalEnvVar, "100");
        b.UseSetting(ServiceLogOptions.BatchEnvVar, "50");
    });

    public void Dispose() => _app.Dispose();

    private HttpClient Client()
    {
        var client = _app.CreateClient();
        var tokens = _app.Services.GetRequiredService<JwtTokenService>();
        var jwt = tokens.CreateToken(
            ControlPlaneFixture.SeededUserId, ControlPlaneFixture.SeededUserEmail,
            "operator", isPlatformAdmin: false);
        client.DefaultRequestHeaders.Authorization = new AuthenticationHeaderValue("Bearer", jwt);
        return client;
    }

    private const string Marker = "service-log-sink-roundtrip-marker";

    /// <summary>Log through the app's own ILogger, then wait for the batching
    /// writer to commit. Polls rather than sleeping a fixed amount so a slow CI
    /// box does not make this flaky.</summary>
    private async Task<long> LogAndFlushAsync(string message, LogLevel level = LogLevel.Warning)
    {
        var logger = _app.Services.GetRequiredService<ILoggerFactory>()
            .CreateLogger("ServiceLogSinkIntegrationTests");
        var metrics = _app.Services.GetRequiredService<ServiceLogMetrics>();
        var before = metrics.EntriesWritten;

        logger.Log(level, "{Marker} {ProjectId}", message, ControlPlaneFixture.SeededProjectId);

        for (var i = 0; i < 100 && metrics.EntriesWritten == before; i++)
        {
            await Task.Delay(100);
        }
        return metrics.EntriesWritten;
    }

    [Fact]
    public async Task Sink_creates_the_table_and_the_reader_reports_it_configured()
    {
        // Before this change the C# control plane never created service_log, so
        // this endpoint always answered "unconfigured"/"none".
        var resp = await Client().GetAsync("/api/logs/pipeline-status");
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);

        using var doc = JsonDocument.Parse(await resp.Content.ReadAsStringAsync());
        var root = doc.RootElement;
        Assert.Equal("service_log", root.GetProperty("log_sink").GetString());
        Assert.NotEqual("unconfigured", root.GetProperty("status").GetString());
        // This process is a writer, not merely a reader of someone else's table.
        Assert.Equal("dashboard", root.GetProperty("writer").GetString());
    }

    [Fact]
    public async Task A_logged_line_comes_back_through_the_reader()
    {
        var written = await LogAndFlushAsync(Marker);
        Assert.True(written > 0, "the writer committed no rows");

        var resp = await Client()
            .GetAsync($"/api/logs?service=dashboard&search={Marker}&limit=50");
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);

        using var doc = JsonDocument.Parse(await resp.Content.ReadAsStringAsync());
        // NOTE: the SUCCESS shape is { entries, total, truncated } — `log_sink`
        // only appears on the unconfigured/empty response. pipeline-status is
        // where that field is asserted.
        var entries = doc.RootElement.GetProperty("entries").EnumerateArray().ToList();
        Assert.NotEmpty(entries);

        var entry = entries[0];
        Assert.Contains(Marker, entry.GetProperty("message").GetString());
        Assert.Equal("dashboard", entry.GetProperty("service").GetString());
        // Warning == 2 in the encoding shared with networker_log::Level::as_db.
        Assert.Equal(2, entry.GetProperty("level").GetInt32());
        Assert.False(string.IsNullOrEmpty(entry.GetProperty("ts").GetString()));
    }

    [Fact]
    public async Task The_level_filter_selects_what_the_writer_filed()
    {
        await LogAndFlushAsync(Marker + "-err", LogLevel.Error);

        // The reader's own level filter must find it — this is the assertion
        // that catches an encoding drift between writer and reader.
        var resp = await Client()
            .GetAsync($"/api/logs?level=error&search={Marker}-err&limit=50");
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);

        using var doc = JsonDocument.Parse(await resp.Content.ReadAsStringAsync());
        var entries = doc.RootElement.GetProperty("entries").EnumerateArray().ToList();
        Assert.NotEmpty(entries);
        Assert.All(entries, e => Assert.Equal(1, e.GetProperty("level").GetInt32()));
    }

    [Fact]
    public async Task Structured_project_id_lands_in_its_own_column()
    {
        // The log call passes {ProjectId}; the sink lifts it into the project_id
        // COLUMN so that filter selects something instead of being decoration
        // over an always-null column.
        //
        // Filtering by `service` rather than `project_id` on purpose: on these
        // flat routes a non-admin's project_id is forced to null (the ported
        // Rust behaviour), so `?project_id=…&search=…` is a search with no
        // narrowing filter and correctly 400s for an operator token. What is
        // under test here is that the column is POPULATED, which the returned
        // rows show either way.
        await LogAndFlushAsync(Marker + "-scoped");

        var resp = await Client()
            .GetAsync($"/api/logs?service=dashboard&search={Marker}-scoped&limit=50");
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);

        using var doc = JsonDocument.Parse(await resp.Content.ReadAsStringAsync());
        var entries = doc.RootElement.GetProperty("entries").EnumerateArray().ToList();
        Assert.NotEmpty(entries);
        Assert.All(entries, e =>
            Assert.Equal(
                ControlPlaneFixture.SeededProjectId,
                e.GetProperty("project_id").GetString()?.TrimEnd()));
    }

    [Fact]
    public async Task Pipeline_status_reports_real_counters_not_zeros()
    {
        await LogAndFlushAsync(Marker + "-metrics");

        var resp = await Client().GetAsync("/api/logs/pipeline-status");
        using var doc = JsonDocument.Parse(await resp.Content.ReadAsStringAsync());
        var root = doc.RootElement;

        Assert.True(root.GetProperty("entries_written").GetInt64() > 0, "entries_written stayed at the old hard-coded 0");
        Assert.True(root.GetProperty("flush_count").GetInt64() > 0);
        Assert.Equal(0, root.GetProperty("flush_errors").GetInt64());
        Assert.Equal("healthy", root.GetProperty("status").GetString());
    }

    [Fact]
    public async Task The_sink_never_logs_itself_into_a_loop()
    {
        // The writer's own diagnostics must not become rows in the sink, or a
        // database failure produces an error per failed flush, forever.
        var logger = _app.Services.GetRequiredService<ILoggerFactory>()
            .CreateLogger(typeof(ServiceLogWriter).FullName!);
        logger.LogError("sink-self-log-must-not-persist");

        await Task.Delay(600);

        var resp = await Client()
            .GetAsync("/api/logs?level=error&search=sink-self-log-must-not-persist&limit=10");
        using var doc = JsonDocument.Parse(await resp.Content.ReadAsStringAsync());
        Assert.Empty(doc.RootElement.GetProperty("entries").EnumerateArray());
    }
}
