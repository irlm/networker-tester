using Networker.ControlPlane.Provisioning;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// The docker-provider reading of an SDK-sample deploy config — the path the
/// lab exercises for free, and therefore the one the create/reuse/update flow
/// is actually proven end to end on.
/// </summary>
public class SdkSampleDockerPlanTests
{
    [Fact]
    public void A_docker_endpoint_with_one_sample_becomes_a_sample_container()
    {
        var plan = DockerDeployPlan.TryParse("""
            {"endpoints":[{"provider":"docker","label":"sdk-go","sdk_samples":["go"]}]}
            """, out var error);
        Assert.Null(error);
        var ep = Assert.Single(plan!.Endpoints);
        Assert.Equal("go", ep.Sample);
        Assert.Null(ep.Stack);
        Assert.Equal("sdk-go", ep.Label);
    }

    [Fact]
    public void Sample_ids_are_normalised_to_lower_case()
    {
        var plan = DockerDeployPlan.TryParse("""
            {"endpoints":[{"provider":"docker","sdk_samples":["GO"]}]}
            """, out var error);
        Assert.Null(error);
        Assert.Equal("go", plan!.Endpoints[0].Sample);
    }

    [Fact]
    public void Several_samples_on_one_docker_endpoint_are_rejected_with_the_fix()
    {
        var plan = DockerDeployPlan.TryParse("""
            {"endpoints":[{"provider":"docker","sdk_samples":["go","rust"]}]}
            """, out var error);
        Assert.Null(plan);
        Assert.Contains("one endpoint per sample", error, StringComparison.Ordinal);
    }

    [Fact]
    public void A_sample_and_a_proxy_stack_on_one_container_are_rejected()
    {
        var plan = DockerDeployPlan.TryParse("""
            {"endpoints":[{"provider":"docker","sdk_samples":["go"],"http_stacks":["nginx"]}]}
            """, out var error);
        Assert.Null(plan);
        Assert.Contains("not both", error, StringComparison.Ordinal);
    }

    [Fact]
    public void A_plain_target_endpoint_still_has_no_sample()
    {
        var plan = DockerDeployPlan.TryParse("""
            {"endpoints":[{"provider":"docker","http_stacks":["nginx"]}]}
            """, out var error);
        Assert.Null(error);
        Assert.Null(plan!.Endpoints[0].Sample);
        Assert.Equal("nginx", plan.Endpoints[0].Stack);
    }

    [Fact]
    public void Mixed_sample_and_target_docker_endpoints_are_one_deployment()
    {
        // Different containers, same deployment — the consolidated docker shape.
        var plan = DockerDeployPlan.TryParse("""
            {"endpoints":[
              {"provider":"docker","label":"sdk-go","sdk_samples":["go"]},
              {"provider":"docker","label":"sdk-rust","sdk_samples":["rust"]}
            ]}
            """, out var error);
        Assert.Null(error);
        Assert.Equal(["go", "rust"], plan!.Endpoints.Select(e => e.Sample));
    }

    [Fact]
    public void Sample_images_follow_the_configured_prefix_and_tag()
    {
        var options = DockerProviderOptions.FromEnvironment(_ => null);
        Assert.Equal("nwk-lab/sdk-go:local", options.SampleImageFor("go"));
        // Distinct from the SDK *target* image (nwk-lab/sdk:local) — different
        // repo name, so the two can coexist in one daemon.
        Assert.NotEqual("nwk-lab/sdk:local", options.SampleImageFor("go"));

        var custom = DockerProviderOptions.FromEnvironment(k => k switch
        {
            DockerProviderOptions.SampleImagePrefixVar => "ghcr.io/acme/laghound-",
            DockerProviderOptions.TargetImageTagVar => "v1",
            _ => null,
        });
        Assert.Equal("ghcr.io/acme/laghound-rust:v1", custom.SampleImageFor("RUST"));
    }

    [Fact]
    public void Sample_run_args_set_the_port_and_token_and_redact_the_token_from_logs()
    {
        var args = DockerComputeProvisioner.BuildSampleRunArgs(
            "nwk-p1-sdk-go-abcde", "labnet",
            new Dictionary<string, string> { [DockerComputeProvisioner.SampleLabel] = "go" },
            "nwk-lab/sdk-go:local", 8105, "the-secret-token");

        Assert.Contains("PORT=8105", args);
        Assert.Contains("LAGHOUND_TOKEN=the-secret-token", args);
        Assert.Contains("unless-stopped", args);
        // The image is always last so docker parses the flags before it.
        Assert.Equal("nwk-lab/sdk-go:local", args[^1]);

        var rendered = DockerComputeProvisioner.RedactSensitive(args);
        Assert.DoesNotContain("the-secret-token", rendered, StringComparison.Ordinal);
        Assert.Contains("LAGHOUND_TOKEN=***", rendered, StringComparison.Ordinal);
    }
}
