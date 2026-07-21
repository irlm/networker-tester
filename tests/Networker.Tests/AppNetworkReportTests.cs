using System.Net;
using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.Data.Entities;

namespace Networker.Tests;

/// <summary>
/// End-to-end tests for <c>GET /api/projects/{id}/reports/app-network</c>
/// against a real Postgres (Testcontainers) and the booted app.
///
/// Split like the perf-per-cost suite: <see cref="AppNetworkAuthzTests"/> runs
/// against the pristine fixture (tester probe schema ABSENT — the 42P01 →
/// empty-report path and the authz matrix), while
/// <see cref="AppNetworkAggregationTests"/> gets its own container, creates the
/// tester-owned V001 slice (RequestAttempt + ServerTimingResult), seeds
/// hand-picked sdkprobe rows, and asserts the split arithmetic + verdict.
/// </summary>
public sealed class AppNetworkAuthzTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fixture;

    public AppNetworkAuthzTests(ControlPlaneFixture fixture) => _fixture = fixture;

    private static string Url(string projectId) => $"/api/projects/{projectId}/reports/app-network";

    [Fact]
    public async Task Report_requires_authentication()
    {
        var resp = await _fixture.CreateClient().GetAsync(Url(ControlPlaneFixture.SeededProjectId));
        Assert.Equal(HttpStatusCode.Unauthorized, resp.StatusCode);
    }

    [Fact]
    public async Task Report_forbids_non_member_project()
    {
        var resp = await _fixture.CreateAuthenticatedClient().GetAsync(Url("proj-not-a-member"));
        Assert.Equal(HttpStatusCode.Forbidden, resp.StatusCode);
    }

    [Fact]
    public async Task Report_is_member_read_and_empty_when_no_probe_schema()
    {
        // No tester probe schema in this container → a valid EMPTY 200 for a
        // viewer, with formulas + a no_data overall verdict (the 42P01 path).
        var resp = await _fixture.CreateViewerClient().GetAsync(Url(ControlPlaneFixture.SeededProjectId));
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);

        using var doc = JsonDocument.Parse(await resp.Content.ReadAsStringAsync());
        var root = doc.RootElement;

        Assert.Empty(root.GetProperty("groups").EnumerateArray());
        Assert.Equal(0, root.GetProperty("attempt_count").GetInt64());
        Assert.Equal("no_data", root.GetProperty("overall_verdict").GetString());
        Assert.Equal("sdkprobe", root.GetProperty("mode").GetString());
        Assert.Contains("TotalServerMs",
            root.GetProperty("formulas").GetProperty("server_ms").GetString());
        Assert.Contains("max(0",
            root.GetProperty("formulas").GetProperty("network_ms").GetString());
    }
}

