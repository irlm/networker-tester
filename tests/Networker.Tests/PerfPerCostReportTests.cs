using System.Net;
using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.Data.Entities;

namespace Networker.Tests;

/// <summary>
/// End-to-end tests for <c>GET /api/projects/{id}/reports/perf-per-cost</c>
/// against a real Postgres (Testcontainers) and the real app.
///
/// Split across two classes on purpose: <see cref="PerfPerCostAuthzTests"/>
/// runs against the pristine fixture (tester probe schema ABSENT — proving
/// the 42P01 → empty-report path and the authz matrix), while
/// <see cref="PerfPerCostAggregationTests"/> gets its own container, creates
/// the tester-owned V001 tables, seeds probe rows, and asserts the actual
/// SQL aggregation + value-score arithmetic end to end.
/// </summary>
public sealed class PerfPerCostAuthzTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fixture;

    public PerfPerCostAuthzTests(ControlPlaneFixture fixture) => _fixture = fixture;

    private static string Url(string projectId) =>
        $"/api/projects/{projectId}/reports/perf-per-cost";

    [Fact]
    public async Task Report_requires_authentication()
    {
        var resp = await _fixture.CreateClient()
            .GetAsync(Url(ControlPlaneFixture.SeededProjectId));

        Assert.Equal(HttpStatusCode.Unauthorized, resp.StatusCode);
    }

    [Fact]
    public async Task Report_forbids_non_member_project()
    {
        var resp = await _fixture.CreateAuthenticatedClient()
            .GetAsync(Url("proj-not-a-member"));

        Assert.Equal(HttpStatusCode.Forbidden, resp.StatusCode);
    }

    [Fact]
    public async Task Report_is_member_read_so_a_viewer_gets_200()
    {
        // No tester probe schema exists in this container (the fixture only
        // materializes the EF model) — the report must still be a valid,
        // EMPTY 200 for a read-only member, with the cost-table metadata and
        // formulas present. This is exactly the 42P01-handled path.
        var resp = await _fixture.CreateViewerClient()
            .GetAsync(Url(ControlPlaneFixture.SeededProjectId));

        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        using var doc = JsonDocument.Parse(await resp.Content.ReadAsStringAsync());
        var root = doc.RootElement;

        Assert.Empty(root.GetProperty("groups").EnumerateArray());
        Assert.Empty(root.GetProperty("missing_cost_skus").EnumerateArray());
        Assert.Equal(0, root.GetProperty("providers_with_data").GetInt32());
        Assert.False(string.IsNullOrEmpty(
            root.GetProperty("cost_table").GetProperty("disclaimer").GetString()));
        Assert.Contains("p95_ms * hourly_usd",
            root.GetProperty("formulas").GetProperty("latency_cost_index").GetString());
        Assert.Contains("/ hourly_usd",
            root.GetProperty("formulas").GetProperty("mbps_per_dollar_hour").GetString());
    }
}

