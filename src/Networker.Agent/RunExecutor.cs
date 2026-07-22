using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;

namespace Networker.Agent;

/// <summary>
/// Executes an <c>assign_run</c> by shelling out to the <c>networker-tester</c>
/// binary and streaming results back — the C# port of the Rust
/// <c>executor::run_test</c> (crates/networker-agent/src/executor.rs).
///
/// Behavioural parity with the Rust executor, step for step:
///   1. Emit <c>run_started</c>.
///   2. Resolve <c>config.endpoint</c> → a target URL (Network only; Proxy /
///      Runtime / Pending are unsupported in the standalone agent → error +
///      failed).
///   3. Build the tester CLI args from the workload (modes, runs, concurrency,
///      timeout, --json-stdout, --insecure, --payload-sizes with the same
///      download/upload fallback to [65536]).
///   4. Locate + spawn the tester (stdout piped, stderr piped, stdin null,
///      kill-on-drop).
///   5. Stream stderr lines back as <c>error</c> frames ("[tester] {line}").
///   6. Read stdout into memory with a 128 MiB cap; on overflow kill + failed.
///   7. On exit: parse the final JSON <c>TestRun</c>. Success or non-zero-exit
///      but parseable → completed; unparseable → failed (+ error frame).
///   8. For each attempt emit <c>attempt_event</c>, tracking success/failure
///      counts, emitting <c>run_progress</c> every 10 attempts + a final one.
///   9. Synthesize the placeholder <c>BenchmarkArtifact</c> iff benchmark mode.
///  10. Emit <c>run_finished</c> with the terminal status + artifact.
///
/// Cancellation: the <see cref="CancellationToken"/> (fired by cancel_run /
/// shutdown / disconnect) kills the child and emits a <c>cancelled</c> terminal
/// status — the analogue of the Rust <c>cancel_rx</c> select arm + kill_on_drop.
///
/// Deadline (quality audit F4): every invocation additionally runs under an
/// overall wall-clock budget (<see cref="ComputeInvocationDeadline"/> — the
/// config's <c>max_duration_secs</c>, else worst-case workload arithmetic); on
/// expiry the tester process tree is killed and a <c>failed</c> terminal is
/// emitted, so a tester that hangs without EOF-ing stdout can never park a run
/// slot forever.
/// </summary>
public sealed class RunExecutor(ILogger<RunExecutor> logger, AgentOptions options)
{
    private const long MaxStdoutBytes = 128L * 1024 * 1024;

    /// <summary>Slack added on top of the computed per-invocation budget so a
    /// tester finishing right at its own timeout is never killed mid-flush.</summary>
    private const uint DeadlineSlackSecs = 60;

    /// <summary>Absolute ceiling on one tester invocation regardless of what the
    /// workload arithmetic (or a pathological <c>max_duration_secs</c>) yields.</summary>
    private static readonly TimeSpan MaxInvocationDeadline = TimeSpan.FromHours(24);

    /// <summary>Bound on the post-kill exit/drain wait: <see cref="KillTree"/>
    /// failures are swallowed, so an unkillable process must not hold one of
    /// the four run slots forever (quality audit F4).</summary>
    private static readonly TimeSpan PostKillGrace = TimeSpan.FromSeconds(10);

