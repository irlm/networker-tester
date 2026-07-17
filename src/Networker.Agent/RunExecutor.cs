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
/// </summary>
public sealed class RunExecutor(ILogger<RunExecutor> logger, AgentOptions options)
{
    private const long MaxStdoutBytes = 128L * 1024 * 1024;

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
            sink.TrySend(new ErrorMessage(runId, msg));
            SendFinished(sink, runId, "failed", artifact: null);
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
            sink.TrySend(new ErrorMessage(runId, msg));
            SendFinished(sink, runId, "failed", artifact: null);
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
                sink.TrySend(new ErrorMessage(runId, msg));
                SendFinished(sink, runId, "failed", artifact: null);
                return;
            }

            foreach (var w in workloads)
                invocations.Add((w.Name, ApibenchWorkloads.BuildArgs(config, target, w)));
        }

        if (invocations.Count == 0)
        {
            const string msg = "No executable modes in workload";
            logger.LogError("{CorrelationId}: {Message}", correlationId, msg);
            sink.TrySend(new ErrorMessage(runId, msg));
            SendFinished(sink, runId, "failed", artifact: null);
            return;
        }

        // Locate tester binary ────────────────────────────────────────────────────
        var binPath = await TesterBinaryLocator.LocateAsync(options.TesterPath, cancellationToken)
            .ConfigureAwait(false);
        if (binPath is null)
        {
            const string msg = "networker-tester binary not found on this machine";
            logger.LogError("{CorrelationId}: {Message}", correlationId, msg);
            sink.TrySend(new ErrorMessage(runId, msg));
            SendFinished(sink, runId, "failed", artifact: null);
            return;
        }

        var successCount = 0u;
        var failureCount = 0u;

        foreach (var (workload, args) in invocations)
        {
            var outcome = await RunTesterOnceAsync(
                    binPath, args, workload, runId, correlationId, sink,
                    successCount, failureCount, cancellationToken)
                .ConfigureAwait(false);

            successCount = outcome.SuccessCount;
            failureCount = outcome.FailureCount;

            if (outcome.Status == InvocationStatus.Cancelled)
            {
                SendFinished(sink, runId, "cancelled", artifact: null);
                return;
            }
            if (outcome.Status == InvocationStatus.Failed)
            {
                SendFinished(sink, runId, "failed", artifact: null);
                return;
            }
        }

        sink.TrySend(new RunProgressMessage(runId, successCount, failureCount));

        var artifact = config.IsBenchmark
            ? BuildArtifact(config, successCount, failureCount)
            : null;

        SendFinished(sink, runId, "completed", artifact);
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
        CancellationToken cancellationToken)
    {
        var label = workload is null ? "tester" : $"tester/{workload}";
        logger.LogInformation(
            "{CorrelationId}: Spawning {Label} {Bin} {Args}",
            correlationId, label, binPath, string.Join(" ", args));

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
            sink.TrySend(new ErrorMessage(runId, msg));
            return new(InvocationStatus.Failed, successCount, failureCount);
        }

        process.StandardInput.Close(); // stdin = null

        // Stream stderr as [tester] error frames (best-effort).
        var stderrTask = Task.Run(async () =>
        {
            try
            {
                string? line;
                while ((line = await process.StandardError.ReadLineAsync(cancellationToken).ConfigureAwait(false)) is not null)
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

        try
        {
            string? line;
            while ((line = await process.StandardOutput.ReadLineAsync(cancellationToken).ConfigureAwait(false)) is not null)
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
            cancelled = true;
            logger.LogWarning("{CorrelationId}: Run cancelled — killing tester subprocess", correlationId);
            KillTree(process);
        }

        if (cancelled)
        {
            await WaitAndDrainAsync(process, stderrTask).ConfigureAwait(false);
            return new(InvocationStatus.Cancelled, successCount, failureCount);
        }

        if (overflow)
        {
            var msg = $"Tester stdout exceeded safety limit of {MaxStdoutBytes} bytes";
            logger.LogError("{CorrelationId}: {Message}", correlationId, msg);
            sink.TrySend(new ErrorMessage(runId, msg));
            await WaitAndDrainAsync(process, stderrTask).ConfigureAwait(false);
            return new(InvocationStatus.Failed, successCount, failureCount);
        }

        // stdout hit EOF → await exit + stderr drain.
        int exitCode;
        try
        {
            await process.WaitForExitAsync(cancellationToken).ConfigureAwait(false);
            exitCode = process.ExitCode;
        }
        catch (OperationCanceledException)
        {
            logger.LogWarning("{CorrelationId}: Run cancelled during wait — killing tester", correlationId);
            KillTree(process);
            await WaitAndDrainAsync(process, stderrTask).ConfigureAwait(false);
            return new(InvocationStatus.Cancelled, successCount, failureCount);
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
            sink.TrySend(new ErrorMessage(runId, msg));
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

    private static void SendFinished(
        RawWebSocketClient.IFrameSink sink, Guid runId, string status, BenchmarkArtifactPayload? artifact)
        => sink.TrySend(new RunFinishedMessage(runId, status, artifact));

    private static async Task WaitAndDrainAsync(Process process, Task stderrTask)
    {
        try { await process.WaitForExitAsync().ConfigureAwait(false); } catch { /* already gone */ }
        try { await stderrTask.ConfigureAwait(false); } catch { /* best-effort */ }
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