public sealed class PerfPerCostAggregationTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fixture;

    public PerfPerCostAggregationTests(ControlPlaneFixture fixture) => _fixture = fixture;

    /// <summary>
    /// Seeds: the tester-owned V001 slice (RequestAttempt/HttpResult), an AWS
    /// tester priced by the table, a GCP tester whose SKU is NOT in the table,
    /// and completed runs with hand-picked probe rows so the expected medians/
    /// p95s/value scores are computable by hand:
    ///
    /// azure/Standard_B1s/eastus ($0.0104): http1 latencies 100+200 ms
    ///   → median 150, p95 195, latency_cost_index 195×0.0104 = 2.028
    /// aws/t3.micro/us-east-1 ($0.0104): download throughput 800+1000 Mbps
    ///   → median 900, mbps_per_dollar_hour 900/0.0104 = 86538.4615
    /// gcp/weird-size/us-east1 (unpriced): tcp latency 50 ms wall time
    ///   → perf shown, hourly_usd null, listed in missing_cost_skus
    /// </summary>
    private async Task SeedAsync()
    {
        await using var ctx = _fixture.NewDbContext();
        if (await ctx.TestRuns.AnyAsync(r => r.WorkerId == "ppc-itest"))
        {
            return; // already seeded by another test in this class
        }

        var azureTesterId = await ctx.ProjectTesters
            .Where(t => t.Name == ControlPlaneFixture.SeededTesterName)
            .Select(t => t.TesterId)
            .SingleAsync();

        var now = new DateTime(2026, 7, 20, 10, 0, 0, DateTimeKind.Utc);
        var awsTesterId = Guid.NewGuid();
        var gcpTesterId = Guid.NewGuid();
        ctx.ProjectTesters.AddRange(
            NewTester(awsTesterId, "ppc-aws", "aws", "us-east-1", "t3.micro", now),
            NewTester(gcpTesterId, "ppc-gcp", "gcp", "us-east1", "weird-size", now));

        var azureRun = Guid.NewGuid();
        var awsRun = Guid.NewGuid();
        var gcpRun = Guid.NewGuid();
        var runningRun = Guid.NewGuid(); // must be EXCLUDED (not completed)
        ctx.TestRuns.AddRange(
            NewRun(azureRun, azureTesterId, "completed", now),
            NewRun(awsRun, awsTesterId, "completed", now),
            NewRun(gcpRun, gcpTesterId, "completed", now),
            NewRun(runningRun, azureTesterId, "running", now));
        await ctx.SaveChangesAsync();

        // The tester-owned probe slice (V001 shape, unquoted → lowercase) +
        // hand-picked rows. The failed attempt and the running run's attempt
        // must not influence the aggregates.
        Guid a1 = Guid.NewGuid(), a2 = Guid.NewGuid(), a3 = Guid.NewGuid(),
             a4 = Guid.NewGuid(), a5 = Guid.NewGuid();
        var conn = ctx.Database.GetDbConnection();
        await conn.OpenAsync();
        await using var cmd = conn.CreateCommand();
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
            CREATE TABLE IF NOT EXISTS HttpResult (
                HttpId UUID PRIMARY KEY,
                AttemptId UUID NOT NULL,
                NegotiatedVersion VARCHAR(20) NOT NULL,
                StatusCode INT NOT NULL,
                TtfbMs DOUBLE PRECISION NOT NULL,
                TotalDurationMs DOUBLE PRECISION NOT NULL,
                StartedAt TIMESTAMPTZ NOT NULL,
                ThroughputMbps DOUBLE PRECISION NULL
            );

            -- azure http1: 100 ms + 200 ms (HttpResult.TotalDurationMs wins
            -- over wall time) + one FAILED attempt that must be ignored.
            {Attempt(a1, azureRun, "http1", 1, 0, true)}
            {Http(a1, 100.0, null)}
            {Attempt(a2, azureRun, "http1", 2, 0, true)}
            {Http(a2, 200.0, null)}
            {Attempt(Guid.NewGuid(), azureRun, "http1", 3, 0, false)}

            -- aws download: throughput samples 800 + 1000 Mbps.
            {Attempt(a3, awsRun, "download", 1, 5, true)}
            {Http(a3, 5000.0, 800.0)}
            {Attempt(a4, awsRun, "download", 2, 5, true)}
            {Http(a4, 5000.0, 1000.0)}

            -- gcp tcp: no HttpResult → latency = wall time (50 ms).
            {Attempt(Guid.NewGuid(), gcpRun, "tcp", 1, 0.05, true)}

            -- attempt on a RUNNING run: excluded by status filter.
            {Attempt(a5, runningRun, "http1", 1, 0, true)}
            {Http(a5, 9999.0, null)}
            """;
        await cmd.ExecuteNonQueryAsync();
    }

    /// <summary>INSERT for one attempt; <paramref name="wallSeconds"/> sets
    /// FinishedAt-StartedAt for modes measured by wall time.</summary>
    private static string Attempt(
        Guid id, Guid runId, string protocol, int seq, double wallSeconds, bool success)
    {
        var wall = wallSeconds.ToString("0.0###", System.Globalization.CultureInfo.InvariantCulture);
        return $"""
            INSERT INTO RequestAttempt
                (AttemptId, RunId, Protocol, SequenceNum, StartedAt, FinishedAt, Success, RetryCount)
            VALUES ('{id}', '{runId}', '{protocol}', {seq},
                    '2026-07-20T10:00:00Z',
                    '2026-07-20T10:00:00Z'::timestamptz + interval '{wall} seconds',
                    {(success ? "TRUE" : "FALSE")}, 0);
            """;
    }

    /// <summary>INSERT the HttpResult carrying the attempt's HTTP timing.</summary>
    private static string Http(Guid attemptId, double totalMs, double? mbps)
    {
        var inv = System.Globalization.CultureInfo.InvariantCulture;
        return $"""
            INSERT INTO HttpResult
                (HttpId, AttemptId, NegotiatedVersion, StatusCode, TtfbMs, TotalDurationMs, StartedAt, ThroughputMbps)
            VALUES ('{Guid.NewGuid()}', '{attemptId}', 'h1', 200, 1.0, {totalMs.ToString("0.0###", inv)},
                    '2026-07-20T10:00:00Z', {(mbps is null ? "NULL" : mbps.Value.ToString("0.0###", inv))});
            """;
    }

    private static ProjectTester NewTester(
        Guid id, string name, string cloud, string region, string vmSize, DateTime now) => new()
    {
        TesterId = id,
        ProjectId = ControlPlaneFixture.SeededProjectId,
        Name = name,
        Cloud = cloud,
        Region = region,
        VmSize = vmSize,
        SshUser = "tester",
        PowerState = "running",
        Allocation = "on-demand",
        AutoShutdownEnabled = false,
        AutoShutdownLocalHour = 0,
        ShutdownDeferralCount = 0,
        AutoProbeEnabled = false,
        BenchmarkRunCount = 0,
        CreatedAt = now,
    };

    private static TestRun NewRun(Guid id, Guid testerId, string status, DateTime now) => new()
    {
        Id = id,
        TestConfigId = ControlPlaneFixture.SeededConfigId,
        ProjectId = ControlPlaneFixture.SeededProjectId,
        Status = status,
        StartedAt = now,
        FinishedAt = status == "completed" ? now.AddMinutes(1) : null,
        SuccessCount = 2,
        FailureCount = 0,
        WorkerId = "ppc-itest",
        CreatedAt = now,
    };

    [Fact]
    public async Task Aggregates_and_value_scores_match_hand_computed_numbers()
    {
        await SeedAsync();
        var client = _fixture.CreateAuthenticatedClient();

        var resp = await client.GetAsync(
            $"/api/projects/{ControlPlaneFixture.SeededProjectId}/reports/perf-per-cost");

        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        using var doc = JsonDocument.Parse(await resp.Content.ReadAsStringAsync());
        var root = doc.RootElement;

        Assert.Equal(3, root.GetProperty("providers_with_data").GetInt32());
        var groups = root.GetProperty("groups").EnumerateArray().ToList();
        Assert.Equal(3, groups.Count);

        // aws/t3.micro — throughput family: median 900 Mbps / $0.0104.
        var aws = groups.Single(g => g.GetProperty("provider").GetString() == "aws");
        Assert.Equal("t3.micro", aws.GetProperty("vm_size").GetString());
        Assert.Equal(0.0104m, aws.GetProperty("hourly_usd").GetDecimal());
        Assert.Equal(JsonValueKind.Null, aws.GetProperty("cost_note").ValueKind);
        var thru = aws.GetProperty("families").EnumerateArray()
            .Single(f => f.GetProperty("family").GetString() == "thru");
        Assert.Equal("throughput_mbps", thru.GetProperty("metric_label").GetString());
        Assert.Equal(1, thru.GetProperty("run_count").GetInt32());
        Assert.Equal(2, thru.GetProperty("sample_count").GetInt32());
        Assert.Equal(900.0, thru.GetProperty("median").GetDouble());
        Assert.Equal("mbps_per_dollar_hour", thru.GetProperty("value_metric").GetString());
        Assert.Equal(86538.4615, thru.GetProperty("value_score").GetDouble(), 4);

        // azure/Standard_B1s — http family: median 150, p95 195,
        // index 195 × 0.0104 = 2.028. The failed attempt and the running run's
        // 9999 ms attempt must not appear (they'd shift the percentiles).
        var azure = groups.Single(g => g.GetProperty("provider").GetString() == "azure");
        var http = azure.GetProperty("families").EnumerateArray()
            .Single(f => f.GetProperty("family").GetString() == "http");
        Assert.Equal("latency_ms", http.GetProperty("metric_label").GetString());
        Assert.Equal(2, http.GetProperty("sample_count").GetInt32());
        Assert.Equal(150.0, http.GetProperty("median").GetDouble());
        Assert.Equal(195.0, http.GetProperty("p95_ms").GetDouble());
        Assert.Equal("latency_cost_index", http.GetProperty("value_metric").GetString());
        Assert.Equal(2.028, http.GetProperty("value_score").GetDouble(), 4);

        // gcp/weird-size — unpriced: perf present (50 ms wall time), cost '—'.
        var gcp = groups.Single(g => g.GetProperty("provider").GetString() == "gcp");
        Assert.Equal(JsonValueKind.Null, gcp.GetProperty("hourly_usd").ValueKind);
        Assert.Contains("no price row", gcp.GetProperty("cost_note").GetString());
        var net = gcp.GetProperty("families").EnumerateArray()
            .Single(f => f.GetProperty("family").GetString() == "net");
        Assert.Equal(50.0, net.GetProperty("median").GetDouble(), 3);
        Assert.Equal(JsonValueKind.Null, net.GetProperty("value_score").ValueKind);

        var missing = root.GetProperty("missing_cost_skus").EnumerateArray().ToList();
        var miss = Assert.Single(missing);
        Assert.Equal("gcp", miss.GetProperty("provider").GetString());
        Assert.Equal("weird-size", miss.GetProperty("vm_size").GetString());
    }
}
