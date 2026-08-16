using Microsoft.Extensions.Logging.Abstractions;
using Networker.ControlPlane.Background;
using Networker.ControlPlane.Endpoints;
using Networker.ControlPlane.Provisioning;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// The Docker (local) provider: pure argv/naming builders behind
/// <see cref="DockerComputeProvisioner"/> (like <see cref="CliProvisionerCreateArgsTests"/>
/// for az/aws/gcloud), the feature-flag gating, the create-tester validation
/// that accepts cloud "docker" without an account, the deploy-plan parser and
/// the reaper's orphan decision. No docker daemon is touched.
/// </summary>
public sealed class DockerProviderTests
{
    private static DockerProviderOptions Enabled(Func<DockerProviderOptions, DockerProviderOptions>? mutate = null)
    {
        var o = DockerProviderOptions.FromEnvironment(k => k == DockerProviderOptions.EnableVar ? "1" : null);
        return mutate is null ? o : mutate(o);
    }

    // ── flag / options ───────────────────────────────────────────────────────

    [Fact]
    public void Options_default_off_with_documented_defaults()
    {
        var o = DockerProviderOptions.FromEnvironment(_ => null);
        Assert.False(o.Enabled);
        Assert.Equal("docker", o.DockerBin);
        Assert.Null(o.Network);
        Assert.Equal("nwk-lab/runner:local", o.RunnerImage);
        Assert.Equal("nwk-lab/target-", o.TargetImagePrefix);
        Assert.Equal("local", o.TargetImageTag);
        Assert.Equal("nwk-lab/target-caddy:local", o.TargetImageFor("caddy"));
        Assert.Null(o.AgentUrl);
        var untagged = DockerProviderOptions.FromEnvironment(k => k == DockerProviderOptions.TargetImageTagVar ? "" : null);
        Assert.Equal("nwk-lab/target-nginx", untagged.TargetImageFor("nginx"));
        Assert.False(DockerProviderOptions.Disabled.Enabled);
    }

    [Theory]
    [InlineData("1", true)]
    [InlineData("true", true)]
    [InlineData("YES", true)]
    [InlineData("on", true)]
    [InlineData("0", false)]
    [InlineData("false", false)]
    [InlineData("", false)]
    [InlineData(null, false)]
    public void Options_flag_parsing(string? value, bool expected)
    {
        var o = DockerProviderOptions.FromEnvironment(k => k == DockerProviderOptions.EnableVar ? value : null);
        Assert.Equal(expected, o.Enabled);
    }

    [Fact]
    public void Options_read_every_env_var()
    {
        var env = new Dictionary<string, string>
        {
            [DockerProviderOptions.EnableVar] = "1",
            [DockerProviderOptions.BinVar] = "/usr/local/bin/docker",
            [DockerProviderOptions.NetworkVar] = "nwk-lab_labnet",
            [DockerProviderOptions.RunnerImageVar] = "me/runner:dev",
            [DockerProviderOptions.TargetImagePrefixVar] = "me/target-",
            [DockerProviderOptions.AgentUrlVar] = "ws://host.docker.internal:5030/ws/agent",
        };
        var o = DockerProviderOptions.FromEnvironment(k => env.GetValueOrDefault(k));
        Assert.True(o.Enabled);
        Assert.Equal("/usr/local/bin/docker", o.DockerBin);
        Assert.Equal("nwk-lab_labnet", o.Network);
        Assert.Equal("me/runner:dev", o.RunnerImage);
        Assert.Equal("me/target-nginx:local", o.TargetImageFor("nginx"));
        Assert.Equal("me/target-rust:local", o.TargetImageFor(null));
        Assert.Equal("me/target-rust:local", o.TargetImageFor("none"));
        Assert.Equal("ws://host.docker.internal:5030/ws/agent", o.ResolveAgentUrl("https://laghound.com"));
    }

    [Fact]
    public void Agent_url_derives_from_public_url_when_not_overridden()
    {
        var o = Enabled();
        Assert.Equal("ws://controlplane:5030/ws/agent", o.ResolveAgentUrl("http://controlplane:5030"));
        Assert.Equal("wss://laghound.com/ws/agent", o.ResolveAgentUrl("https://laghound.com/"));
    }

    // ── naming / argv ────────────────────────────────────────────────────────

