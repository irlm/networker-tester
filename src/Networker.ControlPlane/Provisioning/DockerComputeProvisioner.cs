using System.Diagnostics;
using System.Globalization;
using System.Text.Json;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// <see cref="IComputeProvisioner"/> for the <b>Docker (local)</b> provider —
/// shells out to the docker CLI exactly the way <see cref="CliComputeProvisioner"/>
/// drives <c>az</c>/<c>aws</c>/<c>gcloud</c>, so a runner (project tester) or an
/// endpoint target becomes a container on the control-plane host with
/// <b>no cloud account and no cost</b>, through the same product paths:
///
/// <list type="bullet">
///   <item><see cref="CreateVmAsync"/> → <c>docker run -d</c> of the runner image
///     with the minted agent key in the environment (the container analogue of
///     the cloud-init bootstrap); the public ip is the container's ip on the
///     configured network.</item>
///   <item><see cref="StartAsync"/> → <c>docker start</c>;
///     <see cref="StopAsync"/> / <see cref="DeallocateAsync"/> → <c>docker stop</c>;
///     <see cref="DeleteAsync"/> → <c>docker rm -f</c> (idempotent: a missing
///     container is success); <see cref="ShowAsync"/> → <c>docker inspect</c>
///     normalised to <c>{"powerState": running|stopped, …}</c>;
///     <see cref="RunCommandAsync"/> → <c>docker exec sh -c</c>.</item>
///   <item><see cref="CreateTargetAsync"/> → an endpoint target container
///     (<c>nwk-lab/target-&lt;stack&gt;</c>, <c>TARGET_STACK</c> env) for the deploy
///     runner; <see cref="WaitHealthyAsync"/> gates on the image healthcheck /
///     <c>/health</c>.</item>
///   <item><see cref="ListManagedContainersAsync"/> — everything carrying a
///     <c>networker.role</c> label, for the orphan reaper and
///     <see cref="ResolveByEndpointAsync"/>.</item>
/// </list>
///
/// <para>Container naming: <c>nwk-&lt;projectShortId&gt;-&lt;vmName&gt;</c>, labelled
/// <c>networker.role=runner|target</c>, <c>networker.project</c>,
/// <c>networker.tester_id</c> / <c>networker.deployment_id</c>.
/// <c>VmResourceId</c> = the container name.</para>
///
/// <para>Total like every provisioner: a missing docker binary, a non-zero exit,
/// or a spawn error is captured in the result — never thrown.</para>
/// </summary>
public sealed class DockerComputeProvisioner(
    DockerProviderOptions options,
    ILogger<DockerComputeProvisioner> logger) : IComputeProvisioner
{
    public const string RoleLabel = "networker.role";
    public const string ProjectLabel = "networker.project";
    public const string TesterIdLabel = "networker.tester_id";
    public const string DeploymentIdLabel = "networker.deployment_id";
    public const string StackLabel = "networker.stack";

    /// <summary>Which LagHound SDK sample a target container serves, when it is
    /// a sample container rather than an endpoint/proxy target.</summary>
    public const string SampleLabel = "networker.sdk_sample";
    public const string RoleRunner = "runner";
    public const string RoleTarget = "target";

    private static readonly TimeSpan CommandTimeout = TimeSpan.FromMinutes(3);
    private static readonly TimeSpan HealthyTimeout = TimeSpan.FromSeconds(150);

    private readonly SemaphoreSlim _networkGate = new(1, 1);
    private string? _resolvedNetwork;

    public DockerProviderOptions Options => options;

    // ── naming / argv builders (pure; unit-tested) ───────────────────────────

    /// <summary>
    /// <c>nwk-&lt;projectShortId&gt;-&lt;vmName&gt;</c>. Docker names must match
    /// <c>[a-zA-Z0-9][a-zA-Z0-9_.-]*</c>; the vm name is already DNS-safe, the
    /// project id is reduced to its first 8 safe chars (a uuid prefix or a
    /// slug head is plenty to eyeball ownership in <c>docker ps</c>).
    /// </summary>
    public static string ContainerName(string? projectId, string vmName)
    {
        var shortId = new string((projectId ?? string.Empty)
            .Where(c => char.IsAsciiLetterOrDigit(c))
            .Take(8)
            .ToArray())
            .ToLowerInvariant();
        var safeVm = SanitizeNamePart(vmName);
        return shortId.Length == 0 ? $"nwk-{safeVm}" : $"nwk-{shortId}-{safeVm}";
    }

    internal static string SanitizeNamePart(string s)
    {
        var chars = s.Select(c => char.IsAsciiLetterOrDigit(c) || c is '-' or '_' or '.' ? c : '-').ToArray();
        var v = new string(chars).Trim('-', '.', '_');
        return v.Length == 0 ? "x" : v.ToLowerInvariant();
    }

    /// <summary>
    /// <c>docker run</c> argv for a runner container: detached, named,
    /// on <paramref name="network"/>, labelled, with the ping sysctl + NET_ADMIN /
    /// NET_RAW the lab grants runners (tshark, tc netem, ICMP), and the agent
    /// environment. <paramref name="addHostGateway"/> appends
    /// <c>--add-host host.docker.internal:host-gateway</c> (Linux, control plane
    /// on the host).
    /// </summary>
    public static List<string> BuildRunnerRunArgs(
        string containerName,
        string network,
        IReadOnlyDictionary<string, string> labels,
        IReadOnlyDictionary<string, string> environment,
        string image,
        bool addHostGateway)
    {
        var args = new List<string>
        {
            "run", "-d",
            "--name", containerName,
            "--hostname", containerName,
            "--network", network,
            "--cap-add", "NET_ADMIN",
            "--cap-add", "NET_RAW",
            "--sysctl", "net.ipv4.ping_group_range=0 2147483647",
        };
        if (addHostGateway)
        {
            args.Add("--add-host");
            args.Add("host.docker.internal:host-gateway");
        }
        foreach (var (k, v) in labels.OrderBy(kv => kv.Key, StringComparer.Ordinal))
        {
            args.Add("--label");
            args.Add($"{k}={v}");
        }
        foreach (var (k, v) in environment.OrderBy(kv => kv.Key, StringComparer.Ordinal))
        {
            args.Add("-e");
            args.Add($"{k}={v}");
        }
        args.Add(image);
        return args;
    }

    /// <summary>
    /// <c>docker run</c> argv for an endpoint target container: the stack image
    /// with <c>TARGET_STACK</c> (see <c>lab/images/target/entrypoint.sh</c>);
    /// <c>none</c> for the bare rust endpoint.
    /// </summary>
    public static List<string> BuildTargetRunArgs(
        string containerName,
        string network,
        IReadOnlyDictionary<string, string> labels,
        string image,
        string? stack)
    {
        var args = new List<string>
        {
            "run", "-d",
            "--name", containerName,
            "--hostname", containerName,
            "--network", network,
        };
        foreach (var (k, v) in labels.OrderBy(kv => kv.Key, StringComparer.Ordinal))
        {
            args.Add("--label");
            args.Add($"{k}={v}");
        }
        args.Add("-e");
        args.Add($"TARGET_STACK={TargetStackEnv(stack)}");
        args.Add(image);
        return args;
    }

    /// <summary>
    /// <c>docker run</c> argv for a LagHound SDK sample container: the sample
    /// image with <c>PORT</c> (every sample honours it — see
    /// <c>examples/*.Dockerfile</c>) and <c>LAGHOUND_TOKEN</c>. No published
    /// host port: the control plane and the runners reach the container by its
    /// ip/name on the shared network, exactly like a target container.
    /// </summary>
    public static List<string> BuildSampleRunArgs(
        string containerName,
        string network,
        IReadOnlyDictionary<string, string> labels,
        string image,
        int port,
        string token)
    {
        var args = new List<string>
        {
            "run", "-d",
            "--name", containerName,
            "--hostname", containerName,
            "--network", network,
            "--restart", "unless-stopped",
        };
        foreach (var (k, v) in labels.OrderBy(kv => kv.Key, StringComparer.Ordinal))
        {
            args.Add("--label");
            args.Add($"{k}={v}");
        }
        args.Add("-e");
        args.Add($"PORT={port.ToString(CultureInfo.InvariantCulture)}");
        args.Add("-e");
        args.Add($"LAGHOUND_TOKEN={token}");
        args.Add(image);
        return args;
    }

    /// <summary>The <c>TARGET_STACK</c> value: <c>none</c> for the bare endpoint.</summary>
    public static string TargetStackEnv(string? stack) =>
        string.IsNullOrWhiteSpace(stack) || stack is "none" or "rust" ? "none" : stack.Trim().ToLowerInvariant();

    /// <summary>Lifecycle argv: start / stop / rm -f / inspect.</summary>
    public static List<string> BuildLifecycleArgs(LifecycleOp op, string containerName) => op switch
    {
        LifecycleOp.Start => ["start", containerName],
        LifecycleOp.Stop => ["stop", "--time", "20", containerName],
        LifecycleOp.Delete => ["rm", "-f", "-v", containerName],
        LifecycleOp.Show => ["inspect", "--type", "container", containerName],
        _ => throw new ArgumentOutOfRangeException(nameof(op)),
    };

    /// <summary><c>docker exec &lt;c&gt; sh -c &lt;script&gt;</c>.</summary>
    public static List<string> BuildExecArgs(string containerName, string script) =>
        ["exec", containerName, "sh", "-c", script];

    /// <summary>"No such container" / "No such object" — the docker not-found signals.</summary>
    public static bool LooksMissing(string stderr) =>
        stderr.Contains("No such container", StringComparison.OrdinalIgnoreCase)
        || stderr.Contains("No such object", StringComparison.OrdinalIgnoreCase)
        || stderr.Contains("no such container", StringComparison.OrdinalIgnoreCase);

    /// <summary>
    /// A container as seen through <c>docker inspect</c>: the bits the product
    /// cares about (state, ip on the first/selected network, labels, created).
    /// </summary>
    public sealed record ContainerInfo(
        string Name,
        string Status,
        bool Running,
        string? Ip,
        IReadOnlyDictionary<string, string> Labels,
        DateTime? CreatedUtc,
        string? Health)
    {
        public string? Role => Labels.GetValueOrDefault(RoleLabel);
        public string? ProjectId => Labels.GetValueOrDefault(ProjectLabel);
        public string? TesterId => Labels.GetValueOrDefault(TesterIdLabel);
        public string? DeploymentId => Labels.GetValueOrDefault(DeploymentIdLabel);

        /// <summary>The coarse product power state ("running" | "stopped").</summary>
        public string PowerState => Running ? "running" : "stopped";
    }

    /// <summary>
    /// Parse the <c>docker inspect</c> JSON array. <paramref name="preferredNetwork"/>
    /// selects the ip when the container sits on several networks (else the first
    /// with an address wins).
    /// </summary>
    public static List<ContainerInfo> ParseInspect(string json, string? preferredNetwork)
    {
        var list = new List<ContainerInfo>();
        if (string.IsNullOrWhiteSpace(json))
        {
            return list;
        }
        using var doc = JsonDocument.Parse(json);
        if (doc.RootElement.ValueKind != JsonValueKind.Array)
        {
            return list;
        }
        foreach (var c in doc.RootElement.EnumerateArray())
        {
            var name = c.TryGetProperty("Name", out var n) ? (n.GetString() ?? string.Empty).TrimStart('/') : string.Empty;
            var status = "unknown";
            var running = false;
            string? health = null;
            if (c.TryGetProperty("State", out var st) && st.ValueKind == JsonValueKind.Object)
            {
                status = st.TryGetProperty("Status", out var s) ? s.GetString() ?? "unknown" : "unknown";
                running = st.TryGetProperty("Running", out var r) && r.ValueKind == JsonValueKind.True;
                if (st.TryGetProperty("Health", out var h) && h.ValueKind == JsonValueKind.Object
                    && h.TryGetProperty("Status", out var hs))
                {
                    health = hs.GetString();
                }
            }
            string? ip = null;
            if (c.TryGetProperty("NetworkSettings", out var ns) && ns.ValueKind == JsonValueKind.Object
                && ns.TryGetProperty("Networks", out var nets) && nets.ValueKind == JsonValueKind.Object)
            {
                string? first = null;
                foreach (var net in nets.EnumerateObject())
                {
                    var addr = net.Value.TryGetProperty("IPAddress", out var a) ? a.GetString() : null;
                    if (string.IsNullOrEmpty(addr))
                    {
                        continue;
                    }
                    first ??= addr;
                    if (preferredNetwork is not null
                        && string.Equals(net.Name, preferredNetwork, StringComparison.Ordinal))
                    {
                        ip = addr;
                        break;
                    }
                }
                ip ??= first;
            }
            var labels = new Dictionary<string, string>(StringComparer.Ordinal);
            if (c.TryGetProperty("Config", out var cfg) && cfg.ValueKind == JsonValueKind.Object
                && cfg.TryGetProperty("Labels", out var lb) && lb.ValueKind == JsonValueKind.Object)
            {
                foreach (var l in lb.EnumerateObject())
                {
                    labels[l.Name] = l.Value.GetString() ?? string.Empty;
                }
            }
            DateTime? created = null;
            if (c.TryGetProperty("Created", out var cr) && cr.ValueKind == JsonValueKind.String
                && DateTime.TryParse(cr.GetString(), null,
                    System.Globalization.DateTimeStyles.AdjustToUniversal | System.Globalization.DateTimeStyles.AssumeUniversal,
                    out var dt))
            {
                created = dt;
            }
            list.Add(new ContainerInfo(name, status, running, ip, labels, created, health));
        }
        return list;
    }

    /// <summary>The normalised <c>show</c> payload the lifecycle handlers parse
    /// (<c>ParsePowerState("docker", …)</c> reads <c>powerState</c>).</summary>
    public static string NormalizedShowJson(ContainerInfo c) =>
        JsonSerializer.Serialize(new
        {
            powerState = c.PowerState,
            status = c.Status,
            health = c.Health,
            ip = c.Ip,
            name = c.Name,
        });

    // ── IComputeProvisioner ──────────────────────────────────────────────────

    public Task<ProvisionResult> StartAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default) =>
        LifecycleAsync(tester, LifecycleOp.Start, ct);

    public Task<ProvisionResult> StopAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default) =>
        LifecycleAsync(tester, LifecycleOp.Stop, ct);

    public Task<ProvisionResult> DeallocateAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default) =>
        LifecycleAsync(tester, LifecycleOp.Stop, ct);

    public Task<ProvisionResult> DeleteAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default) =>
        LifecycleAsync(tester, LifecycleOp.Delete, ct);

    public async Task<ProvisionResult> ShowAsync(ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct = default)
    {
        var res = await LifecycleAsync(tester, LifecycleOp.Show, ct).ConfigureAwait(false);
        if (!res.Success)
        {
            return res;
        }
        try
        {
            var info = ParseInspect(res.StdOut, await ResolveNetworkAsync(ct).ConfigureAwait(false)).FirstOrDefault();
            return info is null
                ? ProvisionResult.Failed(1, res.StdOut, "docker inspect returned no container")
                : ProvisionResult.Ok(0, NormalizedShowJson(info), res.StdErr);
        }
        catch (JsonException ex)
        {
            return ProvisionResult.Failed(1, res.StdOut, $"docker inspect output was not JSON: {ex.Message}");
        }
    }

    public async Task<ProvisionResult> RunCommandAsync(ProjectTester tester, ProviderCredentials? credentials, string script, CancellationToken ct = default)
    {
        if (!Guard(tester, out var name, out var bad))
        {
            return bad!;
        }
        return await RunAsync(BuildExecArgs(name, script), ct, timeout: TimeSpan.FromMinutes(15)).ConfigureAwait(false);
    }

    /// <summary>
    /// Runner container create — the docker analogue of <c>az vm create</c> +
    /// cloud-init: <c>docker run -d</c> with the agent env, then the container's
    /// ip on the configured network as the "public ip". A run that fails after
    /// the container was created carries the container name as the resource id
    /// (F8: never lose a created resource).
    /// </summary>
    public async Task<VmCreateResult> CreateVmAsync(VmCreateRequest request, ProviderCredentials? credentials, CancellationToken ct = default)
    {
        if (!options.Enabled)
        {
            return VmCreateResult.Fail($"docker provider is disabled (set {DockerProviderOptions.EnableVar}=1)");
        }
        if (!DockerProviderOptions.IsDocker(request.Cloud))
        {
            return VmCreateResult.Fail($"unsupported cloud provider: {request.Cloud}");
        }
        try
        {
            var labels = new Dictionary<string, string>(request.Labels ?? new Dictionary<string, string>(), StringComparer.Ordinal)
            {
                [RoleLabel] = RoleRunner,
            };
            var env = request.Environment ?? new Dictionary<string, string>();
            var name = ContainerName(labels.GetValueOrDefault(ProjectLabel), request.Name);
            var network = await ResolveNetworkAsync(ct).ConfigureAwait(false);
            var addHost = NeedsHostGateway(env.GetValueOrDefault("AGENT_DASHBOARD_URL"));
            var args = BuildRunnerRunArgs(name, network, labels, env, options.RunnerImage, addHost);

            var run = await RunAsync(args, ct, timeout: TimeSpan.FromMinutes(10), sensitiveArgs: true).ConfigureAwait(false);
            if (!run.Success)
            {
                return VmCreateResult.Fail($"docker run failed for {name}: {run.Error ?? run.StdErr}");
            }

            var ip = await ContainerIpAsync(name, ct).ConfigureAwait(false);
            if (string.IsNullOrEmpty(ip))
            {
                return VmCreateResult.Fail($"container {name} started but has no ip on network '{network}'", name);
            }
            logger.LogInformation("docker runner {Container} started on {Network} at {Ip} (image {Image})", name, network, ip, options.RunnerImage);
            return VmCreateResult.Created(name, ip, request.Name);
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            throw;
        }
        catch (Exception ex)
        {
            logger.LogError(ex, "docker CreateVmAsync for {Name} threw", request.Name);
            return VmCreateResult.Fail(ex.Message);
        }
    }

    /// <summary>
    /// Reverse-resolve a target container by its ip (the deployment's
    /// <c>endpoint_ips</c> entry) so the deployment delete can tear it down.
    /// Exact-ip match on <c>networker.role=target</c> containers only.
    /// </summary>
    public async Task<ResolvedVm?> ResolveByEndpointAsync(string cloud, ProviderCredentials? credentials, string endpoint, CancellationToken ct = default)
    {
        if (!DockerProviderOptions.IsDocker(cloud) || string.IsNullOrWhiteSpace(endpoint))
        {
            return null;
        }
        var all = await ListManagedContainersAsync(ct).ConfigureAwait(false);
        var match = all.FirstOrDefault(c => c.Role == RoleTarget && string.Equals(c.Ip, endpoint.Trim(), StringComparison.Ordinal));
        return match is null ? null : new ResolvedVm(match.Name, match.Name);
    }

    // ── target containers (deploy runner) ────────────────────────────────────

    /// <summary>Inputs for <see cref="CreateTargetAsync"/>.</summary>
    public sealed record TargetContainerRequest(
        string ProjectId,
        Guid DeploymentId,
        string Label,
        string? Stack);

    /// <summary>
    /// Start an endpoint target container for a deployment. Returns the
    /// container name as resource id and its ip as the endpoint ip. The caller
    /// gates readiness with <see cref="WaitHealthyAsync"/>.
    /// </summary>
    public async Task<VmCreateResult> CreateTargetAsync(TargetContainerRequest req, CancellationToken ct = default)
    {
        if (!options.Enabled)
        {
            return VmCreateResult.Fail($"docker provider is disabled (set {DockerProviderOptions.EnableVar}=1)");
        }
        try
        {
            var stackEnv = TargetStackEnv(req.Stack);
            var suffix = Guid.NewGuid().ToString("N")[..5];
            var vmName = SanitizeNamePart($"ep-{(stackEnv == "none" ? "rust" : stackEnv)}-{req.Label}-{suffix}");
            var name = ContainerName(req.ProjectId, vmName);
            var labels = new Dictionary<string, string>(StringComparer.Ordinal)
            {
                [RoleLabel] = RoleTarget,
                [ProjectLabel] = req.ProjectId,
                [DeploymentIdLabel] = req.DeploymentId.ToString(),
                [StackLabel] = stackEnv == "none" ? "rust" : stackEnv,
            };
            var network = await ResolveNetworkAsync(ct).ConfigureAwait(false);
            var image = options.TargetImageFor(req.Stack);
            var run = await RunAsync(BuildTargetRunArgs(name, network, labels, image, req.Stack), ct, timeout: TimeSpan.FromMinutes(10))
                .ConfigureAwait(false);
            if (!run.Success)
            {
                return VmCreateResult.Fail($"docker run failed for {name} ({image}): {run.Error ?? run.StdErr}");
            }
            var ip = await ContainerIpAsync(name, ct).ConfigureAwait(false);
            if (string.IsNullOrEmpty(ip))
            {
                return VmCreateResult.Fail($"container {name} started but has no ip on network '{network}'", name);
            }
            return VmCreateResult.Created(name, ip, vmName);
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            throw;
        }
        catch (Exception ex)
        {
            logger.LogError(ex, "docker CreateTargetAsync for deployment {DeploymentId} threw", req.DeploymentId);
            return VmCreateResult.Fail(ex.Message);
        }
    }

    /// <summary>Inputs for <see cref="CreateSampleAsync"/>.</summary>
    public sealed record SampleContainerRequest(
        string ProjectId,
        Guid DeploymentId,
        string Label,
        string Sample,
        int Port,
        string Token);

    /// <summary>
    /// Start a LagHound SDK sample container for a deployment. Same contract as
    /// <see cref="CreateTargetAsync"/> — container name is the resource id, its
    /// network ip is the endpoint ip — so the deploy runner treats both roles
    /// identically. Readiness is gated by <see cref="WaitSampleHealthyAsync"/>,
    /// not the target's /health probe: a sample serves the contract prefix, not
    /// networker-endpoint's routes.
    /// </summary>
    public async Task<VmCreateResult> CreateSampleAsync(SampleContainerRequest req, CancellationToken ct = default)
    {
        if (!options.Enabled)
        {
            return VmCreateResult.Fail($"docker provider is disabled (set {DockerProviderOptions.EnableVar}=1)");
        }
        try
        {
            var suffix = Guid.NewGuid().ToString("N")[..5];
            var vmName = SanitizeNamePart($"sdk-{req.Sample}-{suffix}");
            var name = ContainerName(req.ProjectId, vmName);
            var labels = new Dictionary<string, string>(StringComparer.Ordinal)
            {
                [RoleLabel] = RoleTarget,
                [ProjectLabel] = req.ProjectId,
                [DeploymentIdLabel] = req.DeploymentId.ToString(),
                [SampleLabel] = req.Sample,
            };
            var network = await ResolveNetworkAsync(ct).ConfigureAwait(false);
            var image = options.SampleImageFor(req.Sample);
            var run = await RunAsync(
                    BuildSampleRunArgs(name, network, labels, image, req.Port, req.Token),
                    ct, timeout: TimeSpan.FromMinutes(10), sensitiveArgs: true)
                .ConfigureAwait(false);
            if (!run.Success)
            {
                return VmCreateResult.Fail(
                    $"docker run failed for {name} ({image}): {run.Error ?? run.StdErr}. "
                    + "Build the sample images first (lab/lab.sh build --samples).");
            }
            var ip = await ContainerIpAsync(name, ct).ConfigureAwait(false);
            if (string.IsNullOrEmpty(ip))
            {
                return VmCreateResult.Fail($"container {name} started but has no ip on network '{network}'", name);
            }
            return VmCreateResult.Created(name, ip, vmName);
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            throw;
        }
        catch (Exception ex)
        {
            logger.LogError(ex, "docker CreateSampleAsync for deployment {DeploymentId} threw", req.DeploymentId);
            return VmCreateResult.Fail(ex.Message);
        }
    }

    /// <summary>
    /// Force-remove every container this deployment already owns.
    ///
    /// <para>The docker path is re-entrant: an <b>update</b> re-runs the stored
    /// config, and a recovery pass re-runs an interrupted deploy. On a VM that
    /// is idempotent (install.sh re-installs in place); with containers it is
    /// not — a second run would leave the previous container running on the old
    /// ip while the row points at the new one, so the deployment would pay for
    /// (and leak) a container nothing addresses. Sweeping first makes the
    /// docker update mean the same thing as the VM update: same deployment,
    /// refreshed workload.</para>
    ///
    /// <para>Best-effort: a removal that fails is logged and left to the orphan
    /// reaper rather than failing the deploy.</para>
    /// </summary>
    public async Task<int> RemoveDeploymentContainersAsync(
        Guid deploymentId, Action<string>? progress, CancellationToken ct = default)
    {
        var wanted = deploymentId.ToString();
        var removed = 0;
        List<ContainerInfo> managed;
        try
        {
            managed = await ListManagedContainersAsync(ct).ConfigureAwait(false);
        }
        catch (Exception ex)
        {
            logger.LogWarning(ex, "docker: could not list containers before re-running deployment {DeploymentId}", deploymentId);
            return 0;
        }
        foreach (var c in managed)
        {
            if (!string.Equals(c.DeploymentId, wanted, StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }
            var res = await RemoveContainerAsync(c.Name, ct).ConfigureAwait(false);
            if (res.Success)
            {
                removed++;
                progress?.Invoke($"removed previous container {c.Name} (re-running this deployment)");
            }
            else
            {
                logger.LogWarning(
                    "docker: could not remove {Container} for deployment {DeploymentId}: {Error}",
                    c.Name, deploymentId, res.Error ?? res.StdErr);
            }
        }
        return removed;
    }

    /// <summary>
    /// Wait until an SDK sample container serves its own <c>GET /</c> — the one
    /// route the contract leaves ungated (<c>/laghound/*</c> answers a bare 404
    /// without the token, which would be indistinguishable from "not up yet").
    /// Probed from THIS process over the shared docker network rather than by
    /// <c>docker exec</c>, because the sample images are deliberately minimal
    /// and several ship no curl/wget. Returns null on success, else the reason.
    /// </summary>
    public async Task<string?> WaitSampleHealthyAsync(
        string containerName, string ip, int port, Action<string>? progress, CancellationToken ct = default)
    {
        using var client = new HttpClient { Timeout = TimeSpan.FromSeconds(3) };
        var url = $"http://{ip}:{port.ToString(CultureInfo.InvariantCulture)}/";
        var sw = Stopwatch.StartNew();
        while (sw.Elapsed < HealthyTimeout)
        {
            var show = await RunAsync(BuildLifecycleArgs(LifecycleOp.Show, containerName), ct).ConfigureAwait(false);
            if (!show.Success)
            {
                return $"container {containerName} disappeared: {show.Error ?? show.StdErr}";
            }
            ContainerInfo? info;
            try
            {
                info = ParseInspect(show.StdOut, null).FirstOrDefault();
            }
            catch (JsonException)
            {
                info = null;
            }
            if (info is null)
            {
                return $"container {containerName}: inspect returned nothing";
            }
            if (!info.Running)
            {
                var logs = await RunAsync(["logs", "--tail", "30", containerName], ct).ConfigureAwait(false);
                return $"container {containerName} is {info.Status} (exited before serving). Last log lines: "
                       + (logs.StdOut + "\n" + logs.StdErr).Trim().Replace("\r", string.Empty);
            }
            try
            {
                using var resp = await client.GetAsync(url, ct).ConfigureAwait(false);
                if (resp.IsSuccessStatusCode)
                {
                    progress?.Invoke($"{containerName}: serving on :{port} ({sw.Elapsed.TotalSeconds:0}s)");
                    return null;
                }
            }
            catch (OperationCanceledException) when (ct.IsCancellationRequested)
            {
                throw;
            }
            catch (Exception)
            {
                // Not up yet — keep waiting until the budget runs out.
            }
            await Task.Delay(TimeSpan.FromSeconds(2), ct).ConfigureAwait(false);
        }
        return $"container {containerName} did not serve :{port} within {HealthyTimeout.TotalSeconds:0}s";
    }

    /// <summary>
    /// Wait until the container reports <c>healthy</c> (image HEALTHCHECK) or,
    /// when the image has no healthcheck, until <c>curl http://127.0.0.1:8080/health</c>
    /// inside it succeeds. <paramref name="progress"/> receives one line per
    /// notable transition (streamed to the deploy log). Returns null on success,
    /// else the failure reason.
    /// </summary>
    public async Task<string?> WaitHealthyAsync(string containerName, Action<string>? progress, CancellationToken ct = default)
    {
        var sw = Stopwatch.StartNew();
        string? lastHealth = null;
        while (sw.Elapsed < HealthyTimeout)
        {
            var show = await RunAsync(BuildLifecycleArgs(LifecycleOp.Show, containerName), ct).ConfigureAwait(false);
            if (!show.Success)
            {
                return $"container {containerName} disappeared: {show.Error ?? show.StdErr}";
            }
            ContainerInfo? info;
            try
            {
                info = ParseInspect(show.StdOut, null).FirstOrDefault();
            }
            catch (JsonException)
            {
                info = null;
            }
            if (info is null)
            {
                return $"container {containerName}: inspect returned nothing";
            }
            if (!info.Running)
            {
                var logs = await RunAsync(["logs", "--tail", "30", containerName], ct).ConfigureAwait(false);
                return $"container {containerName} is {info.Status} (exited before becoming healthy). Last log lines: "
                       + (logs.StdOut + "\n" + logs.StdErr).Trim().Replace("\r", string.Empty);
            }
            if (info.Health is not null)
            {
                if (info.Health != lastHealth)
                {
                    progress?.Invoke($"{containerName}: health={info.Health} ({sw.Elapsed.TotalSeconds:0}s)");
                    lastHealth = info.Health;
                }
                if (info.Health == "healthy")
                {
                    return null;
                }
            }
            else
            {
                var curl = await RunAsync(BuildExecArgs(containerName, "curl -fsS -m 2 http://127.0.0.1:8080/health >/dev/null"), ct)
                    .ConfigureAwait(false);
                if (curl.Success)
                {
                    progress?.Invoke($"{containerName}: /health ok ({sw.Elapsed.TotalSeconds:0}s)");
                    return null;
                }
            }
            await Task.Delay(TimeSpan.FromSeconds(2), ct).ConfigureAwait(false);
        }
        return $"container {containerName} did not become healthy within {HealthyTimeout.TotalSeconds:0}s";
    }

    // ── inventory ────────────────────────────────────────────────────────────

    /// <summary>Every container (any state) carrying a <c>networker.role</c> label.</summary>
    public async Task<List<ContainerInfo>> ListManagedContainersAsync(CancellationToken ct = default)
    {
        var ids = await RunAsync(["ps", "-aq", "--filter", $"label={RoleLabel}"], ct).ConfigureAwait(false);
        if (!ids.Success)
        {
            logger.LogWarning("docker ps failed: {Err}", ids.Error ?? ids.StdErr);
            return [];
        }
        var idList = ids.StdOut.Split('\n', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries).ToList();
        if (idList.Count == 0)
        {
            return [];
        }
        var args = new List<string> { "inspect", "--type", "container" };
        args.AddRange(idList);
        var inspect = await RunAsync(args, ct).ConfigureAwait(false);
        if (!inspect.Success && string.IsNullOrWhiteSpace(inspect.StdOut))
        {
            logger.LogWarning("docker inspect failed: {Err}", inspect.Error ?? inspect.StdErr);
            return [];
        }
        try
        {
            return ParseInspect(inspect.StdOut, await ResolveNetworkAsync(ct).ConfigureAwait(false));
        }
        catch (JsonException ex)
        {
            logger.LogWarning(ex, "docker inspect output was not JSON");
            return [];
        }
    }

    /// <summary>Force-remove a container by name (reaper). Missing = success.</summary>
    public async Task<ProvisionResult> RemoveContainerAsync(string containerName, CancellationToken ct = default)
    {
        var res = await RunAsync(BuildLifecycleArgs(LifecycleOp.Delete, containerName), ct).ConfigureAwait(false);
        return !res.Success && LooksMissing(res.StdErr) ? ProvisionResult.Ok(0, res.StdOut, res.StdErr) : res;
    }

    /// <summary>The container's ip on the resolved network (null when none).</summary>
    public async Task<string?> ContainerIpAsync(string containerName, CancellationToken ct = default)
    {
        var network = await ResolveNetworkAsync(ct).ConfigureAwait(false);
        // The ip can lag `docker run -d` by a beat on some daemons; retry briefly.
        for (var attempt = 0; attempt < 10; attempt++)
        {
            var show = await RunAsync(BuildLifecycleArgs(LifecycleOp.Show, containerName), ct).ConfigureAwait(false);
            if (!show.Success)
            {
                return null;
            }
            try
            {
                var info = ParseInspect(show.StdOut, network).FirstOrDefault();
                if (!string.IsNullOrEmpty(info?.Ip))
                {
                    return info!.Ip;
                }
            }
            catch (JsonException)
            {
                return null;
            }
            await Task.Delay(500, ct).ConfigureAwait(false);
        }
        return null;
    }

    /// <summary>
    /// The network containers attach to: the configured one, else the network
    /// this process's own container is on (<c>docker inspect $(hostname)</c>),
    /// else <c>nwk-lab_labnet</c> when it exists, else <c>bridge</c>. Cached.
    /// </summary>
    public async Task<string> ResolveNetworkAsync(CancellationToken ct = default)
    {
        if (options.Network is { Length: > 0 } configured)
        {
            return configured;
        }
        if (_resolvedNetwork is not null)
        {
            return _resolvedNetwork;
        }
        await _networkGate.WaitAsync(ct).ConfigureAwait(false);
        try
        {
            if (_resolvedNetwork is not null)
            {
                return _resolvedNetwork;
            }
            string? net = null;
            var host = Environment.MachineName;
            if (!string.IsNullOrEmpty(host))
            {
                var self = await RunAsync(["inspect", "--type", "container", "--format",
                    "{{range $k,$v := .NetworkSettings.Networks}}{{$k}}\n{{end}}", host], ct).ConfigureAwait(false);
                if (self.Success)
                {
                    net = self.StdOut.Split('\n', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries).FirstOrDefault();
                }
            }
            if (net is null)
            {
                var lab = await RunAsync(["network", "inspect", DockerProviderOptions.LabNetwork, "--format", "{{.Name}}"], ct).ConfigureAwait(false);
                net = lab.Success ? DockerProviderOptions.LabNetwork : "bridge";
            }
            _resolvedNetwork = net;
            logger.LogInformation("docker provider: containers will attach to network '{Network}' (set {Var} to override)", net, DockerProviderOptions.NetworkVar);
            return net;
        }
        finally
        {
            _networkGate.Release();
        }
    }

    /// <summary>Linux hosts need <c>--add-host host.docker.internal:host-gateway</c>
    /// for containers to reach a control plane running on the host.</summary>
    public static bool NeedsHostGateway(string? agentUrl) =>
        agentUrl is not null
        && agentUrl.Contains("host.docker.internal", StringComparison.OrdinalIgnoreCase)
        && !OperatingSystem.IsWindows() && !OperatingSystem.IsMacOS();

    // ── internals ────────────────────────────────────────────────────────────

    private async Task<ProvisionResult> LifecycleAsync(ProjectTester tester, LifecycleOp op, CancellationToken ct)
    {
        if (!Guard(tester, out var name, out var bad))
        {
            return bad!;
        }
        var res = await RunAsync(BuildLifecycleArgs(op, name), ct).ConfigureAwait(false);
        if (op == LifecycleOp.Delete && !res.Success && LooksMissing(res.StdErr))
        {
            logger.LogInformation("docker container {Container} already gone; treating delete as success", name);
            return ProvisionResult.Ok(res.ExitCode ?? 0, res.StdOut, res.StdErr);
        }
        return res;
    }

    private bool Guard(ProjectTester tester, out string name, out ProvisionResult? failure)
    {
        name = tester.VmResourceId ?? string.Empty;
        failure = null;
        if (!DockerProviderOptions.IsDocker(tester.Cloud))
        {
            failure = ProvisionResult.Unsupported(tester.Cloud ?? "(null)");
            return false;
        }
        if (!options.Enabled)
        {
            failure = ProvisionResult.SpawnError($"docker provider is disabled (set {DockerProviderOptions.EnableVar}=1)");
            return false;
        }
        if (string.IsNullOrEmpty(name))
        {
            failure = ProvisionResult.SpawnError("tester has no vm_resource_id; nothing to act on at the docker layer");
            return false;
        }
        return true;
    }

    /// <summary>Hardened process runner (same shape as the cloud CLI runner):
    /// streams drained concurrently, tree-kill on timeout, spawn failure → SpawnError.</summary>
    internal async Task<ProvisionResult> RunAsync(
        List<string> args, CancellationToken cancellationToken, TimeSpan? timeout = null, bool sensitiveArgs = false)
    {
        var file = options.DockerBin;
        var psi = new ProcessStartInfo
        {
            FileName = file,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true,
        };
        foreach (var a in args)
        {
            psi.ArgumentList.Add(a);
        }

        logger.LogInformation(
            "docker provisioner spawning {File} {Args}",
            file,
            sensitiveArgs ? RedactSensitive(args) : string.Join(' ', args));

        using var timeoutCts = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeoutCts.CancelAfter(timeout ?? CommandTimeout);
        var ct = timeoutCts.Token;

        using var process = new Process { StartInfo = psi };
        try
        {
            process.Start();
        }
        catch (Exception ex)
        {
            var message = $"failed to launch '{file}': {ex.Message}. Install the docker CLI on the control-plane host "
                          + $"(and mount /var/run/docker.sock when containerised), or set {DockerProviderOptions.BinVar}.";
            logger.LogWarning(ex, "Failed to launch docker CLI '{File}'", file);
            return ProvisionResult.SpawnError(message);
        }

        var stdoutTask = process.StandardOutput.ReadToEndAsync(ct);
        var stderrTask = process.StandardError.ReadToEndAsync(ct);
        string stdout, stderr;
        try
        {
            await process.WaitForExitAsync(ct).ConfigureAwait(false);
            stdout = (await stdoutTask.ConfigureAwait(false)).Trim();
            stderr = (await stderrTask.ConfigureAwait(false)).Trim();
        }
        catch (OperationCanceledException) when (timeoutCts.IsCancellationRequested && !cancellationToken.IsCancellationRequested)
        {
            KillTree(process);
            return ProvisionResult.SpawnError($"docker CLI timed out after {(timeout ?? CommandTimeout).TotalSeconds:0}s and was killed");
        }
        catch (OperationCanceledException)
        {
            KillTree(process);
            throw;
        }

        return process.ExitCode == 0
            ? ProvisionResult.Ok(process.ExitCode, stdout, stderr)
            : ProvisionResult.Failed(process.ExitCode, stdout, stderr);
    }

    /// <summary>Log-safe rendering of a run argv: <c>-e AGENT_API_KEY=…</c> and
    /// <c>-e LAGHOUND_TOKEN=…</c> are masked.</summary>
    internal static string RedactSensitive(IReadOnlyList<string> args)
    {
        var parts = new List<string>(args.Count);
        for (var i = 0; i < args.Count; i++)
        {
            var a = args[i];
            if (i > 0 && args[i - 1] == "-e" && a.StartsWith("AGENT_API_KEY=", StringComparison.Ordinal))
            {
                parts.Add("AGENT_API_KEY=***");
            }
            else if (i > 0 && args[i - 1] == "-e" && a.StartsWith("LAGHOUND_TOKEN=", StringComparison.Ordinal))
            {
                parts.Add("LAGHOUND_TOKEN=***");
            }
            else
            {
                parts.Add(a);
            }
        }
        return string.Join(' ', parts);
    }

    private static void KillTree(Process process)
    {
        try
        {
            if (!process.HasExited)
            {
                process.Kill(entireProcessTree: true);
            }
        }
        catch
        {
            // best-effort
        }
    }
}
