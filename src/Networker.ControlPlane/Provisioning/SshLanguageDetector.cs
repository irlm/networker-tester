using System.Diagnostics;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// SSH probe that detects which benchmark language runtimes are deployed on a
/// catalog VM — the C# port of the Rust dashboard's
/// <c>ssh_detect_languages</c> (<c>api/benchmark_catalog.rs:227-292</c>).
///
/// <para>Fidelity notes (Rust parity):</para>
/// <list type="bullet">
///   <item>Same probe commands, same order: one <c>test -f</c> per known
///   language install path under <c>/opt/bench</c>, then a single
///   <c>ls -d /opt/bench/csharp-net*</c> sweep for the C# runtime ladder.</item>
///   <item>Same ssh options: <c>StrictHostKeyChecking=no</c>,
///   <c>ConnectTimeout=10</c>, <c>BatchMode=yes</c> — key auth only, using the
///   dashboard host's default identity (<c>~/.ssh</c>), exactly like the Rust
///   <c>tokio::process::Command::new("ssh")</c> call. No password prompts.</item>
///   <item>Probes run sequentially (the Rust loop awaited each command); an
///   unreachable host therefore costs up to ~10s per probe, and a probe error
///   is indistinguishable from "not installed" (Rust ignored spawn errors and
///   non-zero exits alike).</item>
/// </list>
/// </summary>
public interface ISshLanguageDetector
{
    /// <summary>Returns the detected language ids (empty when unreachable).</summary>
    Task<IReadOnlyList<string>> DetectAsync(string ip, string sshUser, CancellationToken ct);
}

public sealed class SshLanguageDetector(ILogger<SshLanguageDetector> logger) : ISshLanguageDetector
{
    /// <summary>
    /// Per-language existence probes — byte-for-byte the Rust check list.
    /// </summary>
    internal static readonly IReadOnlyList<(string Language, string Command)> LanguageChecks =
    [
        ("rust", "test -f /opt/bench/rust-server"),
        ("go", "test -f /opt/bench/go-server"),
        ("cpp", "test -f /opt/bench/cpp-build/server"),
        ("nodejs", "test -f /opt/bench/nodejs/server.js"),
        ("python", "test -f /opt/bench/python/server.py"),
        ("ruby", "test -f /opt/bench/ruby/config.ru"),
        ("php", "test -f /opt/bench/php/server.php"),
        ("java", "test -f /opt/bench/java/server.jar"),
        ("nginx", "which nginx > /dev/null 2>&1"),
    ];

    /// <summary>C# runtime-ladder sweep — one probe lists every csharp-net* dir.</summary>
    internal const string CsharpProbeCommand =
        "ls -d /opt/bench/csharp-net* 2>/dev/null | sed 's|/opt/bench/||'";

    // Rust: -o ConnectTimeout=10. Overall guard so a wedged ssh (e.g. host
    // answering TCP but stalling auth) can't hang the request forever.
    private static readonly TimeSpan ProbeTimeout = TimeSpan.FromSeconds(30);

    public async Task<IReadOnlyList<string>> DetectAsync(
        string ip, string sshUser, CancellationToken ct)
    {
        var detected = new List<string>();

        foreach (var (language, command) in LanguageChecks)
        {
            var result = await RunSshAsync(sshUser, ip, command, ct).ConfigureAwait(false);
            if (result is { ExitCode: 0 })
            {
                detected.Add(language);
            }
        }

        var csharp = await RunSshAsync(sshUser, ip, CsharpProbeCommand, ct).ConfigureAwait(false);
        if (csharp is { ExitCode: 0 })
        {
            detected.AddRange(ParseCsharpVariants(csharp.Value.Stdout));
        }

        return detected;
    }

    /// <summary>
    /// ssh argument vector — mirrors the Rust <c>.args([...])</c> exactly.
    /// ArgumentList (no shell) means no quoting/injection concerns on this side;
    /// the remote command strings are compile-time constants.
    /// </summary>
    internal static string[] BuildSshArgs(string sshUser, string ip, string command) =>
    [
        "-o", "StrictHostKeyChecking=no",
        "-o", "ConnectTimeout=10",
        "-o", "BatchMode=yes",
        $"{sshUser}@{ip}",
        command,
    ];

    /// <summary>
    /// Parses the csharp sweep output: one dir name per line; keep trimmed,
    /// non-empty lines that start with "csharp-net" (Rust lines() loop).
    /// </summary>
    internal static IReadOnlyList<string> ParseCsharpVariants(string stdout)
    {
        var variants = new List<string>();
        foreach (var line in stdout.Split('\n'))
        {
            var trimmed = line.Trim();
            if (trimmed.Length > 0 && trimmed.StartsWith("csharp-net", StringComparison.Ordinal))
            {
                variants.Add(trimmed);
            }
        }

        return variants;
    }

    /// <summary>
    /// Runs one ssh probe. Returns null on spawn failure or timeout — the Rust
    /// code treated any <c>Err</c> from <c>.output()</c> as "not detected".
    /// </summary>
    private async Task<(int ExitCode, string Stdout)?> RunSshAsync(
        string sshUser, string ip, string command, CancellationToken ct)
    {
        var psi = new ProcessStartInfo
        {
            FileName = "ssh",
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true,
        };
        foreach (var arg in BuildSshArgs(sshUser, ip, command))
        {
            psi.ArgumentList.Add(arg);
        }

        using var timeoutCts = CancellationTokenSource.CreateLinkedTokenSource(ct);
        timeoutCts.CancelAfter(ProbeTimeout);

        using var process = new Process { StartInfo = psi };
        try
        {
            process.Start();
        }
        catch (Exception ex)
        {
            // ssh binary missing or unlaunchable — same soft posture as the
            // provisioner CLI path (and as Rust's ignored Err).
            logger.LogWarning(ex, "Failed to spawn ssh for language probe of {Ip}", ip);
            return null;
        }

        // Drain both streams, await after exit (pipe-buffer deadlock guard —
        // same hardened pattern as CliComputeProvisioner.RunAsync).
        var stdoutTask = process.StandardOutput.ReadToEndAsync(timeoutCts.Token);
        var stderrTask = process.StandardError.ReadToEndAsync(timeoutCts.Token);
        try
        {
            await process.WaitForExitAsync(timeoutCts.Token).ConfigureAwait(false);
            var stdout = await stdoutTask.ConfigureAwait(false);
            _ = await stderrTask.ConfigureAwait(false);
            return (process.ExitCode, stdout);
        }
        catch (OperationCanceledException) when (!ct.IsCancellationRequested)
        {
            KillTree(process);
            logger.LogWarning(
                "ssh language probe of {Ip} timed out after {Seconds}s and was killed",
                ip, ProbeTimeout.TotalSeconds);
            return null;
        }
        catch (OperationCanceledException)
        {
            KillTree(process);
            throw;
        }
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
}