    [Fact]
    public void Container_name_is_nwk_project_vmname()
    {
        Assert.Equal("nwk-3f2a9c1e-tester-local-ab12c",
            DockerComputeProvisioner.ContainerName("3f2a9c1e-77aa-4b0e-9d1d-000000000000", "tester-local-ab12c"));
        Assert.Equal("nwk-locallab-tester-local-ab12c",
            DockerComputeProvisioner.ContainerName("Local Lab!", "tester-local-ab12c"));
        Assert.Equal("nwk-tester-local-ab12c", DockerComputeProvisioner.ContainerName(null, "tester-local-ab12c"));
        Assert.Equal("nwk-p-ep-nginx-my-target-x", DockerComputeProvisioner.ContainerName("p", "ep-nginx-my target/x"));
    }

    [Fact]
    public void Runner_run_args_are_complete_and_ordered()
    {
        var labels = new Dictionary<string, string>
        {
            [DockerComputeProvisioner.TesterIdLabel] = "t-1",
            [DockerComputeProvisioner.RoleLabel] = "runner",
            [DockerComputeProvisioner.ProjectLabel] = "p-1",
        };
        var env = new Dictionary<string, string>
        {
            ["AGENT_NAME"] = "tester-local-ab12c",
            ["AGENT_API_KEY"] = "K3Y",
            ["AGENT_DASHBOARD_URL"] = "ws://controlplane:5030/ws/agent",
        };
        var args = DockerComputeProvisioner.BuildRunnerRunArgs(
            "nwk-p1-tester-local-ab12c", "nwk-lab_labnet", labels, env, "nwk-lab/runner:local", addHostGateway: false);

        Assert.Equal(new[]
        {
            "run", "-d",
            "--name", "nwk-p1-tester-local-ab12c",
            "--hostname", "nwk-p1-tester-local-ab12c",
            "--network", "nwk-lab_labnet",
            "--cap-add", "NET_ADMIN",
            "--cap-add", "NET_RAW",
            "--sysctl", "net.ipv4.ping_group_range=0 2147483647",
            "--label", "networker.project=p-1",
            "--label", "networker.role=runner",
            "--label", "networker.tester_id=t-1",
            "-e", "AGENT_API_KEY=K3Y",
            "-e", "AGENT_DASHBOARD_URL=ws://controlplane:5030/ws/agent",
            "-e", "AGENT_NAME=tester-local-ab12c",
            "nwk-lab/runner:local",
        }, args);
    }

    [Fact]
    public void Runner_run_args_add_host_gateway_when_requested()
    {
        var args = DockerComputeProvisioner.BuildRunnerRunArgs(
            "c", "bridge", new Dictionary<string, string>(), new Dictionary<string, string>(), "img", addHostGateway: true);
        var i = args.IndexOf("--add-host");
        Assert.True(i > 0);
        Assert.Equal("host.docker.internal:host-gateway", args[i + 1]);
        Assert.Equal("img", args[^1]);
    }

    [Fact]
    public void Target_run_args_carry_stack_env_and_image()
    {
        var labels = new Dictionary<string, string>
        {
            [DockerComputeProvisioner.RoleLabel] = "target",
            [DockerComputeProvisioner.DeploymentIdLabel] = "d-1",
        };
        var args = DockerComputeProvisioner.BuildTargetRunArgs("nwk-p1-ep-nginx-x", "nwk-lab_labnet", labels, "nwk-lab/target-nginx:local", "nginx");
        Assert.Equal(new[]
        {
            "run", "-d",
            "--name", "nwk-p1-ep-nginx-x",
            "--hostname", "nwk-p1-ep-nginx-x",
            "--network", "nwk-lab_labnet",
            "--label", "networker.deployment_id=d-1",
            "--label", "networker.role=target",
            "-e", "TARGET_STACK=nginx",
            "nwk-lab/target-nginx:local",
        }, args);

        var bare = DockerComputeProvisioner.BuildTargetRunArgs("c", "n", new Dictionary<string, string>(), "nwk-lab/target-rust:local", null);
        Assert.Contains("TARGET_STACK=none", bare);
        Assert.Equal("none", DockerComputeProvisioner.TargetStackEnv("rust"));
        Assert.Equal("caddy", DockerComputeProvisioner.TargetStackEnv(" Caddy "));
    }

    [Fact]
    public void Lifecycle_args_map_to_docker_verbs()
    {
        Assert.Equal(new[] { "start", "c" }, DockerComputeProvisioner.BuildLifecycleArgs(LifecycleOp.Start, "c"));
        Assert.Equal(new[] { "stop", "--time", "20", "c" }, DockerComputeProvisioner.BuildLifecycleArgs(LifecycleOp.Stop, "c"));
        Assert.Equal(new[] { "rm", "-f", "-v", "c" }, DockerComputeProvisioner.BuildLifecycleArgs(LifecycleOp.Delete, "c"));
        Assert.Equal(new[] { "inspect", "--type", "container", "c" }, DockerComputeProvisioner.BuildLifecycleArgs(LifecycleOp.Show, "c"));
        Assert.Equal(new[] { "exec", "c", "sh", "-c", "echo hi" }, DockerComputeProvisioner.BuildExecArgs("c", "echo hi"));
    }

