using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using Microsoft.Data.Sqlite;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Networker.ControlPlane.Endpoints;
using Networker.ControlPlane.Provisioning;
using Networker.Data;
using Networker.Data.Entities;
using Networker.Security;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// The SDK sample module's IO seam: how a deployment is recognised as carrying
/// a sample, what deploy config a create emits, how the token travels, and what
/// the per-language view reports end to end against a real (sqlite) database.
/// </summary>
public class SdkSampleEndpointsTests
{
    private const string ProjectId = "proj-sdksmp-01";

    private static CredentialCipher Cipher() =>
        new(Enumerable.Range(0, CredentialCipher.KeySize).Select(i => (byte)i).ToArray());

    private static (ServiceProvider Sp, SqliteConnection Conn) BuildHost()
    {
        var conn = new SqliteConnection("DataSource=:memory:");
        conn.Open();
        var services = new ServiceCollection();
        services.AddLogging();
        services.AddDbContext<NetworkerDbContext>(o => o.UseSqlite(conn));
        var sp = services.BuildServiceProvider();
        RunDispatcherTesterFkTests.CreateMinimalSchema(conn);
        return (sp, conn);
    }

    private static NetworkerDbContext Db(IServiceProvider sp) =>
        sp.CreateScope().ServiceProvider.GetRequiredService<NetworkerDbContext>();

    private static SdkSampleEndpoints.Target Target(
        SdkSamplePlan.Shape shape = SdkSamplePlan.Shape.Consolidated,
        string provider = "azure") =>
        new(shape, provider, provider == "docker" ? "local" : "eastus",
            provider == "docker" ? "container" : "Standard_B2s", null);

    private static JsonObject TokenNode(CredentialCipher cipher, string token = "sample-token-0123456789")
    {
        var (enc, nonce) = cipher.Encrypt(Encoding.UTF8.GetBytes(token));
        return new JsonObject
        {
            ["token_enc"] = Convert.ToBase64String(enc),
            ["token_nonce"] = Convert.ToBase64String(nonce),
        };
    }

    // ── Deploy-config shape ──────────────────────────────────────────────────

    [Fact]
    public void Consolidated_cloud_config_is_one_endpoint_carrying_every_language()
    {
        var cfg = SdkSampleEndpoints.BuildDeployConfig(Target(), ["go", "rust"], TokenNode(Cipher()));
        var endpoints = cfg["endpoints"]!.AsArray();
        Assert.Single(endpoints);
        var ep = endpoints[0]!.AsObject();
        Assert.Equal("azure", ep["provider"]!.GetValue<string>());
        Assert.Equal(["go", "rust"], ep["sdk_samples"]!.AsArray().Select(x => x!.GetValue<string>()));
        Assert.Equal("linux", ep["azure"]!["os"]!.GetValue<string>());
        Assert.Equal("Standard_B2s", ep["azure"]!["vm_size"]!.GetValue<string>());
        // Nothing else gets installed on a sample server: no proxy stack, no
        // reference-API language, no test run. Every extra is money and risk.
        Assert.Null(ep["http_stacks"]);
        Assert.Null(ep["languages"]);
        Assert.False(cfg["tests"]!["run_tests"]!.GetValue<bool>());
    }

    [Fact]
    public void Aws_and_gcp_use_their_own_size_and_name_keys()
    {
        var aws = SdkSampleEndpoints.BuildDeployConfig(Target(provider: "aws"), ["go"], TokenNode(Cipher()));
        var awsEp = aws["endpoints"]![0]!.AsObject();
        Assert.Equal("Standard_B2s", awsEp["aws"]!["instance_type"]!.GetValue<string>());
        Assert.NotNull(awsEp["aws"]!["instance_name"]);

        var gcp = SdkSampleEndpoints.BuildDeployConfig(Target(provider: "gcp"), ["go"], TokenNode(Cipher()));
        var gcpEp = gcp["endpoints"]![0]!.AsObject();
        Assert.Equal("Standard_B2s", gcpEp["gcp"]!["machine_type"]!.GetValue<string>());
        Assert.Equal("eastus-a", gcpEp["gcp"]!["zone"]!.GetValue<string>());
    }

    [Fact]
    public void Docker_gets_one_endpoint_per_sample_because_one_image_runs_one_sample()
    {
        var cfg = SdkSampleEndpoints.BuildDeployConfig(
            Target(provider: "docker"), ["go", "rust", "python"], TokenNode(Cipher()));
        var endpoints = cfg["endpoints"]!.AsArray();
        Assert.Equal(3, endpoints.Count);
        foreach (var node in endpoints)
        {
            Assert.Single(node!["sdk_samples"]!.AsArray());
        }
        // Endpoint i ⇄ endpoint_ips[i] is what discovery reads, so the two must
        // stay parallel — one endpoint per container.
    }