    /// <summary>Run one assigned execution to completion, streaming frames via
    /// <paramref name="sink"/>. Never throws — all failures become error +
    /// failed frames, matching the Rust executor which returns () on every
    /// path.</summary>
    public async Task ExecuteAsync(
        Guid runId,
        JsonElement configElement,
        RawWebSocketClient.IFrameSink sink,
        CancellationToken cancellationToken)
    {
        var correlationId = runId.ToString();

        // run_started ──────────────────────────────────────────────────────────
        sink.TrySend(new RunStartedMessage(runId, DateTimeOffset.UtcNow));

        TestConfigView config;
        try
        {
            config = TestConfigView.From(configElement);
        }
        catch (Exception ex)
        {
            var msg = $"Malformed assign_run config: {ex.Message}";
            logger.LogError("{CorrelationId}: {Message}", correlationId, msg);
            await SendFailureAsync(sink, runId, msg).ConfigureAwait(false);
            return;
        }

        logger.LogInformation(
            "{CorrelationId}: Run received config_id={ConfigId} endpoint_kind={Kind} modes=[{Modes}] is_benchmark={Bench}",
            correlationId, config.Id, config.EndpointKind, string.Join(",", config.Modes), config.IsBenchmark);

        // Resolve endpoint → target ──────────────────────────────────────────────
        var target = EndpointToTarget(config);
        if (target is null)
        {
            var msg = $"Unsupported endpoint kind for standalone agent: {config.EndpointKind}";
            logger.LogError("{CorrelationId}: {Message}", correlationId, msg);
            await SendFailureAsync(sink, runId, msg).ConfigureAwait(false);
            return;
        }

        // Build the invocation plan. "apibench" is a runner-level mode, not a
        // tester protocol (the tester would silently drop it from --modes):
        // the base invocation carries the remaining protocol modes, then one
        // tester invocation per apibench workload drives the measured /api/*
        // suite (audit C1).
        var apibenchRequested = config.Modes.Any(ApibenchWorkloads.IsApibenchMode);
        var invocations = new List<(string? Workload, List<string> Args)>();
        if (config.Modes.Any(m => !ApibenchWorkloads.IsApibenchMode(m)))
            invocations.Add((null, BuildArgs(config, target)));
        if (apibenchRequested)
        {
            IReadOnlyList<ApibenchWorkloads.Workload> workloads;
            try
            {
                workloads = ApibenchWorkloads.All;
            }
            catch (Exception ex)
            {
                var msg = $"apibench workload set failed to load: {ex.Message}";
                logger.LogError("{CorrelationId}: {Message}", correlationId, msg);
                await SendFailureAsync(sink, runId, msg).ConfigureAwait(false);
                return;
            }

            foreach (var w in workloads)
                invocations.Add((w.Name, ApibenchWorkloads.BuildArgs(config, target, w)));
        }

        if (invocations.Count == 0)
        {
            const string msg = "No executable modes in workload";
            logger.LogError("{CorrelationId}: {Message}", correlationId, msg);
            await SendFailureAsync(sink, runId, msg).ConfigureAwait(false);
            return;
        }

        // Locate tester binary ────────────────────────────────────────────────────
        var binPath = await TesterBinaryLocator.LocateAsync(options.TesterPath, cancellationToken)
            .ConfigureAwait(false);
        if (binPath is null)
        {
            const string msg = "networker-tester binary not found on this machine";
            logger.LogError("{CorrelationId}: {Message}", correlationId, msg);
            await SendFailureAsync(sink, runId, msg).ConfigureAwait(false);
            return;
        }

        var successCount = 0u;
        var failureCount = 0u;

        // Overall per-invocation wall-clock budget (audit F4): a tester that
        // wedges without EOF-ing stdout would otherwise park this task forever
        // and permanently consume one of the MaxConcurrentRuns slots.
        var invocationDeadline = ComputeInvocationDeadline(config);

        foreach (var (workload, args) in invocations)
        {
            var outcome = await RunTesterOnceAsync(
                    binPath, args, workload, runId, correlationId, sink,
                    successCount, failureCount, invocationDeadline, cancellationToken)
                .ConfigureAwait(false);

            successCount = outcome.SuccessCount;
            failureCount = outcome.FailureCount;

            if (outcome.Status == InvocationStatus.Cancelled)
            {
                await SendFinishedAsync(sink, runId, "cancelled", artifact: null).ConfigureAwait(false);
                return;
            }
            if (outcome.Status == InvocationStatus.Failed)
            {
                await SendFinishedAsync(sink, runId, "failed", artifact: null).ConfigureAwait(false);
                return;
            }
        }

        sink.TrySend(new RunProgressMessage(runId, successCount, failureCount));

        var artifact = config.IsBenchmark
            ? BuildArtifact(config, successCount, failureCount)
            : null;

        await SendFinishedAsync(sink, runId, "completed", artifact).ConfigureAwait(false);
    }

