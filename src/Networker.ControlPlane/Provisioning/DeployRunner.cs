using System.Diagnostics;
using System.Text.RegularExpressions;
using Microsoft.EntityFrameworkCore;
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
    private static readonly TimeSpan BaseDeployTimeout = TimeSpan.FromMinutes(30);
    private static readonly TimeSpan PerLanguageBudget = TimeSpan.FromMinutes(8);
    private static readonly TimeSpan MaxDeployTimeout = TimeSpan.FromMinutes(120);

    /// <summary>Workload-scaled install.sh budget: base + 8m per requested
    /// reference-API language across all endpoints, capped at 2h.</summary>
    internal static TimeSpan DeployTimeoutFor(string deployJson)
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
                }
            }
        }
        catch (System.Text.Json.JsonException)
        {
            // Unparseable config fails validation inside install.sh anyway.
        }
        var total = BaseDeployTimeout + languages * PerLanguageBudget;
        return total > MaxDeployTimeout ? MaxDeployTimeout : total;
    }

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

    public DeployRunner(
        IServiceScopeFactory scopeFactory,
        EventBus bus,
        ILogger<DeployRunner> logger)
    {
        _scopeFactory = scopeFactory;
        _bus = bus;
        _logger = logger;
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
            return await RunDockerDeploymentAsync(deploymentId, dockerPlan, ct).ConfigureAwait(false);
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
        int? exitCode;
        try
        {
            exitCode = await ShellInstallAsync(deploymentId, installSh, deployFile, output, ct)
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
        // 143 = SIGTERM: install.sh is a CHILD of this process, so a control-plane
        // restart (every deploy) kills any in-flight deployment — prod produced a
        // bare "install.sh exited with code 143" plus an orphan Azure VM, with no
        // hint that the cause was a restart rather than the customer's config
        // (prod sweep, v0.28.213). Say so, and point at the retry.
        var interrupted = exitCode is 143 or 137;
        var error = success
            ? null
            : interrupted
                ? $"Deployment interrupted (install.sh received {(exitCode == 137 ? "SIGKILL" : "SIGTERM")}, exit {exitCode}) — "
                  + "the control plane restarted or was shut down while deploying. Any VM it had already created is "
                  + "reaped by the orphan sweep; retry the deployment."
                : $"install.sh exited with code {exitCode ?? -1}";
        await FinishAsync(deploymentId, success, output.EndpointIps, output.FullLog, error, ct, output.EndpointHosts)
            .ConfigureAwait(false);

        TryDelete(deployFile);

        _logger.LogInformation(
            "Deployment {DeploymentId} finished status={Status} ips={Ips} hosts={Hosts}",
            deploymentId, success ? "completed" : "failed", string.Join(",", output.EndpointIps),
            string.Join(",", output.EndpointHosts.Select(h => h ?? "-")));

        return output.EndpointIps;
    }

    // ── Docker (local) provider ──────────────────────────────────────────────

    /// <summary>
    /// Start one target container per docker endpoint, gate each on its
    /// healthcheck, and finish the deployment exactly like the shell path
    /// (<c>completed</c> + <c>endpoint_ips</c> = container ips, log streamed via
    /// <see cref="DeployLog"/>). Any failure force-removes the containers this
    /// deployment already started (nothing to bill, but nothing to leak either)
    /// and records <c>failed</c>.
    /// </summary>
    private async Task<IReadOnlyList<string>> RunDockerDeploymentAsync(
        Guid deploymentId, DockerDeployPlan plan, CancellationToken ct)
    {
        var output = new DeployOutput();
        void Log(string line, string stream = "stdout")
        {
            if (output.ProcessLine(line, stream))
            {
                _bus.Publish(new DeployLog(deploymentId, line, stream));
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
            foreach (var ep in plan.Endpoints)
            {
                var image = docker.Options.TargetImageFor(ep.Stack);
                Log($"endpoints[{ep.Index}] {ep.Label}: docker run {image} (TARGET_STACK={DockerComputeProvisioner.TargetStackEnv(ep.Stack)})");
                var res = await docker.CreateTargetAsync(
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
    /// code; a timeout tree-kills the tree and returns a non-zero code.</summary>
    private async Task<int?> ShellInstallAsync(
        Guid deploymentId, string installSh, string deployFile, DeployOutput output, CancellationToken ct)
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

        var deployTimeout = DeployTimeoutFor(await File.ReadAllTextAsync(deployFile, ct).ConfigureAwait(false));
        using var timeoutCts = CancellationTokenSource.CreateLinkedTokenSource(ct);
        timeoutCts.CancelAfter(deployTimeout);
        var runCt = timeoutCts.Token;

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
            var msg = $"install.sh timed out after {deployTimeout.TotalMinutes:0}m and was killed";
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
        if (output.ProcessLine(line, stream))
        {
            _bus.Publish(new DeployLog(deploymentId, line, stream));
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
        await db.Deployments
            .Where(d => d.DeploymentId == deploymentId)
            .ExecuteUpdateAsync(s => s.SetProperty(d => d.Status, status), ct)
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
        IReadOnlyList<string?>? hosts = null)
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

        public string FullLog
        {
            get
            {
                lock (_sync)
                {
                    return _log.ToString();
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
        public bool ProcessLine(string text, string stream)
        {
            lock (_sync)
            {
                return ProcessLineLocked(text, stream);
            }
        }

        private bool ProcessLineLocked(string text, string stream)
        {
            _log.Append(text).Append('\n');

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
                _log.Append(text).Append('\n');
            }
        }
    }
}