    [Fact]
    public void The_generated_config_passes_the_same_preflight_the_deploy_path_runs()
    {
        foreach (var provider in new[] { "azure", "aws", "gcp", "docker" })
        {
            var cfg = SdkSampleEndpoints.BuildDeployConfig(
                Target(provider: provider), ["go", "csharp"], TokenNode(Cipher()));
            Assert.Empty(DeployConfigPreflight.Validate(cfg));
        }
    }

    [Fact]
    public void Preflight_rejects_an_unknown_sample_and_a_windows_sample_host()
    {
        var unknown = JsonNode.Parse("""
            {"endpoints":[{"provider":"azure","sdk_samples":["cobol"],"azure":{"os":"linux"}}]}
            """);
        Assert.Contains(DeployConfigPreflight.Validate(unknown), e => e.Contains("cobol", StringComparison.Ordinal));

        var windows = JsonNode.Parse("""
            {"endpoints":[{"provider":"azure","sdk_samples":["go"],"azure":{"os":"windows"}}]}
            """);
        Assert.Contains(DeployConfigPreflight.Validate(windows), e => e.Contains("Linux", StringComparison.Ordinal));
    }

    [Fact]
    public void Group_for_shape_splits_deployments_but_not_languages()
    {
        Assert.Equal(
            [["go", "rust"]],
            SdkSampleEndpoints.GroupForShape(Target(), ["go", "rust"]));
        Assert.Equal(
            [["go"], ["rust"]],
            SdkSampleEndpoints.GroupForShape(Target(SdkSamplePlan.Shape.Separated), ["go", "rust"]));
    }

    // ── Token handling ───────────────────────────────────────────────────────

    [Fact]
    public void The_sample_token_never_appears_in_the_config_as_plaintext()
    {
        var cipher = Cipher();
        const string token = "super-secret-sample-token";
        var cfg = SdkSampleEndpoints.BuildDeployConfig(Target(), ["go"], TokenNode(cipher, token));
        var text = cfg.ToJsonString();
        Assert.DoesNotContain(token, text, StringComparison.Ordinal);

        // …and round-trips through the installer staging seam.
        var outcome = SdkSampleInstallerToken.Resolve(
            new ServiceCollection().AddSingleton(cipher).BuildServiceProvider().GetRequiredService<IServiceScopeFactory>(),
            text,
            Microsoft.Extensions.Logging.Abstractions.NullLogger.Instance);
        Assert.Equal(token, outcome.Env[SdkSampleInstallerToken.EnvVar]);
    }

    [Fact]
    public void A_config_without_samples_stages_no_token_and_no_note()
    {
        var outcome = SdkSampleInstallerToken.Resolve(
            new ServiceCollection().BuildServiceProvider().GetRequiredService<IServiceScopeFactory>(),
            """{"endpoints":[{"provider":"azure"}]}""",
            Microsoft.Extensions.Logging.Abstractions.NullLogger.Instance);
        Assert.Empty(outcome.Env);
        Assert.Null(outcome.Note);
    }

    [Fact]
    public void Minted_tokens_clear_the_contract_floor_of_16_bytes()
    {
        for (var i = 0; i < 20; i++)
        {
            var token = SdkSampleEndpoints.MintToken();
            Assert.True(Encoding.UTF8.GetByteCount(token) >= 16, token);
            Assert.DoesNotContain('+', token);
            Assert.DoesNotContain('/', token);
            Assert.DoesNotContain('=', token);
        }
    }

    [Fact]
    public void Merge_keeps_both_installer_environments_and_stays_null_when_empty()
    {
        var gcp = new Dictionary<string, string> { ["CLOUDSDK_CONFIG"] = "/tmp/x" };
        var sample = new Dictionary<string, string> { [SdkSampleInstallerToken.EnvVar] = "t" };
        var merged = SdkSampleInstallerToken.Merge(gcp, sample);
        Assert.Equal(2, merged!.Count);
        Assert.Null(SdkSampleInstallerToken.Merge(null, null));
        Assert.Null(SdkSampleInstallerToken.Merge(new Dictionary<string, string>(), null));
    }

    // ── Discovery ────────────────────────────────────────────────────────────

    private static SdkSampleEndpoints.DeploymentRow Row(
        string config, string? ips, string status = "completed", DateTime? at = null, string name = "sdk-1") =>
        new(Guid.NewGuid(), name, status, config, ips, null, at ?? DateTime.UtcNow);

