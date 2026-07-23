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
///     "endpoint"/"deployed"/"public ip" lines.</item>
///   <item>On success: persist <c>endpoint_ips</c> + status <c>completed</c>.
///     On failure: persist <c>error_message</c> + status <c>failed</c>. Either
///     way persist the full log and publish a <see cref="DeployComplete"/>.</item>
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
    // timeout (it relies on install.sh's own guards); 30m is a safe C# backstop.
    private static readonly TimeSpan DeployTimeout = TimeSpan.FromMinutes(30);

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
        var error = success ? null : $"install.sh exited with code {exitCode ?? -1}";
        await FinishAsync(deploymentId, success, output.EndpointIps, output.FullLog, error, ct)
            .ConfigureAwait(false);

        TryDelete(deployFile);

        _logger.LogInformation(
            "Deployment {DeploymentId} finished status={Status} ips={Ips}",
            deploymentId, success ? "completed" : "failed", string.Join(",", output.EndpointIps));

        return output.EndpointIps;
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

        using var timeoutCts = CancellationTokenSource.CreateLinkedTokenSource(ct);
        timeoutCts.CancelAfter(DeployTimeout);
        var runCt = timeoutCts.Token;

        using var process = new Process { StartInfo = psi };
        process.Start();
        process.StandardInput.Close(); // stdin protection (curl|bash-safe, like Rust's Stdio::null)

        // Drain both streams line-by-line off the LIVE pipes so log lines reach
        // the browser as they're produced (not buffered until exit). Each task
        // tags its origin stream, matching the Rust merged-mpsc-with-tag design.
        var stdoutTask = PumpAsync(process.StandardOutput, deploymentId, output, "stdout", runCt);
        var stderrTask = PumpAsync(process.StandardError, deploymentId, output, "stderr", runCt);

        try
        {
            await process.WaitForExitAsync(runCt).ConfigureAwait(false);
            await Task.WhenAll(stdoutTask, stderrTask).ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (timeoutCts.IsCancellationRequested
                                                 && !ct.IsCancellationRequested)
        {
            KillTree(process);
            var msg = $"install.sh timed out after {DeployTimeout.TotalMinutes:0}m and was killed";
            _logger.LogWarning("{Message} (deployment {DeploymentId})", msg, deploymentId);
            output.AppendRaw(msg);
            return -1;
        }
        catch (OperationCanceledException)
        {
            KillTree(process); // caller cancelled — don't leave install.sh running
            throw;
        }

        return process.ExitCode;
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
        Guid deploymentId, bool success, IReadOnlyList<string> ips, string? log, string? error, CancellationToken ct)
    {
        _ = ct; // terminal cleanup is intentionally not cancellable — see summary.
        var status = success ? "completed" : "failed";
        try
        {
            using var scope = _scopeFactory.CreateScope();
            var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();

            var ipsJson = System.Text.Json.JsonSerializer.Serialize(ips);
            var now = DateTime.UtcNow;

            await db.Deployments
                .Where(d => d.DeploymentId == deploymentId)
                .ExecuteUpdateAsync(s => s
                    .SetProperty(d => d.Status, status)
                    .SetProperty(d => d.Log, log)
                    .SetProperty(d => d.EndpointIps, success ? ipsJson : (string?)null)
                    .SetProperty(d => d.ErrorMessage, error)
                    .SetProperty(d => d.FinishedAt, now), CancellationToken.None)
                .ConfigureAwait(false);
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
    /// <c>DeployOutput</c> struct.</summary>
    private sealed class DeployOutput
    {
        private readonly System.Text.StringBuilder _log = new();
        private readonly HashSet<string> _seen = new(StringComparer.Ordinal);
        private readonly List<string> _endpointIps = [];

        public string FullLog => _log.ToString();
        public IReadOnlyList<string> EndpointIps => _endpointIps;

        /// <summary>Process one output line: append to the full log, parse for a
        /// host, and report whether it should be broadcast (true = not a
        /// duplicate). Mirrors Rust <c>process_line</c>.</summary>
        public bool ProcessLine(string text, string stream)
        {
            _log.Append(text).Append('\n');

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
                }
                if (!_endpointIps.Contains(fqdn))
                {
                    _endpointIps.Add(fqdn);
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
                        _endpointIps.Add(ip);
                    }
                }
            }

            var trimmed = text.Trim();
            _ = stream; // origin tag is carried on the DeployLog, dedup is text-only
            return trimmed.Length > 0 && _seen.Add(trimmed);
        }

        /// <summary>Fallback: if the structured regexes caught nothing, scan for
        /// bare IPs on lines mentioning endpoint/deployed/public ip, skipping
        /// loopback/0.* — mirrors the Rust post-exit fallback loop.</summary>
        public void RunFallbackIpScan()
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
                        _endpointIps.Add(ip);
                    }
                }
            }
        }

        /// <summary>Append a synthetic line to the log without broadcasting (used
        /// for timeout/kill notices).</summary>
        public void AppendRaw(string text) => _log.Append(text).Append('\n');
    }
}