    /// <summary>
    /// Overall wall-clock budget for ONE tester invocation (quality audit F4).
    /// Prefers the config's own <c>max_duration_secs</c> when set; otherwise a
    /// generous worst case derived from the workload — every request in every
    /// mode timing out, fully serial: <c>timeout × runs × modes</c> — plus
    /// <see cref="DeadlineSlackSecs"/>, clamped to
    /// <see cref="MaxInvocationDeadline"/>. On expiry the tester process TREE is
    /// killed and the run reports a <c>failed</c> terminal — the workload
    /// <c>timeout_ms</c> alone only bounds individual requests (it becomes the
    /// tester's per-request <c>--timeout</c>), not a tester that hangs without
    /// EOF-ing stdout.
    /// </summary>
    internal static TimeSpan ComputeInvocationDeadline(TestConfigView config)
    {
        double totalSecs;
        if (config.MaxDurationSecs > 0)
        {
            totalSecs = (double)config.MaxDurationSecs + DeadlineSlackSecs;
        }
        else
        {
            // Same rounding as BuildArgs: timeout_ms.div_ceil(1000).max(1).
            var timeoutSecs = Math.Max(1u, (config.TimeoutMs + 999) / 1000);
            var runs = Math.Max(1u, config.Runs);
            var modes = (uint)Math.Max(1, config.Modes.Count);
            totalSecs = (double)timeoutSecs * runs * modes + DeadlineSlackSecs;
        }

        var deadline = TimeSpan.FromSeconds(totalSecs);
        return deadline <= MaxInvocationDeadline ? deadline : MaxInvocationDeadline;
    }

    /// <summary>Terminal failure pair: the reason as an <c>error</c> frame plus
    /// the <c>failed</c> <c>run_finished</c> — both via the critical
    /// (non-droppable) send path, because losing either strands the run
    /// <c>running</c> server-side until the watchdog (quality audit F2).</summary>
    private static async Task SendFailureAsync(
        RawWebSocketClient.IFrameSink sink, Guid runId, string message)
    {
        await sink.TrySendCriticalAsync(new ErrorMessage(runId, message)).ConfigureAwait(false);
        await SendFinishedAsync(sink, runId, "failed", artifact: null).ConfigureAwait(false);
    }

    private enum InvocationStatus { Completed, Failed, Cancelled }

    private readonly record struct InvocationOutcome(
        InvocationStatus Status, uint SuccessCount, uint FailureCount);