    [Fact]
    public void Discovery_maps_each_language_to_its_endpoint_host()
    {
        var cfg = """
            {"endpoints":[
              {"provider":"azure","sdk_samples":["go"],"azure":{"region":"eastus","vm_size":"Standard_B2s"}},
              {"provider":"azure","sdk_samples":["rust"],"azure":{"region":"westus2","vm_size":"Standard_B1s"}}
            ]}
            """;
        var found = SdkSampleEndpoints.DiscoverSampleHosts([Row(cfg, """["10.0.0.1","10.0.0.2"]""")]);
        Assert.Equal("10.0.0.1", found["go"].Host);
        Assert.Equal("10.0.0.2", found["rust"].Host);
        Assert.Equal("westus2", found["rust"].Region);
        Assert.Equal("Standard_B1s", found["rust"].VmSize);
        Assert.Single(found["go"].Languages);
    }

    [Fact]
    public void A_consolidated_host_reports_every_language_it_carries()
    {
        var cfg = """{"endpoints":[{"provider":"azure","sdk_samples":["go","rust","python"]}]}""";
        var found = SdkSampleEndpoints.DiscoverSampleHosts([Row(cfg, """["10.0.0.1"]""")]);
        Assert.Equal(3, found["go"].Languages.Count);
        Assert.Equal(found["go"].DeploymentId, found["python"].DeploymentId);
        Assert.All(found.Values, h => Assert.Equal("10.0.0.1", h.Host));
    }

    [Fact]
    public void The_newest_deployment_wins_a_language()
    {
        var old = Row("""{"endpoints":[{"provider":"azure","sdk_samples":["go"]}]}""",
            """["10.0.0.1"]""", at: DateTime.UtcNow.AddDays(-2), name: "old");
        var recent = Row("""{"endpoints":[{"provider":"aws","sdk_samples":["go"]}]}""",
            """["10.0.0.9"]""", at: DateTime.UtcNow, name: "new");
        var found = SdkSampleEndpoints.DiscoverSampleHosts([old, recent]);
        Assert.Equal("new", found["go"].DeploymentName);
        Assert.Equal("10.0.0.9", found["go"].Host);
    }

    [Fact]
    public void Deployments_without_samples_and_malformed_configs_are_ignored()
    {
        var rows = new[]
        {
            Row("""{"endpoints":[{"provider":"azure","http_stacks":["nginx"]}]}""", """["10.0.0.1"]"""),
            Row("not json", """["10.0.0.2"]"""),
            Row("""{"endpoints":[{"provider":"azure","sdk_samples":["cobol"]}]}""", """["10.0.0.3"]"""),
        };
        Assert.Empty(SdkSampleEndpoints.DiscoverSampleHosts(rows));
    }

    [Fact]
    public void Endpoint_ref_breadcrumb_round_trips_and_rejects_foreign_configs()
    {
        var id = Guid.NewGuid();
        var json = SdkSampleEndpoints.BuildSampleEndpointRef("http://10.0.0.1:8105", id, "go");
        var link = SdkSampleEndpoints.ReadSampleLink(json);
        Assert.Equal((id, "go"), link);

        Assert.Null(SdkSampleEndpoints.ReadSampleLink("""{"kind":"network","host":"https://x"}"""));
        Assert.Null(SdkSampleEndpoints.ReadSampleLink("not json"));
        Assert.Null(SdkSampleEndpoints.ReadSampleLink(null));
        Assert.Null(SdkSampleEndpoints.ReadSampleLink(
            $$"""{"deployment_id":"{{id}}","sdk_sample":"cobol"}"""));
    }

    // ── End-to-end view (real DB) ────────────────────────────────────────────

    [Fact]
    public async Task View_reports_nothing_deployed_for_a_project_with_no_samples()
    {
        var (sp, conn) = BuildHost();
        using var _ = conn;
        var views = await SdkSampleEndpoints.BuildViewAsync(ProjectId, Db(sp), Cipher(), CancellationToken.None);

        Assert.Equal(SdkSampleCatalog.Samples.Count, views.Count);
        Assert.All(views, v => Assert.Equal(SdkSamplePlan.State.None, v.Verdict.StateOf));
        Assert.All(views, v => Assert.Null(v.Url));
    }

    [Fact]
    public async Task View_reports_a_completed_but_dead_sample_as_unhealthy_never_usable()
    {
        var (sp, conn) = BuildHost();
        using var _ = conn;
        var db = Db(sp);
        // 127.0.0.1 with nothing listening on the go port: the probe fails fast
        // (connection refused), which is exactly the dead-endpoint case.
        db.Deployments.Add(new Deployment
        {
            DeploymentId = Guid.NewGuid(),
            Name = "sdk-go-dead",
            Status = "completed",
            Config = """{"endpoints":[{"provider":"azure","sdk_samples":["go"],"azure":{"region":"eastus","vm_size":"Standard_B2s"}}]}""",
            EndpointIps = """["127.0.0.1"]""",
            ProjectId = ProjectId,
            CreatedAt = DateTime.UtcNow,
        });
        await db.SaveChangesAsync();

        var views = await SdkSampleEndpoints.BuildViewAsync(ProjectId, Db(sp), Cipher(), CancellationToken.None);
        var go = views.Single(v => v.Sample.Id == "go");
        Assert.Equal(SdkSamplePlan.State.Unhealthy, go.Verdict.StateOf);
        Assert.False(go.Verdict.Reusable);
        Assert.Equal("http://127.0.0.1:8105", go.Url);
        Assert.Equal("azure", go.Deployed!.Provider);
        Assert.Equal("Standard_B2s", go.Deployed.VmSize);
    }