    [Fact]
    public void Missing_container_signals_are_recognised()
    {
        Assert.True(DockerComputeProvisioner.LooksMissing("Error response from daemon: No such container: nwk-x"));
        Assert.True(DockerComputeProvisioner.LooksMissing("Error: No such object: nwk-x"));
        Assert.False(DockerComputeProvisioner.LooksMissing("permission denied while trying to connect to the Docker daemon socket"));
    }

    [Fact]
    public void Redaction_masks_only_the_api_key()
    {
        var s = DockerComputeProvisioner.RedactSensitive(new[] { "run", "-e", "AGENT_API_KEY=secret", "-e", "AGENT_NAME=x", "img" });
        Assert.Equal("run -e AGENT_API_KEY=*** -e AGENT_NAME=x img", s);
    }

    // ── inspect parsing → power state ────────────────────────────────────────

    private const string InspectRunning = """
        [{"Name":"/nwk-p1-tester-local-ab12c","Created":"2026-08-15T10:00:00.123456789Z",
          "State":{"Status":"running","Running":true,"Health":{"Status":"healthy"}},
          "Config":{"Labels":{"networker.role":"runner","networker.project":"p1","networker.tester_id":"t-1"}},
          "NetworkSettings":{"Networks":{"bridge":{"IPAddress":""},"nwk-lab_labnet":{"IPAddress":"172.31.100.7"}}}}]
        """;

    private const string InspectExited = """
        [{"Name":"/nwk-p1-ep-nginx-x","Created":"2026-08-15T10:00:00Z",
          "State":{"Status":"exited","Running":false},
          "Config":{"Labels":{"networker.role":"target","networker.deployment_id":"d-1"}},
          "NetworkSettings":{"Networks":{}}}]
        """;

    [Fact]
    public void Inspect_parses_state_ip_labels_and_created()
    {
        var c = Assert.Single(DockerComputeProvisioner.ParseInspect(InspectRunning, "nwk-lab_labnet"));
        Assert.Equal("nwk-p1-tester-local-ab12c", c.Name);
        Assert.True(c.Running);
        Assert.Equal("running", c.PowerState);
        Assert.Equal("healthy", c.Health);
        Assert.Equal("172.31.100.7", c.Ip);
        Assert.Equal("runner", c.Role);
        Assert.Equal("t-1", c.TesterId);
        Assert.Equal(new DateTime(2026, 8, 15, 10, 0, 0, DateTimeKind.Utc), c.CreatedUtc!.Value.Date.AddHours(10));

        var e = Assert.Single(DockerComputeProvisioner.ParseInspect(InspectExited, null));
        Assert.False(e.Running);
        Assert.Equal("stopped", e.PowerState);
        Assert.Null(e.Ip);
        Assert.Equal("target", e.Role);
        Assert.Equal("d-1", e.DeploymentId);
    }

    [Fact]
    public void Inspect_prefers_selected_network_else_first_with_ip()
    {
        var withoutPreference = Assert.Single(DockerComputeProvisioner.ParseInspect(InspectRunning, null));
        Assert.Equal("172.31.100.7", withoutPreference.Ip); // bridge has no ip → first WITH an ip
        var wrongPreference = Assert.Single(DockerComputeProvisioner.ParseInspect(InspectRunning, "other"));
        Assert.Equal("172.31.100.7", wrongPreference.Ip);
    }

    [Fact]
    public void Show_json_maps_onto_the_product_power_state_vocabulary()
    {
        var running = DockerComputeProvisioner.ParseInspect(InspectRunning, null)[0];
        var stopped = DockerComputeProvisioner.ParseInspect(InspectExited, null)[0];
        Assert.Equal("running", TesterWriteEndpoints.ParsePowerState("docker", DockerComputeProvisioner.NormalizedShowJson(running)));
        Assert.Equal("stopped", TesterWriteEndpoints.ParsePowerState("docker", DockerComputeProvisioner.NormalizedShowJson(stopped)));
    }

    // ── disabled provider is total, never crashes ────────────────────────────