public sealed class AppNetworkAggregationTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fixture;

    public AppNetworkAggregationTests(ControlPlaneFixture fixture) => _fixture = fixture;

    private static readonly Guid SdkConfigId = Guid.Parse("55555555-5555-4555-8555-555555555555");

    /// <summary>
    /// Seeds a dedicated sdkprobe config + one completed run with four
    /// successful attempts (plus one split anomaly and one failed attempt that
    /// must be ignored):
    ///
    ///   wall / server (ms):  (200,180) (240,180) (300,150) (220,120)
    ///     → network = 20, 60, 150, 100
    ///     → median server 165, median network 80, median wall 230
    ///     → server 165 >= 0.6*230=138 → verdict server_bound
    ///   anomaly attempt: wall 100, server 130 (server > wall) → network floors 0
    ///   failed attempt: ignored (Success = false)
    /// </summary>
    private async Task SeedAsync()
    {
        await using var ctx = _fixture.NewDbContext();
        if (await ctx.TestConfigs.AnyAsync(c => c.Id == SdkConfigId))
        {
            return; // already seeded
        }

        var now = new DateTime(2026, 7, 20, 10, 0, 0, DateTimeKind.Utc);
        ctx.TestConfigs.Add(new TestConfig
        {
            Id = SdkConfigId,
            ProjectId = ControlPlaneFixture.SeededProjectId,
            Name = "sdk-checkout-api",
            EndpointKind = "network",
            EndpointRef = """{"kind":"network","host":"https://customer.example.com"}""",
            Workload = """{"modes":["sdkprobe"],"runs":5}""",
            MaxDurationSecs = 900,
            CreatedAt = now,
            UpdatedAt = now,
        });

        var runId = Guid.NewGuid();
        ctx.TestRuns.Add(new TestRun
        {
            Id = runId,
            TestConfigId = SdkConfigId,
            ProjectId = ControlPlaneFixture.SeededProjectId,
            Status = "completed",
            StartedAt = now,
            FinishedAt = now.AddMinutes(1),
            SuccessCount = 4,
            FailureCount = 1,
            WorkerId = "appnet-itest",
            CreatedAt = now,
        });
        await ctx.SaveChangesAsync();

        var conn = ctx.Database.GetDbConnection();
        await conn.OpenAsync();
        await using var cmd = conn.CreateCommand();
        var a1 = Guid.NewGuid();
        var a2 = Guid.NewGuid();
        var a3 = Guid.NewGuid();
        var a4 = Guid.NewGuid();
        var aAnom = Guid.NewGuid();
        var aFail = Guid.NewGuid();
        cmd.CommandText = $"""
            CREATE TABLE IF NOT EXISTS RequestAttempt (
                AttemptId UUID PRIMARY KEY,
                RunId UUID NOT NULL,
                Protocol VARCHAR(20) NOT NULL,
                SequenceNum INT NOT NULL,
                StartedAt TIMESTAMPTZ NOT NULL,
                FinishedAt TIMESTAMPTZ NULL,
                Success BOOLEAN NOT NULL DEFAULT FALSE,
                ErrorMessage TEXT NULL,
                RetryCount INT NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS ServerTimingResult (
                ServerId UUID PRIMARY KEY,
                AttemptId UUID NOT NULL,
                RequestId VARCHAR(128) NULL,
                ServerTimestamp TIMESTAMPTZ NULL,
                ClockSkewMs DOUBLE PRECISION NULL,
                RecvBodyMs DOUBLE PRECISION NULL,
                ProcessingMs DOUBLE PRECISION NULL,
                TotalServerMs DOUBLE PRECISION NULL
            );

            {Attempt(a1, runId, 1, 0.200, true)}   {Server(a1, 180.0)}
            {Attempt(a2, runId, 2, 0.240, true)}   {Server(a2, 180.0)}
            {Attempt(a3, runId, 3, 0.300, true)}   {Server(a3, 150.0)}
            {Attempt(a4, runId, 4, 0.220, true)}   {Server(a4, 120.0)}

            -- split anomaly: server (130) > wall (100).
            {Attempt(aAnom, runId, 5, 0.100, true)} {Server(aAnom, 130.0)}

            -- failed attempt: must be ignored even though it has a server row.
            {Attempt(aFail, runId, 6, 0.500, false)} {Server(aFail, 999.0)}
            """;
        await cmd.ExecuteNonQueryAsync();
    }

    private static string Attempt(Guid id, Guid runId, int seq, double wallSec, bool success)
    {
        var wall = wallSec.ToString("0.0###", System.Globalization.CultureInfo.InvariantCulture);
        return $"""
            INSERT INTO RequestAttempt
                (AttemptId, RunId, Protocol, SequenceNum, StartedAt, FinishedAt, Success, RetryCount)
            VALUES ('{id}', '{runId}', 'sdkprobe', {seq},
                    '2026-07-20T10:00:00Z',
                    '2026-07-20T10:00:00Z'::timestamptz + interval '{wall} seconds',
                    {(success ? "TRUE" : "FALSE")}, 0);
            """;
    }

    private static string Server(Guid attemptId, double totalServerMs)
    {
        var inv = System.Globalization.CultureInfo.InvariantCulture;
        return $"""
            INSERT INTO ServerTimingResult (ServerId, AttemptId, TotalServerMs)
            VALUES ('{Guid.NewGuid()}', '{attemptId}', {totalServerMs.ToString("0.0###", inv)});
            """;
    }

    [Fact]
    public async Task Split_medians_and_verdict_match_hand_computed_numbers()
    {
        await SeedAsync();
        var client = _fixture.CreateAuthenticatedClient();

        var resp = await client.GetAsync(
            $"/api/projects/{ControlPlaneFixture.SeededProjectId}/reports/app-network?config_id={SdkConfigId}");
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);

        using var doc = JsonDocument.Parse(await resp.Content.ReadAsStringAsync());
        var root = doc.RootElement;

        // 5 successful sdkprobe attempts (4 normal + 1 anomaly); failed excluded.
        Assert.Equal(5, root.GetProperty("attempt_count").GetInt64());
        Assert.Equal(1, root.GetProperty("split_anomaly_count").GetInt64());

        var group = Assert.Single(root.GetProperty("groups").EnumerateArray().ToList());
        Assert.Equal(SdkConfigId, group.GetProperty("config_id").GetGuid());
        Assert.Equal("sdk-checkout-api", group.GetProperty("config_name").GetString());
        Assert.Equal(1, group.GetProperty("run_count").GetInt32());
        Assert.Equal(5, group.GetProperty("attempt_count").GetInt32());
        Assert.Equal(1, group.GetProperty("split_anomaly_count").GetInt32());

        // Servers: 180,180,150,120,130 → median 150. Networks: 20,60,150,100,0
        // (anomaly floors at 0) → median 60. Walls: 200,240,300,220,100 → median 220.
        Assert.Equal(150.0, group.GetProperty("median_server_ms").GetDouble(), 3);
        Assert.Equal(60.0, group.GetProperty("median_network_ms").GetDouble(), 3);
        Assert.Equal(220.0, group.GetProperty("median_wall_ms").GetDouble(), 3);

        // 150 >= 0.6*220 = 132 → server_bound.
        Assert.Equal("server_bound", group.GetProperty("verdict").GetString());
        Assert.Contains("investigate your application", group.GetProperty("main_issue").GetString());

        // Overall (single config here) agrees.
        Assert.Equal("server_bound", root.GetProperty("overall_verdict").GetString());
        Assert.Equal(150.0, root.GetProperty("overall_median_server_ms").GetDouble(), 3);
    }
}
