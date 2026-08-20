using System.Diagnostics;
using System.Globalization;
using System.Text.RegularExpressions;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Endpoints;
using Networker.ControlPlane.Realtime;
using Networker.Data;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// The C# port of the Rust dashboard's deploy runner
/// (<c>crates/networker-dashboard/src/deploy/runner.rs</c>
/// <c>run_deployment</c>).
///
/// <para>Responsibilities, 1:1 with the Rust source:</para>
/// <list type="number">
///   <item>Write the generated <c>deploy.json</c> to a temp file.</item>
///   <item>Shell <c>bash install.sh --deploy &lt;file&gt;</c> — stdout AND stderr
///     piped, stdin nulled, streamed line-by-line so a wedged install can be
///     tree-killed.</item>
///   <item>Stream every output line to the browser via
///     <see cref="EventBus.Publish"/> as a <see cref="DeployLog"/> (tagged with
///     its origin stream), deduping identical lines the same way Rust's
///     <c>DeployOutput::process_line</c> does.</item>
///   <item>Parse endpoint hosts out of the output — FQDN-with-IP-in-parens
///     preferred over a bare IP, with the same fallback scan for IPs near
///     "endpoint"/"deployed"/"public ip" lines — plus, per endpoint, the DNS
///     name install.sh reports as <c>endpoint_host: &lt;name&gt; (&lt;ip&gt;)</c>
///     (Azure DNS label / AWS public DNS), kept PARALLEL to the ips.</item>
///   <item>On success: persist <c>endpoint_ips</c> + <c>endpoint_hosts</c>
///     (V050; the proxy resolver prefers the hostname — SNI → HTTP/3 through
///     IIS) + status <c>completed</c>.
///     On failure: persist <c>error_message</c> + status <c>failed</c> and
///     leave any existing <c>endpoint_ips</c>/<c>endpoint_hosts</c> untouched
///     (an update re-run over a live deployment must not orphan its
///     still-serving endpoints). Either way persist the full log and publish
///     a <see cref="DeployComplete"/>.</item>
/// </list>
///
/// <para><b>CI-safe soft-fail:</b> if <c>install.sh</c> can't be located, or
/// <c>bash</c>/the script fails to launch, the runner does NOT throw — it marks
/// the deployment <c>failed</c> with a descriptive error and publishes
/// <c>DeployComplete{status:"failed"}</c>. This mirrors the
/// <see cref="CliComputeProvisioner"/> "missing CLI ⇒ soft failure" contract so
/// the provisioning path works end-to-end on a dev box / CI runner that has no
/// install.sh or cloud CLIs, without crashing the background worker.</para>
///
/// <para>Process handling reuses the hardened pattern from
/// <see cref="CliComputeProvisioner"/>: streams drained line-by-line off the
/// live pipes, a hard timeout that tree-kills a wedged install,
/// <c>UseShellExecute=false</c> + <c>CreateNoWindow=true</c>, and
/// <c>kill_on_drop</c>-equivalent tree kill on cancel.</para>
/// </summary>
public sealed class DeployRunner
{
    // install.sh cloud provisioning (az/aws/gcloud VM create + apt/choco installs)
    // is slow; give it a generous ceiling but still bound it so a hung install
    // can't pin a background worker forever. The Rust runner has no explicit
    // timeout (it relies on install.sh's own guards). The base is a safe
    // backstop for stack-only deploys; reference-API languages each add a
    // serial install (on Windows: an az run-command round-trip + SDK-sized
    // downloads — the .NET 10 SDK alone is 300 MB), so the budget scales per
    // language. A 3-stack + 8-language Windows deploy was killed at the flat
    // 30m ceiling mid-install (field, 2026-08-16).
    // Internal (not private): the WatchdogService's stale-deploy sweep uses the
    // base as its SQL prefilter floor so its threshold can never undercut the
    // runner's budget (issue #804 — the watchdog killed a legitimate cpp deploy
    // at a flat 30m while the runner's scaled budget was 38m).
    internal static readonly TimeSpan BaseDeployTimeout = TimeSpan.FromMinutes(30);
    private static readonly TimeSpan PerLanguageBudget = TimeSpan.FromMinutes(8);
    private static readonly TimeSpan MaxDeployTimeout = TimeSpan.FromMinutes(120);

    /// <summary>How many per-language installs the deploy config requests,
    /// summed across all endpoints: reference-API <c>languages</c> AND SDK
    /// <c>sdk_samples</c>. Both are serial toolchain-sized installs on the
    /// target (an SDK sample compiles the sample app from source — the Rust one
    /// is a full cargo release build), so both must scale the budget; a sample
    /// deploy that is only counted as "0 languages" gets the flat 30m base and
    /// is tree-killed mid-build. 0 for stack-only or unparseable configs (the
    /// latter fail validation inside install.sh anyway).</summary>
    internal static int LanguageCountFor(string deployJson)
    {
        var languages = 0;
        try
        {
            using var doc = System.Text.Json.JsonDocument.Parse(deployJson);
            if (doc.RootElement.TryGetProperty("endpoints", out var eps)
                && eps.ValueKind == System.Text.Json.JsonValueKind.Array)
            {
                foreach (var ep in eps.EnumerateArray())
                {
                    if (ep.TryGetProperty("languages", out var langs)
                        && langs.ValueKind == System.Text.Json.JsonValueKind.Array)
                    {
                        languages += langs.GetArrayLength();
                    }
                    if (ep.TryGetProperty("sdk_samples", out var samples)
                        && samples.ValueKind == System.Text.Json.JsonValueKind.Array)
                    {
                        languages += samples.GetArrayLength();
                    }
                }
            }
        }
        catch (System.Text.Json.JsonException)
        {
            // Unparseable config fails validation inside install.sh anyway.
        }
        return languages;
    }

    /// <summary>Workload-scaled install.sh budget: base + 8m per requested
    /// reference-API language across all endpoints, capped at 2h. The single
    /// source of truth for the deploy budget — the watchdog's stale-deploy
    /// sweep derives its (later) reap threshold from this same function, so
    /// the runner's own timeout always fires first with its richer message.
    ///
    /// <para>Issue #817 — how the budget AGES: install.sh spends a long,
    /// quota-contended stretch on cloud provisioning (resource group + VM
    /// create + boot + SSH wait) BEFORE any reference-API install runs, and in
    /// a saturated matrix that stretch alone ate most of the budget — slow but
    /// healthy installs (AOT publishes on a B2s) were then tree-killed
    /// mid-publish and surfaced as terminal "exited with code -1". The budget
    /// is therefore RE-ANCHORED when the install phase actually starts (the
    /// first <see cref="InstallPhaseMarkerRe"/> line install.sh prints):
    /// <see cref="ShellInstallAsync"/> re-arms its timer with the full scaled
    /// budget from that moment, and <see cref="StampInstallStartAsync"/>
    /// re-stamps <c>deployment.started_at</c> so the watchdog's reap threshold
    /// ages from the same anchor. Provisioning/queue time is bounded by this
    /// budget from spawn as before; it just no longer counts against the
    /// install itself.</para></summary>
    internal static TimeSpan DeployTimeoutFor(string deployJson)
    {
        var total = BaseDeployTimeout + LanguageCountFor(deployJson) * PerLanguageBudget;
        return total > MaxDeployTimeout ? MaxDeployTimeout : total;
    }

