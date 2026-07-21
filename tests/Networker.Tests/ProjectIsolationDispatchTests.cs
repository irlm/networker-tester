using System.Net;
using System.Net.Http.Json;
using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.Data.Entities;

namespace Networker.Tests;

/// <summary>
/// End-to-end project-isolation guards (project-isolation audit §3 / §6c),
/// against a real Postgres (Testcontainers) and the booted app.
///
/// <list type="bullet">
///   <item><b>§3 (P2)</b> — the app-network report's <c>test_config</c> join is
///   project-scoped in SQL, so even a mis-written <c>test_run</c> in project A
///   that points at a project-B config can never surface project B's config name
///   in project A's report.</item>
///   <item><b>§6c (P2)</b> — a launch that pins a <c>tester_id</c> belonging to a
///   different project than the config is rejected with 400.</item>
/// </list>
/// </summary>
public sealed class ProjectIsolationDispatchTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fixture;

    public ProjectIsolationDispatchTests(ControlPlaneFixture fixture) => _fixture = fixture;

    private const string ProjectB = "proj-itest-b02";

    // ── §3: app-network report excludes another project's config name ─────────

    [Fact]
    public async Task AppNetwork_report_excludes_other_projects_config_even_with_crossed_run()
    {
        var projectAConfig = Guid.NewGuid();
        var projectBConfig = Guid.NewGuid();
        var crossedRun = Guid.NewGuid(); // project-A run pointing at project-B's config
        var cleanRun = Guid.NewGuid();   // legit project-A run against project-A's config

        await using (var ctx = _fixture.NewDbContext())
        {
            var now = new DateTime(2026, 7, 21, 12, 0, 0, DateTimeKind.Utc);

            // Second project + a sdkprobe config that belongs to it.
            if (!await ctx.Projects.AnyAsync(p => p.ProjectId == ProjectB))
            {
                ctx.Projects.Add(new Project
                {
                    ProjectId = ProjectB,
                    Name = "Project B",
                    Slug = "itest-b",
                    Settings = "{}",
                    CreatedAt = now,
                    UpdatedAt = now,
                });
            }
            ctx.TestConfigs.Add(new TestConfig
            {
                Id = projectBConfig,
                ProjectId = ProjectB,
                Name = "PROJECT-B-SECRET-CONFIG-NAME",
                EndpointKind = "network",
                EndpointRef = """{"kind":"network","host":"b.example.com"}""",
                Workload = """{"modes":["sdkprobe"]}""",
                CreatedAt = now,
                UpdatedAt = now,
            });
            ctx.TestConfigs.Add(new TestConfig
            {
                Id = projectAConfig,
                ProjectId = ControlPlaneFixture.SeededProjectId,
                Name = "project-a-clean-config",
                EndpointKind = "network",
                EndpointRef = """{"kind":"network","host":"a.example.com"}""",
                Workload = """{"modes":["sdkprobe"]}""",
                CreatedAt = now,
                UpdatedAt = now,
            });

            // The invariant-violating run: project_id = A, but test_config_id = B's
            // config. This is exactly the state the SQL join fix must survive.
            ctx.TestRuns.Add(new TestRun
            {
                Id = crossedRun,
                TestConfigId = projectBConfig,
                ProjectId = ControlPlaneFixture.SeededProjectId,
                Status = "completed",
                StartedAt = now,
                FinishedAt = now.AddMinutes(1),
                SuccessCount = 1,
                CreatedAt = now,
            });
            // A clean project-A run so the report has at least one legit group.
            ctx.TestRuns.Add(new TestRun
            {
                Id = cleanRun,
                TestConfigId = projectAConfig,
                ProjectId = ControlPlaneFixture.SeededProjectId,
                Status = "completed",
                StartedAt = now,
                FinishedAt = now.AddMinutes(1),
                SuccessCount = 1,
                CreatedAt = now,
            });
            await ctx.SaveChangesAsync();

            // Seed the tester-owned V001 probe slice + one successful sdkprobe
            // attempt (with a server-timing row) for BOTH runs, so both would
            // otherwise appear in the aggregation.
            var conn = ctx.Database.GetDbConnection();
            await conn.OpenAsync();
            await using var cmd = conn.CreateCommand();
            var attCrossed = Guid.NewGuid();
            var attClean = Guid.NewGuid();
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

                INSERT INTO RequestAttempt
                    (AttemptId, RunId, Protocol, SequenceNum, StartedAt, FinishedAt, Success, RetryCount)
                VALUES ('{attCrossed}', '{crossedRun}', 'sdkprobe', 1,
                        '2026-07-21T12:00:00Z',
                        '2026-07-21T12:00:00Z'::timestamptz + interval '0.2 seconds', TRUE, 0);
                INSERT INTO ServerTimingResult (ServerId, AttemptId, TotalServerMs)
                VALUES ('{Guid.NewGuid()}', '{attCrossed}', 120.0);

                INSERT INTO RequestAttempt
                    (AttemptId, RunId, Protocol, SequenceNum, StartedAt, FinishedAt, Success, RetryCount)
                VALUES ('{attClean}', '{cleanRun}', 'sdkprobe', 1,
                        '2026-07-21T12:00:00Z',
                        '2026-07-21T12:00:00Z'::timestamptz + interval '0.2 seconds', TRUE, 0);
                INSERT INTO ServerTimingResult (ServerId, AttemptId, TotalServerMs)
                VALUES ('{Guid.NewGuid()}', '{attClean}', 120.0);
                """;
            await cmd.ExecuteNonQueryAsync();
        }

        var client = _fixture.CreateAuthenticatedClient();
        var resp = await client.GetAsync(
            $"/api/projects/{ControlPlaneFixture.SeededProjectId}/reports/app-network");
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);

        var bodyText = await resp.Content.ReadAsStringAsync();

        // The whole point: project B's config name must NOT appear anywhere in
        // project A's report, even though a crossed run referenced it.
        Assert.DoesNotContain("PROJECT-B-SECRET-CONFIG-NAME", bodyText);

        using var doc = JsonDocument.Parse(bodyText);
        var configNames = doc.RootElement.GetProperty("groups").EnumerateArray()
            .Select(g => g.GetProperty("config_name").GetString())
            .ToList();
        Assert.DoesNotContain("PROJECT-B-SECRET-CONFIG-NAME", configNames);
        // The clean project-A config is present.
        Assert.Contains("project-a-clean-config", configNames);
    }

    // ── §6c: launch rejects a foreign-project tester_id ───────────────────────

    [Fact]
    public async Task Launch_rejects_tester_id_from_another_project()
    {
        var foreignTester = Guid.NewGuid();
        await using (var ctx = _fixture.NewDbContext())
        {
            var now = new DateTime(2026, 7, 21, 12, 0, 0, DateTimeKind.Utc);
            if (!await ctx.Projects.AnyAsync(p => p.ProjectId == ProjectB))
            {
                ctx.Projects.Add(new Project
                {
                    ProjectId = ProjectB,
                    Name = "Project B",
                    Slug = "itest-b",
                    Settings = "{}",
                    CreatedAt = now,
                    UpdatedAt = now,
                });
            }
            // A tester that belongs to project B, NOT the seeded project A.
            ctx.ProjectTesters.Add(new ProjectTester
            {
                TesterId = foreignTester,
                ProjectId = ProjectB,
                Name = "b-tester",
                Cloud = "azure",
                Region = "eastus",
                VmSize = "Standard_B1s",
                SshUser = "azureuser",
                PowerState = "running",
                Allocation = "on-demand",
                CreatedAt = now,
                UpdatedAt = now,
            });
            await ctx.SaveChangesAsync();
        }

        var client = _fixture.CreateAuthenticatedClient(); // operator on project A

        // Launch the SEEDED project-A config, but pin a project-B tester.
        var resp = await client.PostAsJsonAsync(
            $"/api/v2/test-configs/{ControlPlaneFixture.SeededConfigId}/launch",
            new { tester_id = foreignTester });

        Assert.Equal(HttpStatusCode.BadRequest, resp.StatusCode);
    }

    [Fact]
    public async Task Launch_with_no_tester_id_still_succeeds()
    {
        var client = _fixture.CreateAuthenticatedClient();

        // No pinned tester → the §6c validation is skipped; launch proceeds
        // (the run may end up queued if no agent is online — a 200 either way).
        var resp = await client.PostAsJsonAsync(
            $"/api/v2/test-configs/{ControlPlaneFixture.SeededConfigId}/launch",
            new { });

        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
    }
}
