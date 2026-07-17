using System.Net;
using System.Net.Http.Headers;
using System.Net.Http.Json;
using Microsoft.AspNetCore.Mvc.Testing;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Networker.ControlPlane.Auth;
using Networker.Data;
using Networker.Data.Entities;
using Networker.Security;
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
    public static readonly Guid SeededUserId = Guid.Parse("11111111-1111-4111-8111-111111111111");
    public const string SeededUserEmail = "itest@networker.local";
    public static readonly Guid SeededConfigId = Guid.Parse("22222222-2222-4222-8222-222222222222");

    /// A fresh DbContext against the same container — for tests to assert what
    /// a write endpoint persisted.
    public NetworkerDbContext NewDbContext() =>
        new(new DbContextOptionsBuilder<NetworkerDbContext>().UseNpgsql(_db.GetConnectionString()).Options);

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
        // A user + membership so the M1 project-scoped (ProjectMember) endpoints
        // can be exercised end-to-end with a real minted JWT.
        ctx.DashUsers.Add(new DashUser
        {
            UserId = SeededUserId,
            Email = SeededUserEmail,
            Role = "operator",
            Status = "active",
            AuthProvider = "local",
            IsPlatformAdmin = false,
            MustChangePassword = false,
            SsoOnly = false,
            CreatedAt = now,
        });
        ctx.ProjectMembers.Add(new ProjectMember
        {
            ProjectId = SeededProjectId,
            UserId = SeededUserId,
            Role = "operator",
            Status = "active",
            JoinedAt = now,
        });
        // A test config so the M3 launch/dispatch write path can be exercised.
        ctx.TestConfigs.Add(new TestConfig
        {
            Id = SeededConfigId,
            ProjectId = SeededProjectId,
            Name = "itest-config",
            EndpointKind = "network",
            EndpointRef = "{}",
            Workload = "{}",
            MaxDurationSecs = 60,
            CreatedAt = now,
            UpdatedAt = now,
        });
        await ctx.SaveChangesAsync();
    }

    // Point the booted control-plane app at the container instead of its
    // default localhost connection string. Startup schema migrations are
    // opted out: this fixture materializes the schema from the EF model's own
    // DDL (above), so replaying the V0NN chain on top of it would collide.
    protected override void ConfigureWebHost(Microsoft.AspNetCore.Hosting.IWebHostBuilder builder)
    {
        builder.UseSetting("ConnectionStrings:Networker", _db.GetConnectionString());
        builder.UseSetting("NETWORKER_RUN_MIGRATIONS", "0");
    }

    /// A client carrying a JWT for the seeded user, minted by the app's OWN
    /// JwtTokenService (same signing key the JwtBearer handler validates with),
    /// so the M0 auth pipeline is exercised for real.
    public HttpClient CreateAuthenticatedClient()
    {
        var client = CreateClient();
        var tokens = Services.GetRequiredService<JwtTokenService>();
        var jwt = tokens.CreateToken(SeededUserId, SeededUserEmail, "operator", isPlatformAdmin: false);
        client.DefaultRequestHeaders.Authorization = new AuthenticationHeaderValue("Bearer", jwt);
        return client;
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
    public async Task Testers_endpoint_returns_seeded_tester_for_project_member()
    {
        var client = _fixture.CreateAuthenticatedClient();

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
    public async Task Testers_endpoint_requires_authentication()
    {
        var client = _fixture.CreateClient();

        var resp = await client.GetAsync(
            $"/api/projects/{ControlPlaneFixture.SeededProjectId}/testers");

        Assert.Equal(HttpStatusCode.Unauthorized, resp.StatusCode);
    }

    [Fact]
    public async Task Testers_endpoint_forbids_non_member_project()
    {
        var client = _fixture.CreateAuthenticatedClient();

        // The seeded user is a member of SeededProjectId only — a project they
        // don't belong to must be rejected by the ProjectMember policy.
        var resp = await client.GetAsync("/api/projects/proj-not-a-member/testers");

        Assert.Equal(HttpStatusCode.Forbidden, resp.StatusCode);
    }

    [Fact]
    public async Task Project_detail_endpoint_returns_seeded_project_for_member()
    {
        var client = _fixture.CreateAuthenticatedClient();

        var resp = await client.GetAsync(
            $"/api/projects/{ControlPlaneFixture.SeededProjectId}");

        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        var body = await resp.Content.ReadFromJsonAsync<ProjectDetail>();
        Assert.NotNull(body);
        Assert.Equal(ControlPlaneFixture.SeededProjectId, body!.Project_Id);
        Assert.Equal("Integration Test Project", body.Name);
    }

    [Fact]
    public async Task Launch_creates_a_queued_run_via_the_dispatcher()
    {
        var client = _fixture.CreateAuthenticatedClient();

        var resp = await client.PostAsync(
            $"/api/v2/test-configs/{ControlPlaneFixture.SeededConfigId}/launch", content: null);

        // Launch now returns 200 + the full run row (frontend inserts it into the
        // runs list); the run is created queued (no agent connected in the test).
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);

        // The dispatcher created a test_run; with no agent connected in the test
        // it stays queued (the redispatcher/agent would pick it up in prod).
        await using var ctx = _fixture.NewDbContext();
        var run = await ctx.TestRuns
            .FirstOrDefaultAsync(r => r.TestConfigId == ControlPlaneFixture.SeededConfigId);
        Assert.NotNull(run);
        Assert.Equal("queued", run!.Status);
    }

    [Fact]
    public async Task Cloud_account_create_encrypts_credentials_and_round_trips()
    {
        var client = _fixture.CreateAuthenticatedClient();
        var body = new
        {
            name = "itest-azure",
            provider = "azure",
            // personal → only ProjectOperator required (shared accounts need Admin).
            personal = true,
            credentials = new { client_id = "cid", client_secret = "s3cr3t", tenant_id = "tid" },
        };

        var resp = await client.PostAsJsonAsync(
            $"/api/projects/{ControlPlaneFixture.SeededProjectId}/cloud-accounts", body);

        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);

        // The submitted secret is never stored in the clear: the row holds
        // ciphertext+nonce that the app's OWN cipher decrypts back to the input.
        var cipher = _fixture.Services.GetRequiredService<CredentialCipher>();
        await using var ctx = _fixture.NewDbContext();
        var acct = await ctx.CloudAccounts
            .FirstOrDefaultAsync(a => a.ProjectId == ControlPlaneFixture.SeededProjectId);
        Assert.NotNull(acct);
        Assert.NotEmpty(acct!.CredentialsEnc);

        var plaintext = cipher.Decrypt(acct.CredentialsEnc, acct.CredentialsNonce);
        using var doc = System.Text.Json.JsonDocument.Parse(plaintext);
        Assert.Equal("s3cr3t", doc.RootElement.GetProperty("client_secret").GetString());
    }

    [Fact]
    public async Task Deployment_create_persists_a_row()
    {
        var client = _fixture.CreateAuthenticatedClient();
        var body = new { name = "itest-deploy", config = new { version = 1, provider = "azure" } };

        var resp = await client.PostAsJsonAsync(
            $"/api/projects/{ControlPlaneFixture.SeededProjectId}/deployments", body);

        Assert.Equal(HttpStatusCode.Created, resp.StatusCode);

        await using var ctx = _fixture.NewDbContext();
        var dep = await ctx.Deployments
            .FirstOrDefaultAsync(d => d.ProjectId == ControlPlaneFixture.SeededProjectId
                                      && d.Name == "itest-deploy");
        Assert.NotNull(dep);
    }

    [Fact]
    public async Task Admin_users_endpoint_is_forbidden_for_non_platform_admin()
    {
        // The seeded user is a project operator, NOT a platform admin — the M5
        // GlobalAdmin gate must reject them from the platform user list.
        var client = _fixture.CreateAuthenticatedClient();

        var resp = await client.GetAsync("/api/users");

        Assert.Equal(HttpStatusCode.Forbidden, resp.StatusCode);
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

    private sealed record ProjectDetail(string Project_Id, string Name, string Slug);
}