    /// <summary>The install.sh line that marks the actual start of the install
    /// phase: <c>next_step "Install $lang reference API …"</c> renders as
    /// <c>Step N: Install &lt;lang&gt; reference API on &lt;ip&gt; …</c> (remote),
    /// <c>… locally …</c> (local provider) or <c>… (Azure Windows)</c> — always
    /// AFTER the VM exists and SSH answers. Piped output carries no ANSI codes
    /// (install.sh gates colors on <c>-t 1</c>), so the line is matched bare.
    /// <c>Step N: Install &lt;lang&gt; SDK sample …</c> is the same marker for the
    /// SDK sample installs (<c>endpoints[].sdk_samples</c>), which are equally
    /// toolchain-sized and equally post-provisioning.</summary>
    internal static readonly Regex InstallPhaseMarkerRe = new(
        @"^Step \d+: Install \S+ (reference API|SDK sample)",
        RegexOptions.Compiled);

    // Matches "hostname.eastus.cloudapp.azure.com (20.127.36.61)" — FQDN + IP in
    // parens. Ported verbatim from Rust DeployOutput::fqdn_re.
    private static readonly Regex FqdnRe = new(
        @"([-a-z0-9]+(?:\.[-a-z0-9]+)*\.(?:cloudapp\.azure\.com|amazonaws\.com|compute\.googleapis\.com))\s+\((\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})\)",
        RegexOptions.Compiled);

    // Matches "endpoint_ip: 1.2.3.4" / "deployed to 1.2.3.4" / "public ip 1.2.3.4".
    // Ported verbatim from Rust DeployOutput::ip_re.
    private static readonly Regex IpRe = new(
        @"(?i)(?:endpoint[_ ](?:ip|address)|deployed[_ ](?:to|at)|public[_ ]ip)[:\s]+(\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})",
        RegexOptions.Compiled);

    // Matches install.sh's explicit per-endpoint hostname report
    // "endpoint_host: nwk-ep-x.eastus.cloudapp.azure.com (20.1.2.3)" — the DNS
    // name the tester should connect to (V050 endpoint_hosts), tied to the ip
    // so the two arrays stay parallel however the surrounding lines were parsed.
    private static readonly Regex EndpointHostRe = new(
        @"(?i)endpoint[_ ]host[:\s]+([A-Za-z0-9](?:[-A-Za-z0-9.]*[A-Za-z0-9])?)\s+\((\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})\)",
        RegexOptions.Compiled);

    // Bare-IP fallback scanner (only applied to lines mentioning endpoint/deployed/public ip).
    private static readonly Regex BareIpRe = new(
        @"\b(\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})\b",
        RegexOptions.Compiled);

    private readonly IServiceScopeFactory _scopeFactory;
    private readonly EventBus _bus;
    private readonly ILogger<DeployRunner> _logger;
    private readonly IHostApplicationLifetime? _lifetime;

    public DeployRunner(
        IServiceScopeFactory scopeFactory,
        EventBus bus,
        ILogger<DeployRunner> logger,
        IHostApplicationLifetime? lifetime = null)
    {
        _scopeFactory = scopeFactory;
        _bus = bus;
        _logger = logger;
        // Optional (bare test hosts have no host lifetime): lets the exit
        // classifier recognize kills that happen inside a control-plane
        // shutdown window even when install.sh laundered the signal into a
        // plain exit 1/-1 (issue #817; evidence via #804's machinery).
        _lifetime = lifetime;
    }