    /// <summary>
    /// Spawn one tester process and stream its output — the single-invocation
    /// body of the original executor, extracted verbatim so a run can consist
    /// of several invocations (base modes + one per apibench workload).
    /// Success/failure counts accumulate across invocations via the
    /// <paramref name="successCount"/>/<paramref name="failureCount"/> seeds.
    /// </summary>
    private async Task<InvocationOutcome> RunTesterOnceAsync(
        string binPath,
        List<string> args,
        string? workload,
        Guid runId,
        string correlationId,
        RawWebSocketClient.IFrameSink sink,
        uint successCount,
        uint failureCount,
        TimeSpan invocationDeadline,
        CancellationToken cancellationToken)
    {
        var label = workload is null ? "tester" : $"tester/{workload}";
        logger.LogInformation(
            "{CorrelationId}: Spawning {Label} {Bin} {Args} (deadline {Deadline})",
            correlationId, label, binPath, string.Join(" ", RedactSecretArgs(args)), invocationDeadline);

        // Linked CTS = caller cancellation (cancel_run/shutdown/disconnect) +
        // the overall invocation deadline (audit F4). Every await below uses
        // this token; when it fires we distinguish the two causes via
        // cancellationToken.IsCancellationRequested.
        using var deadlineCts = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        deadlineCts.CancelAfter(invocationDeadline);
        var invocationToken = deadlineCts.Token;

        var psi = new ProcessStartInfo
        {
            FileName = binPath,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            RedirectStandardInput = true, // closed immediately → stdin null (Rust: Stdio::null)
            UseShellExecute = false,
            CreateNoWindow = true,
        };
        foreach (var a in args)
            psi.ArgumentList.Add(a);

        using var process = new Process { StartInfo = psi };
        try
        {
            process.Start();
        }
        catch (Exception ex)
        {
            var msg = $"Failed to spawn tester: {ex.Message}";
            logger.LogError("{CorrelationId}: {Message}", correlationId, msg);
            await sink.TrySendCriticalAsync(new ErrorMessage(runId, msg)).ConfigureAwait(false);
            return new(InvocationStatus.Failed, successCount, failureCount);
        }

        process.StandardInput.Close(); // stdin = null

        // Stream stderr as [tester] error frames (best-effort).
        var stderrTask = Task.Run(async () =>
        {
            try
            {
                string? line;
                while ((line = await process.StandardError.ReadLineAsync(invocationToken).ConfigureAwait(false)) is not null)
                {
                    sink.TrySend(new ErrorMessage(runId, $"[{label}] {line}"));
                }
            }
            catch (OperationCanceledException) { /* cancelled */ }
            catch (Exception ex) { logger.LogTrace(ex, "stderr pump ended"); }
        }, CancellationToken.None);

        // Read stdout into memory with a hard cap.
        var stdoutBuilder = new StringBuilder();
        long stdoutBytes = 0;
        bool overflow = false;
        bool cancelled = false;
        bool deadlineExpired = false;

        try
        {
            string? line;
            while ((line = await process.StandardOutput.ReadLineAsync(invocationToken).ConfigureAwait(false)) is not null)
            {
                stdoutBytes += line.Length + 1;
                if (stdoutBytes > MaxStdoutBytes)
                {
                    overflow = true;
                    KillTree(process);
                    break;
                }
                if (stdoutBuilder.Length > 0)
                    stdoutBuilder.Append('\n');
                stdoutBuilder.Append(line);
            }
        }
        catch (OperationCanceledException)
        {
            if (cancellationToken.IsCancellationRequested)
            {
                cancelled = true;
                logger.LogWarning("{CorrelationId}: Run cancelled — killing tester subprocess", correlationId);
            }
            else
            {
                deadlineExpired = true; // only the invocation deadline can fire otherwise
                logger.LogError(
                    "{CorrelationId}: Tester exceeded the overall run deadline of {Deadline} — killing process tree",
                    correlationId, invocationDeadline);
            }
            KillTree(process);
        }

        if (cancelled)
        {
            await WaitAndDrainAsync(process, stderrTask).ConfigureAwait(false);
            return new(InvocationStatus.Cancelled, successCount, failureCount);
        }

        if (deadlineExpired)
        {
            var msg = $"Tester ({label}) exceeded the overall run deadline of {invocationDeadline} — killed";
            await sink.TrySendCriticalAsync(new ErrorMessage(runId, msg)).ConfigureAwait(false);
            await WaitAndDrainAsync(process, stderrTask).ConfigureAwait(false);
            return new(InvocationStatus.Failed, successCount, failureCount);
        }

        if (overflow)
        {
            var msg = $"Tester stdout exceeded safety limit of {MaxStdoutBytes} bytes";
            logger.LogError("{CorrelationId}: {Message}", correlationId, msg);
            await sink.TrySendCriticalAsync(new ErrorMessage(runId, msg)).ConfigureAwait(false);
            await WaitAndDrainAsync(process, stderrTask).ConfigureAwait(false);
            return new(InvocationStatus.Failed, successCount, failureCount);
        }

        // stdout hit EOF → await exit + stderr drain.
        int exitCode;
        try
        {
            await process.WaitForExitAsync(invocationToken).ConfigureAwait(false);
            exitCode = process.ExitCode;
        }
        catch (OperationCanceledException)
        {
            if (cancellationToken.IsCancellationRequested)
            {
                logger.LogWarning("{CorrelationId}: Run cancelled during wait — killing tester", correlationId);
                KillTree(process);
                await WaitAndDrainAsync(process, stderrTask).ConfigureAwait(false);
                return new(InvocationStatus.Cancelled, successCount, failureCount);
            }

            // Deadline expired between stdout EOF and process exit (audit F4).
            logger.LogError(
                "{CorrelationId}: Tester exceeded the overall run deadline of {Deadline} during exit wait — killing process tree",
                correlationId, invocationDeadline);
            KillTree(process);
            await sink.TrySendCriticalAsync(new ErrorMessage(
                runId, $"Tester ({label}) exceeded the overall run deadline of {invocationDeadline} — killed")).ConfigureAwait(false);
            await WaitAndDrainAsync(process, stderrTask).ConfigureAwait(false);
            return new(InvocationStatus.Failed, successCount, failureCount);
        }

        try { await stderrTask.ConfigureAwait(false); } catch { /* best-effort */ }

        var stdoutText = stdoutBuilder.ToString();

        // Parse the final TestRun JSON. success/non-zero-but-parseable → completed;
        // unparseable → failed (+ error frame). (Rust match on exit + parse.)
        JsonDocument? parsed = null;
        try
        {
            parsed = JsonDocument.Parse(stdoutText);
        }
        catch (JsonException parseErr)
        {
            var snippet = stdoutText.Length > 512 ? stdoutText[..512] : stdoutText;
            var msg = $"Tester ({label}) exited with code {exitCode} and unparseable JSON: {parseErr.Message} (stdout starts: {snippet})";
            logger.LogError("{CorrelationId}: {Message}", correlationId, msg);
            await sink.TrySendCriticalAsync(new ErrorMessage(runId, msg)).ConfigureAwait(false);
            return new(InvocationStatus.Failed, successCount, failureCount);
        }

        using (parsed)
        {
            var root = parsed.RootElement;
            // Stream per-attempt events + progress counts (every 10 + final).
            if (root.TryGetProperty("attempts", out var attempts) && attempts.ValueKind == JsonValueKind.Array)
            {
                foreach (var attempt in attempts.EnumerateArray())
                {
                    var ok = attempt.TryGetProperty("success", out var s) && s.ValueKind == JsonValueKind.True;
                    uint total;
                    if (ok)
                    {
                        successCount++;
                        total = successCount;
                    }
                    else
                    {
                        failureCount++;
                        total = failureCount;
                    }

                    sink.TrySend(new AttemptEventMessage(runId, attempt.Clone()));

                    if (total % 10 == 0)
                        sink.TrySend(new RunProgressMessage(runId, successCount, failureCount));
                }
            }
        }

        return new(InvocationStatus.Completed, successCount, failureCount);
    }