    [Fact]
    public async Task View_reports_an_in_flight_deployment_as_deploying_and_does_not_probe_it()
    {
        var (sp, conn) = BuildHost();
        using var _ = conn;
        var db = Db(sp);
        db.Deployments.Add(new Deployment
        {
            DeploymentId = Guid.NewGuid(),
            Name = "sdk-rust-pending",
            Status = "running",
            Config = """{"endpoints":[{"provider":"docker","sdk_samples":["rust"]}]}""",
            EndpointIps = null,
            ProjectId = ProjectId,
            CreatedAt = DateTime.UtcNow,
        });
        await db.SaveChangesAsync();

        var views = await SdkSampleEndpoints.BuildViewAsync(ProjectId, Db(sp), Cipher(), CancellationToken.None);
        var rust = views.Single(v => v.Sample.Id == "rust");
        Assert.Equal(SdkSamplePlan.State.Deploying, rust.Verdict.StateOf);
        Assert.Equal(SdkSamplePlan.Action.Wait, rust.Verdict.Recommended);
    }

    [Fact]
    public async Task View_links_an_existing_registration_to_its_language()
    {
        var (sp, conn) = BuildHost();
        using var _ = conn;
        var db = Db(sp);
        var deploymentId = Guid.NewGuid();
        db.Deployments.Add(new Deployment
        {
            DeploymentId = deploymentId,
            Name = "sdk-go",
            Status = "completed",
            Config = """{"endpoints":[{"provider":"azure","sdk_samples":["go"]}]}""",
            EndpointIps = """["127.0.0.1"]""",
            ProjectId = ProjectId,
            CreatedAt = DateTime.UtcNow,
        });
        var configId = Guid.NewGuid();
        db.TestConfigs.Add(new TestConfig
        {
            Id = configId,
            ProjectId = ProjectId,
            Name = "LagHound Go sample",
            EndpointKind = "network",
            TestKind = TestConfigKinds.SdkProbe,
            EndpointRef = SdkSampleEndpoints.BuildSampleEndpointRef("http://127.0.0.1:8105", deploymentId, "go"),
            Workload = """{"modes":["sdkprobe"]}""",
            CreatedAt = DateTime.UtcNow,
            UpdatedAt = DateTime.UtcNow,
        });
        await db.SaveChangesAsync();

        var views = await SdkSampleEndpoints.BuildViewAsync(ProjectId, Db(sp), Cipher(), CancellationToken.None);
        Assert.Equal(configId, views.Single(v => v.Sample.Id == "go").Deployed!.RegisteredEndpointId);
        // A different language on the same deployment must NOT inherit it.
        Assert.Null(views.Single(v => v.Sample.Id == "rust").Deployed);
    }

    [Fact]
    public async Task View_never_leaks_another_projects_sample()
    {
        var (sp, conn) = BuildHost();
        using var _ = conn;
        var db = Db(sp);
        db.Deployments.Add(new Deployment
        {
            DeploymentId = Guid.NewGuid(),
            Name = "someone-elses",
            Status = "completed",
            Config = """{"endpoints":[{"provider":"azure","sdk_samples":["go"]}]}""",
            EndpointIps = """["10.9.9.9"]""",
            ProjectId = "proj-other",
            CreatedAt = DateTime.UtcNow,
        });
        await db.SaveChangesAsync();

        var views = await SdkSampleEndpoints.BuildViewAsync(ProjectId, Db(sp), Cipher(), CancellationToken.None);
        Assert.All(views, v => Assert.Null(v.Deployed));
    }

    // ── Wire shape ───────────────────────────────────────────────────────────

    [Fact]
    public void Catalog_wire_shape_carries_what_the_page_renders()
    {
        var json = JsonSerializer.Serialize(SdkSampleCatalog.ToWire());
        using var doc = JsonDocument.Parse(json);
        Assert.Equal("/laghound/echo", doc.RootElement.GetProperty("route_default").GetString());
        var first = doc.RootElement.GetProperty("samples").EnumerateArray().First();
        foreach (var field in new[] { "id", "language", "runtime", "description", "sdk_version", "port" })
        {
            Assert.True(first.TryGetProperty(field, out _), $"catalog wire is missing '{field}'");
        }
    }
}