    /// <summary>
    /// Run the deployment identified by <paramref name="deploymentId"/> using the
    /// provided <paramref name="deployJson"/> document. Never throws for an
    /// infrastructure/CLI failure — a failed deploy is recorded on the row and
    /// signalled via <see cref="DeployComplete"/>. Returns the parsed endpoint
    /// hosts (empty on failure).
    /// </summary>
    public async Task<IReadOnlyList<string>> RunDeploymentAsync(
        Guid deploymentId, string deployJson, CancellationToken ct)
    {
        // Docker (local) provider: endpoints with provider "docker" become target
        // containers on the control-plane host — no install.sh, no VM. Same
        // terminal contract (status/endpoint_ips/log/DeployComplete) as the
        // shell path so the proxy resolver, the orchestrator's readiness gate
        // and the UI see no difference.
        var dockerPlan = DockerDeployPlan.TryParse(deployJson, out var dockerPlanError);
        if (dockerPlanError is not null)
        {
            _logger.LogWarning("Deployment {DeploymentId} rejected: {Error}", deploymentId, dockerPlanError);
            await FinishAsync(deploymentId, success: false, ips: [], log: dockerPlanError, error: dockerPlanError, ct)
                .ConfigureAwait(false);
            return [];
        }
        if (dockerPlan is not null)
        {
            return await RunDockerDeploymentAsync(deploymentId, dockerPlan, deployJson, ct).ConfigureAwait(false);
        }

        var deployFile = Path.Combine(Path.GetTempPath(), $"deploy-{deploymentId}.json");
        try
        {
            // deploy.json carries the minted agent API key — write it 0600, never
            // world-readable in the shared temp dir (quality audit F11).
            await SecretFile.WriteAsync(deployFile, deployJson, ct).ConfigureAwait(false);
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Failed to write deploy.json for {DeploymentId}", deploymentId);
            await FinishAsync(deploymentId, success: false, ips: [],
                log: null, error: $"failed to write deploy.json: {ex.Message}", ct)
                .ConfigureAwait(false);
            return [];
        }

        _logger.LogInformation(
            "Starting install.sh --deploy for {DeploymentId} ({DeployFile})",
            deploymentId, deployFile);

        var installSh = FindInstallSh();
        if (installSh is null)
        {
            // CI / dev box with no installer present: soft-fail cleanly rather
            // than crash the worker. Mirrors CliComputeProvisioner's missing-CLI path.
            const string msg =
                "install.sh not found (set INSTALL_SH_PATH); provisioning shell-out skipped";
            _logger.LogWarning("{Message} for deployment {DeploymentId}", msg, deploymentId);
            await FinishAsync(deploymentId, success: false, ips: [], log: msg, error: msg, ct)
                .ConfigureAwait(false);
            TryDelete(deployFile);
            return [];
        }

        // Flip to running + emit the opening log line, matching the Rust runner.
        await SetStatusAsync(deploymentId, "running", ct).ConfigureAwait(false);
        _bus.Publish(new DeployLog(deploymentId, "Deployment started...", "stdout"));

        var output = new DeployOutput();

        // GCP endpoints: install.sh provisions the VM with the gcloud CLI, which
        // authenticates ONLY from its config store or a per-process credential
        // override — never from anything this process knows. Stage the cloud
        // account's service-account key for the installer (#833); the outcome
        // line goes into the persisted log so a pre-flight failure is never a
        // guessing game again. The staging dir is deleted when this scope ends,
        // whatever path the deploy takes out of here.
        var gcp = await GcpInstallerCredentials.PrepareAsync(_scopeFactory, deploymentId, deployJson, _logger, ct)
            .ConfigureAwait(false);
        using var gcpCredentials = gcp.Credentials;
        if (gcp.Note is not null)
        {
            PumpLine(deploymentId, output, gcp.Note, "stdout");
        }

        // SDK sample endpoints: the token the samples will require rides the
        // config as ciphertext (it is readable by every project member there);
        // hand the plaintext to install.sh through the environment only, so it
        // never reaches the log or the stored config.
        var sampleToken = SdkSampleInstallerToken.Resolve(_scopeFactory, deployJson, _logger);
        if (sampleToken.Note is not null)
        {
            PumpLine(deploymentId, output, sampleToken.Note, "stdout");
        }
        var installerEnv = SdkSampleInstallerToken.Merge(gcpCredentials?.Env, sampleToken.Env);

        int? exitCode;
        try
        {
            exitCode = await ShellInstallAsync(deploymentId, installSh, deployFile, output, installerEnv, ct)
                .ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            // Caller cancelled (shutdown). The deployment must NOT stick at
            // running/pending — persist it as failed (best-effort, under
            // CancellationToken.None inside FinishAsync) before rethrowing so the
            // orchestrator's DeploymentFailed arm can fail the run (quality audit
            // F3(c)).
            await FinishAsync(deploymentId, success: false, ips: [],
                log: output.FullLog, error: "Deployment cancelled", ct)
                .ConfigureAwait(false);
            TryDelete(deployFile);
            throw;
        }
        catch (Exception ex)
        {
            // bash/install.sh failed to even launch — soft-fail like the CLI provisioner.
            var msg = $"failed to launch install.sh: {ex.Message}";
            _logger.LogWarning(ex, "install.sh launch failed for {DeploymentId}", deploymentId);
            await FinishAsync(deploymentId, success: false, ips: [],
                log: output.FullLog, error: msg, ct).ConfigureAwait(false);
            TryDelete(deployFile);
            return [];
        }

        output.RunFallbackIpScan();

        var success = exitCode == 0;
        var error = ClassifyExit(
            exitCode,
            shuttingDown: _lifetime?.ApplicationStopping.IsCancellationRequested == true);
        // Terminal (non-interrupted) failures carry the enriched #816 message —
        // the last fatal "✗ …" line and the step that died — instead of the bare
        // exit code. Interruption markers (#764/#817) stay verbatim: the startup
        // recovery pass and the orchestrator's retry arm key on the prefix.
        if (error is not null
            && !error.StartsWith(ProvisioningFailureClassifier.InterruptedErrorPrefix, StringComparison.Ordinal))
        {
            error = BuildFailureMessage(exitCode, output);
        }
        await FinishAsync(deploymentId, success, output.EndpointIps, output.FullLog, error, ct, output.EndpointHosts,
                exitCode, success ? null : output.CurrentStep)
            .ConfigureAwait(false);

        TryDelete(deployFile);

        _logger.LogInformation(
            "Deployment {DeploymentId} finished status={Status} ips={Ips} hosts={Hosts}",
            deploymentId, success ? "completed" : "failed", string.Join(",", output.EndpointIps),
            string.Join(",", output.EndpointHosts.Select(h => h ?? "-")));

        return output.EndpointIps;
    }

    /// <summary>
    /// Classify install.sh's exit into the terminal error message (null on
    /// success). Static + argument-driven so the shapes are unit-testable.
    ///
    /// <list type="bullet">
    ///   <item><b>143/137</b> — SIGTERM/SIGKILL: install.sh is a CHILD of this
    ///     process, so a control-plane restart (every deploy) kills any
    ///     in-flight deployment — prod produced a bare "install.sh exited with
    ///     code 143" plus an orphan Azure VM, with no hint that the cause was a
    ///     restart rather than the customer's config (prod sweep, v0.28.213).
    ///     Say so, and point at the retry.</item>
    ///   <item><b>Any non-zero exit during a control-plane shutdown window</b> —
    ///     the SIGTERM often lands on install.sh's CHILD (ssh/az) first;
    ///     install.sh observes the child failure and exits 1 (or the tree-kill
    ///     surfaces as -1) BEFORE its own signal disposition runs, so the kill
    ///     arrived without the 143/137 code and was misclassified as a terminal
    ///     install failure (issue #817, evidence via #804). If the host is
    ///     stopping, the failure is the shutdown, not the config.</item>
    /// </list>
    ///
    /// Both interruption shapes are built on the classifier's shared prefix:
    /// the startup recovery pass and the orchestrator's interrupted-retry arm
    /// key on that exact marker to auto-re-run the deployment (issue #764).
    /// Everything else keeps the plain terminal message.
    /// </summary>
    internal static string? ClassifyExit(int? exitCode, bool shuttingDown)
    {
        if (exitCode == 0)
        {
            return null;
        }
        if (exitCode is 143 or 137)
        {
            return ProvisioningFailureClassifier.InterruptedErrorPrefix
                + $"{(exitCode == 137 ? "KILL" : "TERM")}, exit {exitCode}) — "
                + "the control plane restarted or was shut down while deploying. Any VM it had already created is "
                + "reaped by the orphan sweep; retry the deployment.";
        }
        if (shuttingDown)
        {
            return ProvisioningFailureClassifier.InterruptedErrorPrefix
                + $"TERM during control-plane shutdown, exit {exitCode ?? -1}) — "
                + "the shutdown killed part of the install tree before install.sh could report the signal itself. "
                + "Any VM it had already created is reaped by the orphan sweep; retry the deployment.";
        }
        return $"install.sh exited with code {exitCode ?? -1}";
    }

    /// <summary>Failure message for a non-signal install.sh exit: the exit code
    /// plus the FIRST ACTIONABLE detail from the output — the last fatal
    /// "✗ …" line install.sh printed, and the step it died in. Issue #816: an
    /// unattended matrix cell's run row used to carry only "install.sh exited
    /// with code 1", which diagnoses nothing; the orchestrator's
    /// DeploymentFailed arm copies this message onto the run verbatim.</summary>
    internal static string BuildFailureMessage(int? exitCode, DeployOutput output)
    {
        var msg = $"install.sh exited with code {exitCode ?? -1}";
        if (output.LastErrorLine is { Length: > 0 } detail)
        {
            msg += $" — {detail}";
        }
        if (output.CurrentStep is { Length: > 0 } step)
        {
            msg += $" (during \"{step}\")";
        }
        return msg;
    }