    // ── endpoint_to_target (Rust parity) ─────────────────────────────────────────
    internal static string? EndpointToTarget(TestConfigView config)
    {
        if (config.EndpointKind != "network" || config.Network is null)
            return null; // proxy / runtime / pending unsupported in standalone agent

        var host = config.Network.Host;
        if (host.StartsWith("http://", StringComparison.Ordinal) ||
            host.StartsWith("https://", StringComparison.Ordinal))
            return host;

        const string scheme = "https";
        return config.Network.Port is { } p
            ? $"{scheme}://{host}:{p}/health"
            : $"{scheme}://{host}/health";
    }

    // ── build_args (Rust parity) ─────────────────────────────────────────────────
    internal static List<string> BuildArgs(TestConfigView config, string target)
    {
        // "apibench" is a runner-level mode — never a tester --modes value
        // (the tester would silently drop it). Its workloads run as separate
        // invocations built by ApibenchWorkloads.BuildArgs.
        var modesCsv = string.Join(
            ",", config.Modes.Where(m => !ApibenchWorkloads.IsApibenchMode(m)));
        // timeout_ms.div_ceil(1000).max(1) — round up to whole seconds, floor 1.
        var timeoutSecs = Math.Max(1u, (config.TimeoutMs + 999) / 1000);

        var args = new List<string>
        {
            "--target", target,
            "--modes", modesCsv,
            "--runs", config.Runs.ToString(),
            "--concurrency", config.Concurrency.ToString(),
            "--timeout", timeoutSecs.ToString(),
            "--json-stdout",
        };

        if (config.Insecure)
            args.Add("--insecure");

        // sdkprobe mode: pass the decrypted LagHound token + optional route. The
        // token reaches here only via the control-plane wire workload (it is
        // never stored plaintext); the spawn log line REDACTS it (see the
        // masked-args log below).
        if (config.Modes.Any(m => string.Equals(m, "sdkprobe", StringComparison.OrdinalIgnoreCase)))
        {
            if (!string.IsNullOrEmpty(config.LagHoundToken))
            {
                args.Add("--laghound-token");
                args.Add(config.LagHoundToken);
            }
            if (!string.IsNullOrEmpty(config.LagHoundRoute))
            {
                args.Add("--laghound-route");
                args.Add(config.LagHoundRoute);
            }
        }

        // Download/Upload hard-require --payload-sizes; fall back to [65536] when
        // a throughput mode is selected but no sizes were supplied.
        var needsPayload = config.Modes.Any(m => m is "download" or "upload");
        var payloadSizes = config.PayloadSizes.Count == 0 && needsPayload
            ? new List<uint> { 65536 }
            : config.PayloadSizes.ToList();

        if (payloadSizes.Count > 0)
        {
            args.Add("--payload-sizes");
            args.Add(string.Join(",", payloadSizes));
        }

        return args;
    }

