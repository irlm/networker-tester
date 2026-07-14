using System.Diagnostics;
using System.Text.Json;
using Microsoft.Extensions.Options;
using Networker.Contracts;

namespace Networker.Agent;

/// <summary>
/// Runs the Rust <c>networker-tester</c> binary as a child process with
/// <c>--json-stdout</c>, captures its stdout, and deserializes it into a
/// <see cref="ProbeRunResult"/> using the frozen JSON contract.
///
/// This is the proof-of-seam: the C# app layer never links Rust; it consumes
/// the versioned JSON the probe core emits over a process boundary.
/// </summary>
public sealed class ProbeRunner(ILogger<ProbeRunner> logger, IOptions<AgentOptions> options)
{
    private readonly AgentOptions _options = options.Value;

    public async Task<ProbeRunResult> RunAsync(
        string target,
        CancellationToken cancellationToken = default)
    {
        var psi = new ProcessStartInfo
        {
            FileName = _options.TesterPath,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true,
        };
        psi.ArgumentList.Add("--target");
        psi.ArgumentList.Add(target);
        psi.ArgumentList.Add("--modes");
        psi.ArgumentList.Add(_options.Modes);
        psi.ArgumentList.Add("--runs");
        psi.ArgumentList.Add("1");
        psi.ArgumentList.Add("--timeout");
        psi.ArgumentList.Add(_options.TimeoutSeconds.ToString());
        psi.ArgumentList.Add("--json-stdout");

        logger.LogInformation(
            "Spawning {Tester} --target {Target} --modes {Modes} --json-stdout",
            _options.TesterPath, target, _options.Modes);

        // Hard ceiling so a hung tester can never wedge the agent: the run's own
        // timeout plus a grace window. If it trips we kill the whole process
        // tree rather than leak an orphaned tester.
        using var timeoutCts = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        var hardTimeout = TimeSpan.FromSeconds(_options.TimeoutSeconds + 10);
        timeoutCts.CancelAfter(hardTimeout);
        var ct = timeoutCts.Token;

        using var process = new Process { StartInfo = psi };
        try
        {
            process.Start();
        }
        catch (Exception ex)
        {
            throw new ProbeRunnerException(
                $"failed to launch tester binary '{_options.TesterPath}': {ex.Message}", ex);
        }

        // Drain both streams concurrently and await them AFTER exit. This
        // avoids the pipe-buffer deadlock (a full stderr blocking stdout) and
        // the flush race the event-based BeginOutputReadLine model has, where
        // WaitForExitAsync can return before the final data callback fires.
        var stdoutTask = process.StandardOutput.ReadToEndAsync(ct);
        var stderrTask = process.StandardError.ReadToEndAsync(ct);

        string json, stderr;
        try
        {
            await process.WaitForExitAsync(ct).ConfigureAwait(false);
            json = (await stdoutTask.ConfigureAwait(false)).Trim();
            stderr = (await stderrTask.ConfigureAwait(false)).Trim();
        }
        catch (OperationCanceledException) when (timeoutCts.IsCancellationRequested
                                                 && !cancellationToken.IsCancellationRequested)
        {
            KillTree(process);
            throw new ProbeRunnerException(
                $"tester timed out after {hardTimeout.TotalSeconds:0}s and was killed");
        }
        catch (OperationCanceledException)
        {
            KillTree(process); // caller cancelled — don't leave the child running
            throw;
        }

        if (process.ExitCode != 0)
        {
            throw new ProbeRunnerException(
                $"tester exited with code {process.ExitCode}: {stderr}");
        }

        if (json.Length == 0)
        {
            throw new ProbeRunnerException("tester produced no stdout");
        }

        try
        {
            // The tester emits a single TestRun object for a single target.
            var result = JsonSerializer.Deserialize(
                json, ProbeContractJsonContext.Default.ProbeRunResult);
            return result ?? throw new ProbeRunnerException("tester JSON deserialized to null");
        }
        catch (JsonException ex)
        {
            throw new ProbeRunnerException(
                $"could not parse tester JSON against contract: {ex.Message}", ex);
        }
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
            // Best-effort — the process may have exited between the check and
            // the kill, or we may lack permission; nothing more we can do.
        }
    }
}

/// <summary>Raised when the tester process fails or emits unparsable output.</summary>
public sealed class ProbeRunnerException : Exception
{
    public ProbeRunnerException(string message) : base(message) { }
    public ProbeRunnerException(string message, Exception inner) : base(message, inner) { }
}