    // ── Docker (local) provider ──────────────────────────────────────────────

    /// <summary>
    /// Start one container per docker endpoint — a target container, or a
    /// LagHound SDK sample container when the endpoint declares
    /// <c>sdk_samples</c> — gate each on its readiness probe, and finish the
    /// deployment exactly like the shell path (<c>completed</c> +
    /// <c>endpoint_ips</c> = container ips, log streamed via
    /// <see cref="DeployLog"/>). Any failure force-removes the containers this
    /// deployment already started (nothing to bill, but nothing to leak either)
    /// and records <c>failed</c>.
    /// </summary>
    private async Task<IReadOnlyList<string>> RunDockerDeploymentAsync(
        Guid deploymentId, DockerDeployPlan plan, string deployJson, CancellationToken ct)
    {
        var output = new DeployOutput();
        void Log(string line, string stream = "stdout")
        {
            if (output.ProcessLine(line, stream, out var clean))
            {
                _bus.Publish(new DeployLog(deploymentId, clean, stream));
            }
        }

        DockerComputeProvisioner? docker;
        string? projectId;
        using (var scope = _scopeFactory.CreateScope())
        {
            docker = scope.ServiceProvider.GetService<DockerComputeProvisioner>();
            var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
            projectId = await db.Deployments.AsNoTracking()
                .Where(d => d.DeploymentId == deploymentId)
                .Select(d => d.ProjectId)
                .FirstOrDefaultAsync(ct)
                .ConfigureAwait(false);
        }

        if (docker is null || !docker.Options.Enabled)
        {
            var msg = $"docker endpoints require the Docker (local) provider ({DockerProviderOptions.EnableVar}=1) on this control plane";
            _logger.LogWarning("{Message} (deployment {DeploymentId})", msg, deploymentId);
            await FinishAsync(deploymentId, success: false, ips: [], log: msg, error: msg, ct).ConfigureAwait(false);
            return [];
        }

        await SetStatusAsync(deploymentId, "running", ct).ConfigureAwait(false);
        Log("Deployment started...");
        // Re-run (update / recovery): drop the containers this deployment
        // already owns first. install.sh re-installs in place on a VM; a second
        // `docker run` would instead leave the old container serving the old ip
        // that nothing points at any more.
        var swept = await docker.RemoveDeploymentContainersAsync(deploymentId, line => Log(line), ct)
            .ConfigureAwait(false);
        if (swept > 0)
        {
            Log($"re-running deployment: removed {swept} previous container(s)");
        }
        var network = await docker.ResolveNetworkAsync(ct).ConfigureAwait(false);
        Log($"provider: docker (local) — starting {plan.Endpoints.Count} target container(s) on network '{network}'");

        using var flushCts = CancellationTokenSource.CreateLinkedTokenSource(ct);
        var flusher = FlushLogPeriodicallyAsync(deploymentId, output, flushCts.Token);

        var ips = new List<string>();
        // Parallel to ips: the container name — resolvable by every container on
        // the same docker network (embedded DNS), so runners provisioned by the
        // docker provider address the target by hostname like a cloud FQDN.
        var hosts = new List<string?>();
        var created = new List<string>();
        string? error = null;
        try
        {
            // SDK sample containers need the token their /laghound routes will
            // require; it rides the config as ciphertext (see
            // SdkSampleInstallerToken) and is resolved once for the deployment.
            var sampleToken = plan.Endpoints.Any(e => e.Sample is not null)
                ? SdkSampleInstallerToken.Resolve(_scopeFactory, deployJson, _logger)
                : SdkSampleInstallerToken.Outcome.None;

            foreach (var ep in plan.Endpoints)
            {
                VmCreateResult res;
                if (ep.Sample is { } sampleId)
                {
                    var sample = SdkSampleCatalog.Find(sampleId);
                    if (sample is null)
                    {
                        error = $"endpoints[{ep.Index}]: unknown SDK sample '{sampleId}'";
                        Log($"ERROR: {error}", "stderr");
                        break;
                    }
                    if (!sampleToken.Env.TryGetValue(SdkSampleInstallerToken.EnvVar, out var token))
                    {
                        error = $"endpoints[{ep.Index}]: no LagHound sample token staged on this deployment — "
                                + "recreate it from the SDK Endpoints page";
                        Log($"ERROR: {error}", "stderr");
                        break;
                    }
                    var sampleImage = docker.Options.SampleImageFor(sample.Id);
                    Log($"endpoints[{ep.Index}] {ep.Label}: docker run {sampleImage} (PORT={sample.Port}, LagHound {sample.Language} sample {sample.SdkVersion})");
                    res = await docker.CreateSampleAsync(
                        new DockerComputeProvisioner.SampleContainerRequest(
                            projectId ?? string.Empty, deploymentId, ep.Label, sample.Id, sample.Port, token), ct)
                        .ConfigureAwait(false);
                    if (!string.IsNullOrEmpty(res.ResourceId))
                    {
                        created.Add(res.ResourceId);
                    }
                    if (!res.Success)
                    {
                        error = res.Error ?? "docker run failed";
                        Log($"ERROR: {error}", "stderr");
                        break;
                    }
                    Log($"container {res.ResourceId} started at {res.PublicIp} — waiting for :{sample.Port}");
                    var sampleErr = await docker
                        .WaitSampleHealthyAsync(res.ResourceId!, res.PublicIp!, sample.Port, line => Log(line), ct)
                        .ConfigureAwait(false);
                    if (sampleErr is not null)
                    {
                        error = sampleErr;
                        Log($"ERROR: {sampleErr}", "stderr");
                        break;
                    }
                    Log($"endpoint_ip: {res.PublicIp} ({ep.Label}, {sample.Language} sample on :{sample.Port})");
                    Log($"endpoint_host: {res.ResourceId} ({res.PublicIp}) — container name, docker-network DNS");
                    ips.Add(res.PublicIp!);
                    hosts.Add(res.ResourceId);
                    continue;
                }

                var image = docker.Options.TargetImageFor(ep.Stack);
                Log($"endpoints[{ep.Index}] {ep.Label}: docker run {image} (TARGET_STACK={DockerComputeProvisioner.TargetStackEnv(ep.Stack)})");
                res = await docker.CreateTargetAsync(
                    new DockerComputeProvisioner.TargetContainerRequest(projectId ?? string.Empty, deploymentId, ep.Label, ep.Stack), ct)
                    .ConfigureAwait(false);
                if (!string.IsNullOrEmpty(res.ResourceId))
                {
                    created.Add(res.ResourceId);
                }
                if (!res.Success)
                {
                    error = res.Error ?? "docker run failed";
                    Log($"ERROR: {error}", "stderr");
                    break;
                }
                Log($"container {res.ResourceId} started at {res.PublicIp} — waiting for health");
                var healthErr = await docker.WaitHealthyAsync(res.ResourceId!, line => Log(line), ct).ConfigureAwait(false);
                if (healthErr is not null)
                {
                    error = healthErr;
                    Log($"ERROR: {healthErr}", "stderr");
                    break;
                }
                var stackNote = ep.Stack is null
                    ? "bare endpoint 8080/8443"
                    : $"{ep.Stack} on :{ProvisioningOrchestrator.ProxyHttpsPort(ep.Stack)}";
                Log($"endpoint_ip: {res.PublicIp} ({ep.Label}, {stackNote})");
                Log($"endpoint_host: {res.ResourceId} ({res.PublicIp}) — container name, docker-network DNS");
                ips.Add(res.PublicIp!);
                hosts.Add(res.ResourceId);
            }
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            foreach (var name in created)
            {
                await docker.RemoveContainerAsync(name, CancellationToken.None).ConfigureAwait(false);
            }
            await FinishAsync(deploymentId, success: false, ips: [], log: output.FullLog, error: "Deployment cancelled", ct)
                .ConfigureAwait(false);
            throw;
        }
        finally
        {
            flushCts.Cancel();
            try
            {
                await flusher.ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                // normal
            }
        }

        if (error is not null)
        {
            foreach (var name in created)
            {
                Log($"removing {name} after failure", "stderr");
                await docker.RemoveContainerAsync(name, CancellationToken.None).ConfigureAwait(false);
            }
            await FinishAsync(deploymentId, success: false, ips: [], log: output.FullLog, error: error, ct).ConfigureAwait(false);
            _logger.LogInformation("Deployment {DeploymentId} (docker) finished status=failed: {Error}", deploymentId, error);
            return [];
        }

        Log($"deployed {ips.Count} docker target(s): {string.Join(", ", ips)}");
        await FinishAsync(deploymentId, success: true, ips, output.FullLog, error: null, ct, hosts).ConfigureAwait(false);
        _logger.LogInformation(
            "Deployment {DeploymentId} (docker) finished status=completed ips={Ips}", deploymentId, string.Join(",", ips));
        return ips;
    }