    /// <summary>
    /// The tester flags whose VALUE is a secret and must never appear in a log
    /// line. The value immediately following one of these flags is replaced with
    /// a mask before the spawn command is logged.
    /// </summary>
    private static readonly HashSet<string> SecretArgFlags =
        new(StringComparer.OrdinalIgnoreCase) { "--laghound-token", "--bearer-token" };

    private const string SecretArgMask = "***REDACTED***";

    /// <summary>
    /// Copy of <paramref name="args"/> with the value after any secret flag
    /// masked, so the spawn log can be safe to emit verbatim. Order-preserving;
    /// only the token VALUES are hidden, not the flag names.
    /// </summary>
    internal static IReadOnlyList<string> RedactSecretArgs(IReadOnlyList<string> args)
    {
        var redacted = new List<string>(args.Count);
        for (var i = 0; i < args.Count; i++)
        {
            redacted.Add(args[i]);
            if (SecretArgFlags.Contains(args[i]) && i + 1 < args.Count)
            {
                redacted.Add(SecretArgMask);
                i++; // skip the real value
            }
        }
        return redacted;
    }

    // ── Placeholder BenchmarkArtifact (Rust parity) ──────────────────────────────
    private static BenchmarkArtifactPayload BuildArtifact(
        TestConfigView config, uint successCount, uint failureCount)
    {
        JsonElement El(string json) => JsonDocument.Parse(json).RootElement.Clone();

        var clientOs = OperatingSystem.IsWindows() ? "windows"
            : OperatingSystem.IsMacOS() ? "macos"
            : OperatingSystem.IsLinux() ? "linux"
            : RuntimeInformation.OSDescription;

        var environment = El(JsonSerializer.Serialize(new Dictionary<string, string>
        {
            ["client_os"] = clientOs,
            ["client_version"] = AgentVersion.Current,
        }));

        var methodology = config.Methodology.ValueKind is JsonValueKind.Undefined or JsonValueKind.Null
            ? El("null")
            : config.Methodology.Clone();

        var summaries = El(JsonSerializer.Serialize(new Dictionary<string, uint>
        {
            ["success"] = successCount,
            ["failure"] = failureCount,
        }));

        var dataQuality = El("""
            {
              "noise_level": null,
              "publication_ready": false,
              "blockers": ["agent-side artifact synthesis is a placeholder pending Agent A/B"]
            }
            """);

        return new BenchmarkArtifactPayload(
            Environment: environment,
            Methodology: methodology,
            Launches: El("[]"),
            Cases: El("[]"),
            Samples: null,
            Summaries: summaries,
            DataQuality: dataQuality);
    }

    /// <summary>The terminal <c>run_finished</c> — always via the critical
    /// (non-droppable) send path: silently losing it leaves the control-plane
    /// run <c>running</c> until the watchdog fails it (quality audit F2).</summary>
    private static async Task SendFinishedAsync(
        RawWebSocketClient.IFrameSink sink, Guid runId, string status, BenchmarkArtifactPayload? artifact)
        => await sink.TrySendCriticalAsync(new RunFinishedMessage(runId, status, artifact)).ConfigureAwait(false);

    private static async Task WaitAndDrainAsync(Process process, Task stderrTask)
    {
        // Bounded (audit F4): KillTree failures are swallowed, so an unkillable
        // process (or a grandchild holding the stderr pipe open) must not park
        // this task — and its MaxConcurrentRuns slot — forever.
        using var grace = new CancellationTokenSource(PostKillGrace);
        try { await process.WaitForExitAsync(grace.Token).ConfigureAwait(false); } catch { /* already gone or unkillable */ }
        try { await stderrTask.WaitAsync(grace.Token).ConfigureAwait(false); } catch { /* best-effort */ }
    }

    private static void KillTree(Process process)
    {
        try
        {
            if (!process.HasExited)
                process.Kill(entireProcessTree: true);
        }
        catch
        {
            // Best-effort — may have exited between the check and the kill.
        }
    }
}
