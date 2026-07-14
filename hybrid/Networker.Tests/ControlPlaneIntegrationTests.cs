using System.Net;
using System.Net.Http.Json;
using Microsoft.AspNetCore.Mvc.Testing;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Networker.Data;
using Networker.Data.Entities;
using Testcontainers.PostgreSql;

namespace Networker.Tests;

/// End-to-end proof that the Phase-2 EF model materializes against a REAL
/// Postgres and the control-plane REST endpoints return the seeded rows.
///
/// This is the automated version of the manual "does EF read the live schema"
/// check: a disposable Postgres is spun up in Docker (Testcontainers), the
/// scaffolded model is materialized into it (EnsureCreated), rows are seeded,
/// and the actual control-plane app is booted in-process (WebApplicationFactory)
/// pointed at that container. If a future model change stops mapping a column
/// the endpoint reads, THIS test fails — not production.
///
/// Requires Docker (present on GitHub ubuntu runners and local dev). The whole
/// class shares one container/app via IClassFixture so the ~2s container start
/// is paid once.
public sealed class ControlPlaneFixture : WebApplicationFactory<Program>, IAsyncLifetime
{
    private readonly PostgreSqlContainer _db = new PostgreSqlBuilder()
        .WithImage("postgres:16-alpine")
        .WithDatabase("networker_core")
        .WithUsername("networker")
        .WithPassword("networker")
        .Build();

    public const string SeededProjectId = "proj-itest-001";
    public const string SeededTesterName = "itest-tester-eastus";

    public async Task InitializeAsync()
    {
        await _db.StartAsync();

        // Materialize the scaffolded model into the fresh container and seed a
        // project + tester. We render the model's OWN DDL (GenerateCreateScript)
        // and run it — so this proves the model is internally consistent (valid
        // column types, no dangling FK to an unmapped table) against a real
        // engine, not a mock.
        //
        // We strip the `CREATE EXTENSION` lines: prod runs on TimescaleDB and
        // the model declares the timescaledb / timescaledb_toolkit extensions
        // for the probe-result hypertables — but those hypertables aren't in
        // this scaffolded slice (only the config/metadata tables are, all
        // standard column types), so the extensions are irrelevant here and
        // requiring the heavy HA image in CI would buy nothing.
        var options = new DbContextOptionsBuilder<NetworkerDbContext>()
            .UseNpgsql(_db.GetConnectionString())
            .Options;
        await using var ctx = new NetworkerDbContext(options);
        var ddl = string.Join(
            '\n',
            ctx.Database.GenerateCreateScript()
                .Split('\n')
                .Where(line => !line.TrimStart()
                    .StartsWith("CREATE EXTENSION", StringComparison.OrdinalIgnoreCase)));
        // Run over the raw ADO connection, not ExecuteSqlRaw: the DDL contains
        // literal braces (jsonb '{}' defaults) that EF's parameter formatter
        // would misread as {0}-style placeholders.
        var conn = ctx.Database.GetDbConnection();
        await conn.OpenAsync();
        await using (var cmd = conn.CreateCommand())
        {
            cmd.CommandText = ddl;
            await cmd.ExecuteNonQueryAsync();
        }

        var now = new DateTime(2026, 1, 1, 0, 0, 0, DateTimeKind.Utc);
        ctx.Projects.Add(new Project
        {
            ProjectId = SeededProjectId,
            Name = "Integration Test Project",
            Slug = "itest",
            Settings = "{}",
            CreatedAt = now,
            UpdatedAt = now,
            DeleteProtection = false,
        });
        ctx.ProjectTesters.Add(new ProjectTester
        {
            TesterId = Guid.NewGuid(),
            ProjectId = SeededProjectId,
            Name = SeededTesterName,
            Cloud = "azure",
            Region = "eastus",
            VmSize = "Standard_B1s",
            SshUser = "azureuser",
            PowerState = "running",
            Allocation = "on-demand",
            AutoShutdownEnabled = false,
            AutoShutdownLocalHour = 0,
            ShutdownDeferralCount = 0,
            AutoProbeEnabled = false,
            BenchmarkRunCount = 0,
            CreatedAt = now,
        });
        await ctx.SaveChangesAsync();
    }

    // Point the booted control-plane app at the container instead of its
    // default localhost connection string.
    protected override void ConfigureWebHost(Microsoft.AspNetCore.Hosting.IWebHostBuilder builder)
    {
        builder.UseSetting("ConnectionStrings:Networker", _db.GetConnectionString());
    }

    public new async Task DisposeAsync()
    {
        await _db.DisposeAsync();
        await base.DisposeAsync();
    }
}

public sealed class ControlPlaneIntegrationTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fixture;

    public ControlPlaneIntegrationTests(ControlPlaneFixture fixture) => _fixture = fixture;

    [Fact]
    public async Task Health_endpoint_reports_db_ok_against_real_postgres()
    {
        var client = _fixture.CreateClient();

        var resp = await client.GetAsync("/api/health");

        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        var body = await resp.Content.ReadFromJsonAsync<HealthResponse>();
        Assert.NotNull(body);
        Assert.Equal("ok", body!.Status);
        Assert.Equal("ok", body.Db);
    }

    [Fact]
    public async Task Testers_endpoint_returns_seeded_tester_from_ef_model()
    {
        var client = _fixture.CreateClient();

        var resp = await client.GetAsync(
            $"/api/projects/{ControlPlaneFixture.SeededProjectId}/testers");

        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        var testers = await resp.Content.ReadFromJsonAsync<List<TesterRow>>();
        Assert.NotNull(testers);
        var tester = Assert.Single(testers!);
        Assert.Equal(ControlPlaneFixture.SeededTesterName, tester.Name);
        Assert.Equal("azure", tester.Cloud);
        Assert.Equal("eastus", tester.Region);
        Assert.Equal("running", tester.Power_State);
    }

    [Fact]
    public async Task Testers_endpoint_returns_empty_for_unknown_project()
    {
        var client = _fixture.CreateClient();

        var resp = await client.GetAsync("/api/projects/does-not-exist/testers");

        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        var testers = await resp.Content.ReadFromJsonAsync<List<TesterRow>>();
        Assert.NotNull(testers);
        Assert.Empty(testers!);
    }

    private sealed record HealthResponse(string Status, string Version, string Db);

    private sealed record TesterRow(
        Guid Tester_Id,
        string Name,
        string Cloud,
        string Region,
        string Vm_Size,
        string Power_State,
        string Allocation);
}