    // ── Process shell-out (hardened, streamed) ───────────────────────────────

    /// <summary>Spawn <c>bash install.sh --deploy &lt;file&gt;</c>, stream both
    /// pipes into <paramref name="output"/>, wait for exit. Returns the exit
    /// code; a timeout tree-kills the tree and returns a non-zero code.
    /// <paramref name="extraEnv"/> (the staged GCP credential env, #833) is
    /// layered over the inherited environment.</summary>
    private async Task<int?> ShellInstallAsync(
        Guid deploymentId,
        string installSh,
        string deployFile,
        DeployOutput output,
        IReadOnlyDictionary<string, string>? extraEnv,
        CancellationToken ct)
    {
        var psi = new ProcessStartInfo
        {
            FileName = "bash",
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            RedirectStandardInput = true, // nulled below — never wait on stdin in a pipe
            UseShellExecute = false,
            CreateNoWindow = true,
        };
        psi.ArgumentList.Add(installSh);
        psi.ArgumentList.Add("--deploy");
        psi.ArgumentList.Add(deployFile);
        if (extraEnv is not null)
        {
            foreach (var (k, v) in extraEnv)
            {
                psi.Environment[k] = v;
            }
        }

        var deployTimeout = DeployTimeoutFor(await File.ReadAllTextAsync(deployFile, ct).ConfigureAwait(false));
        using var timeoutCts = CancellationTokenSource.CreateLinkedTokenSource(ct);
        timeoutCts.CancelAfter(deployTimeout);
        var runCt = timeoutCts.Token;

        // Budget re-anchor (issue #817): when install.sh reports the install
        // phase actually starting (first reference-API install step — i.e. the
        // VM exists and SSH answers), re-arm the timer with the FULL scaled
        // budget from that moment, so quota/provisioning contention ahead of
        // the install can't eat the install's own budget. Also re-stamp
        // deployment.started_at (best-effort) so the watchdog's stale-deploy
        // sweep ages from the same anchor and can never undercut this timer.
        var spawnedAt = DateTime.UtcNow;
        output.OnInstallPhaseStarted = () =>
        {
            try
            {
                timeoutCts.CancelAfter(deployTimeout);
            }
            catch (ObjectDisposedException)
            {
                // The deploy already finished/timed out — the late marker line
                // was drained after the fact; nothing to re-arm.
                return;
            }
            _logger.LogInformation(
                "Deployment {DeploymentId}: install phase started — {Budget:0}m budget re-anchored (provisioning took {Elapsed:0}m)",
                deploymentId, deployTimeout.TotalMinutes, (DateTime.UtcNow - spawnedAt).TotalMinutes);
            _ = StampInstallStartAsync(deploymentId);
        };

        using var process = new Process { StartInfo = psi };
        process.Start();
        process.StandardInput.Close(); // stdin protection (curl|bash-safe, like Rust's Stdio::null)

        // Drain both streams line-by-line off the LIVE pipes so log lines reach
        // the browser as they're produced (not buffered until exit). Each task
        // tags its origin stream, matching the Rust merged-mpsc-with-tag design.
        var stdoutTask = PumpAsync(process.StandardOutput, deploymentId, output, "stdout", runCt);
        var stderrTask = PumpAsync(process.StandardError, deploymentId, output, "stderr", runCt);

        // Incremental log persistence: deployment.log used to be written only
        // at terminal state, so a page load mid-deploy showed 'running' with
        // an EMPTY log — all progress lived in the live event stream and was
        // lost on refresh (user-caught 2026-08-12, a 15-min 5-stack deploy
        // with nothing to look at). Flush the accumulated log every few
        // seconds; the event bus stays the low-latency path and FinishAsync
        // still writes the authoritative final log.
        using var flushCts = CancellationTokenSource.CreateLinkedTokenSource(runCt);
        var flusher = FlushLogPeriodicallyAsync(deploymentId, output, flushCts.Token);

        try
        {
            await process.WaitForExitAsync(runCt).ConfigureAwait(false);
            await Task.WhenAll(stdoutTask, stderrTask).ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (timeoutCts.IsCancellationRequested
                                                 && !ct.IsCancellationRequested)
        {
            KillTree(process);
            var msg = TimeoutMessageFor(deployTimeout, output.InstallPhaseStartedUtc is not null);
            _logger.LogWarning("{Message} (deployment {DeploymentId})", msg, deploymentId);
            output.AppendRaw(msg);
            return -1;
        }
        catch (OperationCanceledException)
        {
            KillTree(process); // caller cancelled — don't leave install.sh running
            throw;
        }
        finally
        {
            flushCts.Cancel();
            try
            {
                await flusher.ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                // normal: the flusher's timer wait observed the cancel
            }
        }

        return process.ExitCode;
    }

    /// <summary>Timeout-kill notice, honest about which anchor the budget aged
    /// from (#817): pre-install ("never started") means provisioning/queueing
    /// consumed the whole window; post-anchor means the install itself did.</summary>
    internal static string TimeoutMessageFor(TimeSpan deployTimeout, bool installPhaseStarted)
    {
        return installPhaseStarted
            ? string.Format(
                CultureInfo.InvariantCulture,
                "install.sh timed out {0:0}m after its install phase started and was killed",
                deployTimeout.TotalMinutes)
            : string.Format(
                CultureInfo.InvariantCulture,
                "install.sh timed out after {0:0}m (install phase never started — provisioning/queue time used the whole budget) and was killed",
                deployTimeout.TotalMinutes);
    }

    /// <summary>Best-effort re-stamp of <c>deployment.started_at</c> at the
    /// moment the install phase begins, so the watchdog's stale-deploy sweep
    /// (<c>started_at ?? created_at</c> basis) ages the SAME re-anchored budget
    /// the runner's timer enforces (#817). Guarded on status <c>running</c> —
    /// a deployment another writer already finished/reaped keeps its stamps.</summary>
    internal async Task StampInstallStartAsync(Guid deploymentId)
    {
        try
        {
            using var scope = _scopeFactory.CreateScope();
            var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
            await db.Deployments
                .Where(d => d.DeploymentId == deploymentId && d.Status == "running")
                .ExecuteUpdateAsync(s => s.SetProperty(d => d.StartedAt, DateTime.UtcNow))
                .ConfigureAwait(false);
        }
        catch (Exception ex)
        {
            // Aging just stays anchored at the running-flip — strictly the old
            // behaviour, never worse.
            _logger.LogDebug(ex, "install-start stamp failed for deployment {DeploymentId}", deploymentId);
        }
    }

    /// <summary>Persist the accumulated log every few seconds while install.sh
    /// runs, so the deployment detail page shows progress on LOAD instead of
    /// only over the live event stream. Skips ticks with no new output;
    /// best-effort (a failed flush is a debug log, never a deploy failure).</summary>
    private async Task FlushLogPeriodicallyAsync(Guid deploymentId, DeployOutput output, CancellationToken ct)
    {
        var lastLength = 0;
        try
        {
            using var timer = new PeriodicTimer(TimeSpan.FromSeconds(3));
            while (await timer.WaitForNextTickAsync(ct).ConfigureAwait(false))
            {
                var log = output.FullLog;
                if (log.Length == lastLength)
                {
                    continue;
                }
                lastLength = log.Length;
                try
                {
                    using var scope = _scopeFactory.CreateScope();
                    var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
                    await db.Deployments
                        .Where(d => d.DeploymentId == deploymentId)
                        .ExecuteUpdateAsync(s => s.SetProperty(d => d.Log, log), ct)
                        .ConfigureAwait(false);
                }
                catch (Exception ex) when (ex is not OperationCanceledException)
                {
                    _logger.LogDebug(ex, "Incremental log flush failed for {DeploymentId}", deploymentId);
                }
            }
        }
        catch (OperationCanceledException)
        {
            // normal shutdown — FinishAsync writes the authoritative final log
        }
    }

    private void PumpLine(Guid deploymentId, DeployOutput output, string line, string stream)
    {
        // process_line: accumulate for the full log + IP parse, then dedup-broadcast.
        // The broadcast carries the ANSI-scrubbed form — same content the log
        // persists, so live viewers and post-mortem readers see identical text.
        if (output.ProcessLine(line, stream, out var clean))
        {
            _bus.Publish(new DeployLog(deploymentId, clean, stream));
        }
    }

    private async Task PumpAsync(
        StreamReader reader, Guid deploymentId, DeployOutput output, string stream, CancellationToken ct)
    {
        while (await reader.ReadLineAsync(ct).ConfigureAwait(false) is { } line)
        {
            PumpLine(deploymentId, output, line, stream);
        }
    }

    // ── DB persistence (fresh scope — this runs on a background task) ─────────

    private async Task SetStatusAsync(Guid deploymentId, string status, CancellationToken ct)
    {
        using var scope = _scopeFactory.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
        // Both callers flip to `running` — stamp started_at (issue #764's
        // "`started_at` is null" observation; also the watchdog's stale-deploy
        // age basis, so a recovered re-run gets a fresh 30-minute window
        // instead of being reaped against the ORIGINAL attempt's created_at).
        var now = DateTime.UtcNow;
        await db.Deployments
            .Where(d => d.DeploymentId == deploymentId)
            .ExecuteUpdateAsync(s => s
                .SetProperty(d => d.Status, status)
                .SetProperty(d => d.StartedAt, now), ct)
            .ConfigureAwait(false);
    }

    /// <summary>Persist the terminal state (log + ips/error + status) and publish
    /// <see cref="DeployComplete"/>. Best-effort: a DB error here is logged, not
    /// thrown, so the completion event still fires.
    ///
    /// <para>The terminal-persist ExecuteUpdate runs under
    /// <see cref="CancellationToken.None"/>, NOT the caller's <c>ct</c>: this is
    /// cleanup that MUST complete even during shutdown. If it honoured a cancelled
    /// token the deployment could never be marked terminal and would wedge at
    /// <c>running</c> forever (quality audit F3(c)).</para></summary>
    private async Task FinishAsync(
        Guid deploymentId, bool success, IReadOnlyList<string> ips, string? log, string? error, CancellationToken ct,
        IReadOnlyList<string?>? hosts = null, int? exitCode = null, string? failedStep = null)
    {
        _ = ct; // terminal cleanup is intentionally not cancellable — see summary.
        var status = success ? "completed" : "failed";
        try
        {
            using var scope = _scopeFactory.CreateScope();
            var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();

            var ipsJson = System.Text.Json.JsonSerializer.Serialize(ips);
            // endpoint_hosts stays parallel to endpoint_ips (null where the
            // provider gave no DNS name); omitted entirely when nothing was recorded.
            var hostsJson = success && hosts is not null && hosts.Any(h => !string.IsNullOrEmpty(h))
                ? System.Text.Json.JsonSerializer.Serialize(
                    ips.Select((_, i) => i < hosts.Count && !string.IsNullOrEmpty(hosts[i]) ? hosts[i] : null).ToList())
                : null;
            var now = DateTime.UtcNow;

            if (success)
            {
                await db.Deployments
                    .Where(d => d.DeploymentId == deploymentId)
                    .ExecuteUpdateAsync(s => s
                        .SetProperty(d => d.Status, status)
                        .SetProperty(d => d.Log, log)
                        .SetProperty(d => d.EndpointIps, ipsJson)
                        .SetProperty(d => d.EndpointHosts, hostsJson)
                        .SetProperty(d => d.ErrorMessage, error)
                        .SetProperty(d => d.ExitCode, exitCode)
                        .SetProperty(d => d.FailedStep, (string?)null)
                        .SetProperty(d => d.FinishedAt, now), CancellationToken.None)
                    .ConfigureAwait(false);
            }
            else
            {
                // Failure: leave endpoint_ips/endpoint_hosts UNTOUCHED. This
                // used to null them, which was harmless on a first deploy (they
                // were already null) but destructive on an UPDATE re-run over a
                // live deployment: the still-serving endpoints vanished from the
                // deployed-targets/version panels, and DELETE's VM teardown lost
                // its reverse-lookup inputs — orphaning the VM to the reaper
                // (silent-update investigation, 2026-08-18).
                await db.Deployments
                    .Where(d => d.DeploymentId == deploymentId)
                    .ExecuteUpdateAsync(s => s
                        .SetProperty(d => d.Status, status)
                        .SetProperty(d => d.Log, log)
                        .SetProperty(d => d.ErrorMessage, error)
                        .SetProperty(d => d.ExitCode, exitCode)
                        .SetProperty(d => d.FailedStep, failedStep)
                        .SetProperty(d => d.FinishedAt, now), CancellationToken.None)
                    .ConfigureAwait(false);
            }
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Failed to persist terminal state for deployment {DeploymentId}", deploymentId);
        }

        _bus.Publish(new DeployComplete(deploymentId, status, ips));
    }

    // ── install.sh location (mirrors Rust find_install_sh) ───────────────────

    /// <summary>Locate <c>install.sh</c>: explicit <c>INSTALL_SH_PATH</c> override
    /// first, then the current directory and up to five parent directories, then
    /// relative to the assembly location's repo root. Returns null if not found —
    /// the caller soft-fails.</summary>
    internal static string? FindInstallSh()
    {
        if (Environment.GetEnvironmentVariable("INSTALL_SH_PATH") is { Length: > 0 } overridePath
            && File.Exists(overridePath))
        {
            return Path.GetFullPath(overridePath);
        }

        var dir = new DirectoryInfo(Directory.GetCurrentDirectory());
        for (var i = 0; i < 6 && dir is not null; i++)
        {
            var candidate = Path.Combine(dir.FullName, "install.sh");
            if (File.Exists(candidate))
            {
                return candidate;
            }
            dir = dir.Parent;
        }

        // Fall back to walking up from the assembly location (bin/Release/... → repo root).
        var asmDir = new DirectoryInfo(AppContext.BaseDirectory);
        for (var i = 0; i < 8 && asmDir is not null; i++)
        {
            var candidate = Path.Combine(asmDir.FullName, "install.sh");
            if (File.Exists(candidate))
            {
                return candidate;
            }
            asmDir = asmDir.Parent;
        }

        return null;
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
            // Best-effort — may have exited between the check and the kill.
        }
    }