    [Fact]
    public async Task Disabled_provider_soft_fails_every_operation()
    {
        var p = new DockerComputeProvisioner(DockerProviderOptions.Disabled, NullLogger<DockerComputeProvisioner>.Instance);
        var tester = new ProjectTester { Cloud = "docker", VmResourceId = "nwk-x", Region = "local" };

        var created = await p.CreateVmAsync(new VmCreateRequest("docker", "n", "local", "container", "ubuntu", "img", null), null);
        Assert.False(created.Success);
        Assert.Contains("disabled", created.Error);

        var start = await p.StartAsync(tester, null);
        Assert.False(start.Success);
        Assert.Null(start.ExitCode); // soft (missing-CLI class), so FinishAsync converges the row
        Assert.Contains(DockerProviderOptions.EnableVar, start.Error);

        var target = await p.CreateTargetAsync(new DockerComputeProvisioner.TargetContainerRequest("p", Guid.NewGuid(), "t", "nginx"));
        Assert.False(target.Success);

        var wrongCloud = await p.DeleteAsync(new ProjectTester { Cloud = "azure", VmResourceId = "/subscriptions/x" }, null);
        Assert.Contains("unsupported cloud provider", wrongCloud.Error);
    }

    [Fact]
    public async Task Resolve_by_endpoint_ignores_non_docker_clouds()
    {
        var p = new DockerComputeProvisioner(Enabled(), NullLogger<DockerComputeProvisioner>.Instance);
        Assert.Null(await p.ResolveByEndpointAsync("azure", null, "1.2.3.4"));
        Assert.Null(await p.ResolveByEndpointAsync("docker", null, ""));
    }

    // ── create-tester validation ─────────────────────────────────────────────

    [Fact]
    public void Create_validation_rejects_docker_when_flag_off()
    {
        var err = TesterCreateLogic.ValidateDockerCreate(dockerEnabled: false, null, null);
        Assert.NotNull(err);
        Assert.Contains("unsupported cloud provider: docker", err);
        Assert.Contains(DockerProviderOptions.EnableVar, err);
    }

    [Fact]
    public void Create_validation_accepts_docker_without_account_and_rejects_with_one()
    {
        Assert.Null(TesterCreateLogic.ValidateDockerCreate(dockerEnabled: true, null, null));
        Assert.Contains("cloud_account_id", TesterCreateLogic.ValidateDockerCreate(true, Guid.NewGuid(), null));
        Assert.Contains("cloud_connection_id", TesterCreateLogic.ValidateDockerCreate(true, null, Guid.NewGuid()));
    }

    [Fact]
    public void Docker_body_is_pinned_to_local_region_and_container_size()
    {
        var body = new TesterWriteEndpoints.CreateTesterBody
        {
            Name = "lab-runner",
            Cloud = "Docker",
            Region = "eastus",
            VmSize = "Standard_B2s",
            RequestedOs = "windows-2022",
            CloudAccountId = null,
        };
        var n = TesterCreateLogic.NormalizeDockerBody(body);
        Assert.Equal("lab-runner", n.Name);
        Assert.Equal("docker", n.Cloud);
        Assert.Equal("local", n.Region);
        Assert.Equal("container", n.VmSize);
        Assert.Equal("ubuntu-24.04", n.RequestedOs);
        Assert.Equal("server", n.RequestedVariant);
        Assert.Null(n.CloudAccountId);
    }

    // ── deploy plan ──────────────────────────────────────────────────────────

    [Fact]
    public void Deploy_plan_is_null_for_cloud_configs()
    {
        var plan = DockerDeployPlan.TryParse(
            """{"endpoints":[{"provider":"azure","http_stacks":["nginx"],"azure":{"region":"eastus"}}]}""", out var err);
        Assert.Null(plan);
        Assert.Null(err);
        Assert.Null(DockerDeployPlan.TryParse("not json", out _));
        Assert.Null(DockerDeployPlan.TryParse("{}", out _));
    }

    [Fact]
    public void Deploy_plan_one_container_per_docker_endpoint()
    {
        var plan = DockerDeployPlan.TryParse(
            """{"endpoints":[{"provider":"docker","label":"web","http_stacks":["nginx"]},{"provider":"docker","http_stacks":[]}]}""",
            out var err);
        Assert.Null(err);
        Assert.NotNull(plan);
        Assert.Equal(2, plan!.Endpoints.Count);
        Assert.Equal(("web", "nginx"), (plan.Endpoints[0].Label, plan.Endpoints[0].Stack));
        Assert.Equal(("target-2", (string?)null), (plan.Endpoints[1].Label, plan.Endpoints[1].Stack));
    }

