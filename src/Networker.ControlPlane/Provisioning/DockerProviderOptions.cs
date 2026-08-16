namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// Configuration for the <b>Docker (local)</b> compute provider — the
/// zero-cost twin of the Azure/AWS/GCP providers that materialises runners
/// (project testers) and endpoint targets as containers on the control-plane
/// host's Docker daemon, through the exact same product paths (create-tester →
/// provision → agent online; deployments → completed with endpoint_ips;
/// start/stop/delete; orphan reaper).
///
/// <para><b>Feature-flagged, default OFF.</b> <c>DASHBOARD_DOCKER_PROVIDER=1</c>
/// enables it; production never sets it, so the "Docker (local)" choice never
/// appears in the prod UI and a <c>cloud: "docker"</c> create is rejected with
/// 400. Every consumer must tolerate a missing registration (bare test hosts)
/// by treating it as <see cref="Disabled"/>.</para>
///
/// <para>Environment (all optional):</para>
/// <list type="bullet">
///   <item><c>DASHBOARD_DOCKER_BIN</c> — docker CLI (default <c>docker</c>).</item>
///   <item><c>DASHBOARD_DOCKER_NETWORK</c> — network to attach containers to.
///     Default: the network the control plane's own container is on (detected
///     lazily via <c>docker inspect $(hostname)</c>), else <c>nwk-lab_labnet</c>
///     if it exists, else <c>bridge</c>.</item>
///   <item><c>DASHBOARD_DOCKER_RUNNER_IMAGE</c> — runner image (default
///     <c>nwk-lab/runner:local</c>).</item>
///   <item><c>DASHBOARD_DOCKER_TARGET_IMAGE_PREFIX</c> — target image =
///     prefix + stack + <c>:</c> + <c>DASHBOARD_DOCKER_TARGET_IMAGE_TAG</c>
///     (<c>rust</c> for the bare endpoint); defaults <c>nwk-lab/target-</c> /
///     <c>local</c> → <c>nwk-lab/target-nginx:local</c>.</item>
///   <item><c>DASHBOARD_DOCKER_AGENT_URL</c> — WS url baked into runner
///     containers; default derived from <c>DASHBOARD_PUBLIC_URL</c> via
///     <see cref="CloudInitScripts.AgentWsUrl"/>. When the control plane runs on
///     the host (Docker Desktop) use <c>ws://host.docker.internal:5030/ws/agent</c>
///     — the provisioner adds <c>--add-host host.docker.internal:host-gateway</c>
///     on Linux whenever the url points there.</item>
/// </list>
/// </summary>
public sealed class DockerProviderOptions
{
    public const string EnableVar = "DASHBOARD_DOCKER_PROVIDER";
    public const string BinVar = "DASHBOARD_DOCKER_BIN";
    public const string NetworkVar = "DASHBOARD_DOCKER_NETWORK";
    public const string RunnerImageVar = "DASHBOARD_DOCKER_RUNNER_IMAGE";
    public const string TargetImagePrefixVar = "DASHBOARD_DOCKER_TARGET_IMAGE_PREFIX";
    public const string AgentUrlVar = "DASHBOARD_DOCKER_AGENT_URL";
    public const string TargetImageTagVar = "DASHBOARD_DOCKER_TARGET_IMAGE_TAG";

    /// <summary>The cloud identifier used on tester rows / deploy configs.</summary>
    public const string CloudName = "docker";

    /// <summary>The single region the provider reports (there is only one host).</summary>
    public const string Region = "local";

    /// <summary>The fixed default OS runner/target images are built on.</summary>
    public const string RequestedOs = "ubuntu-24.04";

    /// <summary>The fixed vm_size string a docker tester carries (cosmetic).</summary>
    public const string VmSize = "container";

    public const string DefaultRunnerImage = "nwk-lab/runner:local";
    public const string DefaultTargetImagePrefix = "nwk-lab/target-";
    public const string DefaultTargetImageTag = "local";
    public const string LabNetwork = "nwk-lab_labnet";

    public bool Enabled { get; init; }
    public string DockerBin { get; init; } = "docker";

    /// <summary>Explicitly configured network, or null when it must be detected.</summary>
    public string? Network { get; init; }

    public string RunnerImage { get; init; } = DefaultRunnerImage;
    public string TargetImagePrefix { get; init; } = DefaultTargetImagePrefix;

    /// <summary>Tag appended to <c>prefix + stack</c> (default <c>local</c>, the
    /// tag <c>lab/lab.sh build</c> uses); empty → no tag (docker's <c>latest</c>).</summary>
    public string TargetImageTag { get; init; } = DefaultTargetImageTag;

    /// <summary>Explicitly configured agent WS url, or null → derived from the
    /// public url at create time.</summary>
    public string? AgentUrl { get; init; }

    /// <summary>The always-off instance used by consumers when nothing was registered.</summary>
    public static DockerProviderOptions Disabled { get; } = new();

    /// <summary>Read the options from the process environment.</summary>
    public static DockerProviderOptions FromEnvironment() =>
        FromEnvironment(Environment.GetEnvironmentVariable);

    /// <summary>Testable core of <see cref="FromEnvironment()"/>.</summary>
    public static DockerProviderOptions FromEnvironment(Func<string, string?> getEnv)
    {
        static string? NonEmpty(string? v) => string.IsNullOrWhiteSpace(v) ? null : v.Trim();
        return new DockerProviderOptions
        {
            Enabled = IsTruthy(getEnv(EnableVar)),
            DockerBin = NonEmpty(getEnv(BinVar)) ?? "docker",
            Network = NonEmpty(getEnv(NetworkVar)),
            RunnerImage = NonEmpty(getEnv(RunnerImageVar)) ?? DefaultRunnerImage,
            TargetImagePrefix = NonEmpty(getEnv(TargetImagePrefixVar)) ?? DefaultTargetImagePrefix,
            TargetImageTag = getEnv(TargetImageTagVar) is { } tag ? tag.Trim() : DefaultTargetImageTag,
            AgentUrl = NonEmpty(getEnv(AgentUrlVar)),
        };
    }

    /// <summary>"1" / "true" / "yes" / "on" (case-insensitive) enable the flag.</summary>
    public static bool IsTruthy(string? value) =>
        value is not null
        && value.Trim().ToLowerInvariant() is "1" or "true" or "yes" or "on";

    /// <summary>Is <paramref name="cloud"/> the docker provider?</summary>
    public static bool IsDocker(string? cloud) =>
        string.Equals(cloud, CloudName, StringComparison.OrdinalIgnoreCase);

    /// <summary>Image for a target container: prefix + stack (<c>rust</c> for the
    /// bare endpoint / no stack).</summary>
    public string TargetImageFor(string? stack) =>
        TargetImagePrefix
        + (string.IsNullOrWhiteSpace(stack) || stack is "none" or "rust" ? "rust" : stack.Trim().ToLowerInvariant())
        + (TargetImageTag.Length == 0 ? string.Empty : ":" + TargetImageTag);

    /// <summary>The agent WS url runner containers connect to: the explicit
    /// override, else the public url mapped through <see cref="CloudInitScripts.AgentWsUrl"/>.</summary>
    public string ResolveAgentUrl(string publicUrl) =>
        AgentUrl ?? CloudInitScripts.AgentWsUrl(publicUrl);
}