    private static void TryDelete(string path)
    {
        try { File.Delete(path); } catch { /* best-effort temp cleanup */ }
    }

    // ── Output accumulator (port of Rust DeployOutput) ───────────────────────

    /// <summary>Accumulates the full log, dedups broadcast lines, and extracts
    /// endpoint hosts (FQDN preferred over bare IP). Ported from the Rust
    /// <c>DeployOutput</c> struct. Additionally records, per endpoint, the DNS
    /// name install.sh reports (<c>endpoint_host: name (ip)</c>, or the cloud
    /// FQDN it printed next to the ip) as <see cref="EndpointHosts"/> — kept
    /// index-parallel to <see cref="EndpointIps"/>.</summary>
    internal sealed class DeployOutput
    {
        // Bound on the ACCUMULATED log (chars ≈ bytes for this mostly-ASCII
        // output). The log is flushed to the deployment row every few seconds
        // and returned by the list/detail endpoints, so it must stay bounded —
        // an 8-language Windows deploy's SDK-download chatter is megabytes.
        // When the cap is hit the OLDEST lines are dropped: the diagnosis of a
        // failure lives in the tail (issue #816). Endpoint-IP parsing happens
        // per-line on arrival, so trimming the front never loses parsed hosts.
        internal const int MaxLogChars = 256 * 1024;