    [Fact]
    public void Deploy_plan_rejects_mixed_multi_stack_and_languages()
    {
        Assert.Null(DockerDeployPlan.TryParse(
            """{"endpoints":[{"provider":"docker","http_stacks":["nginx"]},{"provider":"lan","lan":{"ip":"10.0.0.1"}}]}""", out var mixed));
        Assert.Contains("mix", mixed);

        Assert.Null(DockerDeployPlan.TryParse(
            """{"endpoints":[{"provider":"docker","http_stacks":["nginx","caddy"]}]}""", out var multi));
        Assert.Contains("exactly one http_stack", multi);

        Assert.Null(DockerDeployPlan.TryParse(
            """{"endpoints":[{"provider":"docker","http_stacks":["nginx"],"languages":["rust"]}]}""", out var langs));
        Assert.Contains("languages", langs);
    }

    // ── pending endpoint (orchestrator) ──────────────────────────────────────

    [Fact]
    public void Pending_docker_ref_parses_without_account_and_builds_docker_deploy_json()
    {
        var pending = ProvisioningOrchestrator.ParsePending(
            """{"kind":"pending","provider":"docker","region":"local","vm_size":"container","os":"linux","proxy_stack":"nginx"}""");
        Assert.NotNull(pending);
        Assert.Null(pending!.CloudAccountId);
        Assert.Equal("docker", pending.Provider);

        var json = ProvisioningOrchestrator.BuildDeployJson(pending, "docker", "cfg", Guid.NewGuid());
        Assert.Null(json["cloud_account_id"]);
        var ep = json["endpoints"]![0]!;
        Assert.Equal("docker", ep["provider"]!.GetValue<string>());
        Assert.Equal("nginx", ep["http_stacks"]![0]!.GetValue<string>());
        Assert.Equal("linux", ep["docker"]!["os"]!.GetValue<string>());
        Assert.NotNull(DockerDeployPlan.TryParse(json.ToJsonString(), out var err));
        Assert.Null(err);
    }

    [Fact]
    public void Pending_cloud_ref_still_requires_account()
    {
        Assert.Null(ProvisioningOrchestrator.ParsePending("""{"kind":"pending","region":"eastus","proxy_stack":"nginx"}"""));
        var ok = ProvisioningOrchestrator.ParsePending(
            $$"""{"kind":"pending","cloud_account_id":"{{Guid.NewGuid()}}","region":"eastus","proxy_stack":"nginx"}""");
        Assert.NotNull(ok);
        Assert.NotNull(ok!.CloudAccountId);
        Assert.Null(ok.Provider);
    }

    // ── orphan reaper decision ───────────────────────────────────────────────

    private static DockerComputeProvisioner.ContainerInfo Container(string role, string? tester, string? dep, TimeSpan age, string name = "nwk-x")
    {
        var labels = new Dictionary<string, string> { [DockerComputeProvisioner.RoleLabel] = role };
        if (tester is not null) labels[DockerComputeProvisioner.TesterIdLabel] = tester;
        if (dep is not null) labels[DockerComputeProvisioner.DeploymentIdLabel] = dep;
        return new DockerComputeProvisioner.ContainerInfo(name, "running", true, "172.31.100.9", labels, DateTime.UtcNow - age, null);
    }

    [Fact]
    public void Reaper_keeps_owned_and_young_containers_and_reaps_the_rest()
    {
        var now = DateTime.UtcNow;
        var testers = new HashSet<string> { "t-live" };
        var resources = new HashSet<string> { "nwk-known" };
        var deps = new HashSet<string> { "d-live" };
        var old = TimeSpan.FromHours(1);
        var young = TimeSpan.FromMinutes(1);

        Assert.False(OrphanReaperService.IsDockerOrphan(Container("runner", "t-live", null, old), testers, resources, deps, now));
        Assert.False(OrphanReaperService.IsDockerOrphan(Container("runner", "t-gone", null, old, "nwk-known"), testers, resources, deps, now));
        Assert.True(OrphanReaperService.IsDockerOrphan(Container("runner", "t-gone", null, old), testers, resources, deps, now));
        Assert.True(OrphanReaperService.IsDockerOrphan(Container("runner", null, null, old), testers, resources, deps, now));
        Assert.False(OrphanReaperService.IsDockerOrphan(Container("runner", "t-gone", null, young), testers, resources, deps, now));

        Assert.False(OrphanReaperService.IsDockerOrphan(Container("target", null, "d-live", old), testers, resources, deps, now));
        Assert.True(OrphanReaperService.IsDockerOrphan(Container("target", null, "d-gone", old), testers, resources, deps, now));
        Assert.True(OrphanReaperService.IsDockerOrphan(Container("target", null, null, old), testers, resources, deps, now));

        Assert.False(OrphanReaperService.IsDockerOrphan(Container("something-else", null, null, old), testers, resources, deps, now));
    }
}
