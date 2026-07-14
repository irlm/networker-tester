using System.Diagnostics;
using System.Text;
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

        using var process = new Process { StartInfo = psi };
        var stdout = new StringBuilder();
        var stderr = new StringBuilder();
        process.OutputDataReceived += (_, e) => { if (e.Data is not null) stdout.AppendLine(e.Data); };
        process.ErrorDataReceived += (_, e) => { if (e.Data is not null) stderr.AppendLine(e.Data); };

        try
        {
            process.Start();
        }
        catch (Exception ex)
        {
            throw new ProbeRunnerException(
                $"failed to launch tester binary '{_options.TesterPath}': {ex.Message}", ex);
        }

        process.BeginOutputReadLine();
        process.BeginErrorReadLine();
        await process.WaitForExitAsync(cancellationToken).ConfigureAwait(false);

        if (process.ExitCode != 0)
        {
            throw new ProbeRunnerException(
                $"tester exited with code {process.ExitCode}: {stderr.ToString().Trim()}");
        }

        var json = stdout.ToString().Trim();
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
}

/// <summary>Raised when the tester process fails or emits unparsable output.</summary>
public sealed class ProbeRunnerException : Exception
{
    public ProbeRunnerException(string message) : base(message) { }
    public ProbeRunnerException(string message, Exception inner) : base(message, inner) { }
}