        // install.sh step header (print_step_header): "Step 3: Deploy endpoints".
        private static readonly Regex StepRe = new(@"^Step \d+: ", RegexOptions.Compiled);

        // One lock for all mutable state: the stdout and stderr pumps append
        // CONCURRENTLY (a latent race before the incremental flusher made it
        // load-bearing — StringBuilder is not thread-safe), and the flusher
        // reads FullLog while both are writing.
        private readonly object _sync = new();
        private readonly System.Text.StringBuilder _log = new();
        private readonly HashSet<string> _seen = new(StringComparer.Ordinal);
        private readonly List<string> _endpointIps = [];
        // Parallel to _endpointIps: the recorded DNS name for that entry, or null.
        private readonly List<string?> _endpointHosts = [];
        // First InstallPhaseMarkerRe sighting (issue #817's budget anchor).
        private DateTime? _installPhaseStartedUtc;

        /// <summary>Invoked ONCE (outside the lock), when the first
        /// <see cref="InstallPhaseMarkerRe"/> line is observed — i.e. the
        /// install phase actually started. The shell path re-arms its deploy
        /// budget and re-stamps <c>deployment.started_at</c> from it (#817).</summary>
        public Action? OnInstallPhaseStarted { get; set; }

        /// <summary>When the install phase started (first marker line), or null
        /// while still provisioning / for stack-only deploys.</summary>
        public DateTime? InstallPhaseStartedUtc
        {
            get
            {
                lock (_sync)
                {
                    return _installPhaseStartedUtc;
                }
            }
        }

        private bool _truncated;
        private string? _currentStep;
        private string? _lastErrorLine;

        public string FullLog
        {
            get
            {
                lock (_sync)
                {
                    return _truncated
                        ? $"[log truncated — older output dropped, showing the most recent {MaxLogChars / 1024} KB]\n" + _log
                        : _log.ToString();
                }
            }
        }

        /// <summary>The last <c>Step N: …</c> header install.sh printed — on a
        /// failed deploy, the step it died in. Null before the first step.</summary>
        public string? CurrentStep
        {
            get
            {
                lock (_sync)
                {
                    return _currentStep;
                }
            }
        }

        /// <summary>The last fatal-looking line seen (install.sh's
        /// <c>print_err</c> "✗ …" or the docker path's "ERROR: …"), without the
        /// marker — the first actionable detail for error messages.</summary>
        public string? LastErrorLine
        {
            get
            {
                lock (_sync)
                {
                    return _lastErrorLine;
                }
            }
        }

        public IReadOnlyList<string> EndpointIps
        {
            get
            {
                lock (_sync)
                {
                    return _endpointIps.ToArray();
                }
            }
        }

        /// <summary>Per-endpoint DNS names, index-parallel to <see cref="EndpointIps"/>
        /// (null where none was reported).</summary>
        public IReadOnlyList<string?> EndpointHosts
        {
            get
            {
                lock (_sync)
                {
                    return _endpointHosts.ToArray();
                }
            }
        }

        private void AddEndpoint(string addr, string? host)
        {
            _endpointIps.Add(addr);
            _endpointHosts.Add(host);
        }

        /// <summary>Process one output line: append to the full log, parse for a
        /// host, and report whether it should be broadcast (true = not a
        /// duplicate). Mirrors Rust <c>process_line</c>.</summary>
        public bool ProcessLine(string text, string stream) => ProcessLine(text, stream, out _);

        /// <summary>Same as <see cref="ProcessLine(string,string)"/>, additionally
        /// returning the ANSI-scrubbed form of the line — what was actually
        /// logged, and what broadcasters should publish.</summary>
        public bool ProcessLine(string text, string stream, out string clean)
        {
            bool broadcast;
            Action? installPhaseCallback = null;
            lock (_sync)
            {
                clean = AnsiText.Strip(text) ?? string.Empty;
                broadcast = ProcessLineLocked(clean, stream);
                if (_installPhaseStartedUtc is null && InstallPhaseMarkerRe.IsMatch(clean))
                {
                    _installPhaseStartedUtc = DateTime.UtcNow;
                    // Grab-and-clear under the lock so the callback fires exactly
                    // once even with stdout/stderr pumping concurrently; invoke
                    // OUTSIDE the lock (it re-arms a CTS and touches the DB).
                    installPhaseCallback = OnInstallPhaseStarted;
                    OnInstallPhaseStarted = null;
                }
            }
            installPhaseCallback?.Invoke();
            return broadcast;
        }

        // `text` arrives ANSI-scrubbed (see ProcessLine): the installer's own
        // colors are TTY-gated off, but nested tools (az, cargo, apt, choco)
        // still emit escape codes, which used to land verbatim in
        // deployment.log (issue #816 ask 1).
        private bool ProcessLineLocked(string text, string stream)
        {
            AppendBounded(text);

            var stripped = text.Trim();
            if (StepRe.IsMatch(stripped))
            {
                _currentStep = stripped;
            }
            else if (stripped.StartsWith('✗'))
            {
                // install.sh print_err: "✗ <message>" (two-space indent trimmed).
                _lastErrorLine = stripped.TrimStart('✗').Trim();
            }
            else if (stripped.StartsWith("ERROR: ", StringComparison.Ordinal))
            {
                // The docker provider path's failure lines.
                _lastErrorLine = stripped["ERROR: ".Length..].Trim();
            }

            // Explicit per-endpoint hostname report: bind the DNS name to the
            // ip's entry (create it if this is the first mention). Does not
            // rewrite endpoint_ips — that column keeps its historical shape.
            var hostMatch = EndpointHostRe.Match(text);
            if (hostMatch.Success)
            {
                var name = hostMatch.Groups[1].Value;
                var ip = hostMatch.Groups[2].Value;
                var pos = _endpointIps.IndexOf(ip);
                if (pos < 0)
                {
                    pos = _endpointIps.IndexOf(name);
                }
                if (pos < 0)
                {
                    pos = _endpointHosts.IndexOf(name);
                }
                if (pos >= 0)
                {
                    _endpointHosts[pos] = name;
                }
                else
                {
                    AddEndpoint(ip, name);
                }
                _ = TrimmedBroadcast(text, out var bcast);
                return bcast;
            }

            // Prefer FQDN-with-IP; replace a previously-captured bare IP with the FQDN.
            var fqdnMatch = FqdnRe.Match(text);
            if (fqdnMatch.Success)
            {
                var fqdn = fqdnMatch.Groups[1].Value;
                var ip = fqdnMatch.Groups[2].Value;
                var pos = _endpointIps.IndexOf(ip);
                if (pos >= 0)
                {
                    _endpointIps[pos] = fqdn;
                    _endpointHosts[pos] = fqdn;
                }
                if (!_endpointIps.Contains(fqdn))
                {
                    AddEndpoint(fqdn, fqdn);
                }
            }
            else
            {
                var ipMatch = IpRe.Match(text);
                if (ipMatch.Success)
                {
                    var ip = ipMatch.Groups[1].Value;
                    if (!_endpointIps.Contains(ip))
                    {
                        AddEndpoint(ip, null);
                    }
                }
            }

            _ = stream; // origin tag is carried on the DeployLog, dedup is text-only
            _ = TrimmedBroadcast(text, out var broadcast);
            return broadcast;
        }

        private string TrimmedBroadcast(string text, out bool broadcast)
        {
            var trimmed = text.Trim();
            broadcast = trimmed.Length > 0 && _seen.Add(trimmed);
            return trimmed;
        }

        /// <summary>Fallback: if the structured regexes caught nothing, scan for
        /// bare IPs on lines mentioning endpoint/deployed/public ip, skipping
        /// loopback/0.* — mirrors the Rust post-exit fallback loop.</summary>
        public void RunFallbackIpScan()
        {
            lock (_sync)
            {
                RunFallbackIpScanLocked();
            }
        }

        private void RunFallbackIpScanLocked()
        {
            if (_endpointIps.Count > 0)
            {
                return;
            }

            foreach (var line in _log.ToString().Split('\n'))
            {
                var lower = line.ToLowerInvariant();
                if (!lower.Contains("endpoint") && !lower.Contains("deployed") && !lower.Contains("public ip"))
                {
                    continue;
                }
                foreach (Match m in BareIpRe.Matches(line))
                {
                    var ip = m.Groups[1].Value;
                    if (!ip.StartsWith("127.", StringComparison.Ordinal)
                        && !ip.StartsWith("0.", StringComparison.Ordinal)
                        && !_endpointIps.Contains(ip))
                    {
                        AddEndpoint(ip, null);
                    }
                }
            }
        }

        /// <summary>Append a synthetic line to the log without broadcasting (used
        /// for timeout/kill notices).</summary>
        public void AppendRaw(string text)
        {
            lock (_sync)
            {
                AppendBounded(AnsiText.Strip(text) ?? string.Empty);
            }
        }

        /// <summary>Append one line, then enforce <see cref="MaxLogChars"/> by
        /// dropping WHOLE lines from the front (the failure diagnosis lives in
        /// the tail). Callers hold <c>_sync</c>.</summary>
        private void AppendBounded(string text)
        {
            _log.Append(text).Append('\n');
            if (_log.Length <= MaxLogChars)
            {
                return;
            }
            var cut = _log.Length - MaxLogChars;
            // Advance to the end of the line straddling the cut so the kept
            // region starts on a line boundary.
            while (cut < _log.Length && _log[cut - 1] != '\n')
            {
                cut++;
            }
            _log.Remove(0, cut);
            _truncated = true;
        }
    }
}
