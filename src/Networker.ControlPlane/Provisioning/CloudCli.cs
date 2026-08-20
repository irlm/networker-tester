using System.Diagnostics;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// Outcome of one cloud-CLI invocation through <see cref="CloudCli.RunAsync"/>.
///
/// <para>The three failure modes are distinct on purpose — callers surface very
/// different messages for them: <see cref="Spawned"/> false means the binary
/// isn't there (<see cref="LaunchError"/> carries the actionable message),
/// <see cref="TimedOut"/> means it was killed after the caller's budget, and a
/// non-zero <see cref="ExitCode"/> is the CLI's own refusal (auth, quota,
/// permissions) with the reason on <see cref="StdErr"/>.</para>
/// </summary>
public readonly record struct CloudCliResult(
    bool Spawned,
    int ExitCode,
    string StdOut,
    string StdErr,
    string? LaunchError,
    bool TimedOut)
{
    /// <summary>The CLI ran to completion and reported success.</summary>
    public bool Success => Spawned && !TimedOut && ExitCode == 0;
}

/// <summary>
/// Cloud-CLI + home-directory resolution for the provisioning shell-outs
/// (fidelity audit F3/F12).
///
/// <para><b>Binary resolution:</b> every cloud CLI honours an env-var override
/// — <c>AZ_CMD</c> (the Rust-era <c>az_bin()</c> shim), and, symmetrically,
/// <c>AWS_CMD</c> and <c>GCLOUD_CMD</c>. Overrides matter beyond dev shims:
/// under the systemd unit a snap-installed <c>gcloud</c> lives in
/// <c>/snap/bin</c>, which is not on systemd's default PATH. When a CLI fails
/// to launch, <see cref="LaunchFailureMessage"/> names the binary AND the
/// override var so the soft-fail is diagnosable instead of silent.</para>
///
/// <para><b>Home resolution:</b> systemd system units do not reliably export
/// <c>$HOME</c>. The old <c>GetEnvironmentVariable("HOME") ?? ""</c> pattern
/// degraded to the *relative* path <c>.ssh/id_rsa.pub</c> under cwd
/// <c>/</c> — silently skipping AWS key-pair import, GCP ssh-keys metadata,
/// and emitting a false <c>gcp_no_local_ssh_key</c> precheck warning.
/// <see cref="HomeDirectory()"/> falls back to the passwd-backed user profile
/// (<see cref="Environment.SpecialFolder.UserProfile"/>) and finally
/// <c>/root</c>.</para>
/// </summary>
public static class CloudCli
{
    /// <summary>Env var overriding the <c>az</c> binary path (Rust parity).</summary>
    public const string AzOverrideVar = "AZ_CMD";

    /// <summary>Env var overriding the <c>aws</c> binary path.</summary>
    public const string AwsOverrideVar = "AWS_CMD";

    /// <summary>Env var overriding the <c>gcloud</c> binary path.</summary>
    public const string GcloudOverrideVar = "GCLOUD_CMD";

    /// <summary>Resolve the Azure CLI binary (<c>AZ_CMD</c> override, else <c>az</c>).</summary>
    public static string AzBin() => Resolve("az", AzOverrideVar, Environment.GetEnvironmentVariable);

    /// <summary>Resolve the AWS CLI binary (<c>AWS_CMD</c> override, else <c>aws</c>).</summary>
    public static string AwsBin() => Resolve("aws", AwsOverrideVar, Environment.GetEnvironmentVariable);

    /// <summary>Resolve the gcloud CLI binary (<c>GCLOUD_CMD</c> override, else <c>gcloud</c>).</summary>
    public static string GcloudBin() => Resolve("gcloud", GcloudOverrideVar, Environment.GetEnvironmentVariable);

    /// <summary>Testable core of the Bin() resolvers. On Windows the cloud CLIs
    /// ship as <c>az.cmd</c> / <c>aws.cmd</c> / <c>gcloud.cmd</c>; with
    /// <c>UseShellExecute=false</c> the bare name is not resolved through
    /// PATHEXT, so every provisioning/reaper shell-out soft-failed with
    /// SpawnError on a Windows dev box unless the override var was set.</summary>
    internal static string Resolve(string defaultName, string overrideVar, Func<string, string?> getEnv) =>
        Resolve(defaultName, overrideVar, getEnv, OperatingSystem.IsWindows());

    internal static string Resolve(string defaultName, string overrideVar, Func<string, string?> getEnv, bool isWindows) =>
        getEnv(overrideVar) is { Length: > 0 } o ? o
        : isWindows ? defaultName + ".cmd"
        : defaultName;

    /// <summary>
    /// The override env var for a (possibly already overridden) CLI file name,
    /// or null for a binary this class doesn't own. Matches on the file's base
    /// name so absolute override paths (<c>/snap/bin/gcloud</c>) still map.
    /// </summary>
    public static string? OverrideVarFor(string file) =>
        Path.GetFileNameWithoutExtension(file) switch
        {
            "az" => AzOverrideVar,
            "aws" => AwsOverrideVar,
            "gcloud" => GcloudOverrideVar,
            _ => null,
        };

    /// <summary>
    /// Human-actionable message for a CLI that failed to launch: names the
    /// binary, the failure, and the env var that overrides its path — audit
    /// F12's "no silent soft-fail" contract.
    /// </summary>
    public static string LaunchFailureMessage(string file, string reason)
    {
        var hint = OverrideVarFor(file) is { } overrideVar
            ? $" Install it on the control-plane host and ensure it is on the service's PATH " +
              $"(snap installs live in /snap/bin — see deploy/alethedash-cs.service), " +
              $"or set {overrideVar} to its absolute path."
            : string.Empty;
        return $"failed to launch '{file}': {reason}.{hint}";
    }

    /// <summary>
    /// Spawn a cloud CLI and collect its output — the ONE hardened process
    /// runner behind every az/aws/gcloud shell-out in the control plane
    /// (<see cref="CliComputeProvisioner"/>, the orphan reaper, the inventory
    /// scan), so the deadlock/timeout/kill semantics can't drift between them.
    ///
    /// <para>Hardening (ported from <c>Networker.Agent.ProbeRunner</c>): both
    /// streams are drained concurrently and awaited after exit (no pipe-buffer
    /// deadlock), <c>UseShellExecute=false</c> + <c>CreateNoWindow=true</c>, and
    /// a hard <paramref name="timeout"/> that kills the whole process tree — a
    /// cloud CLI that hangs on a network call must never pin a request or a
    /// background sweep.</para>
    ///
    /// <para>Total for infrastructure failure: a missing binary comes back as
    /// <see cref="CloudCliResult.Spawned"/> false with a
    /// <see cref="CloudCliResult.LaunchError"/>, a timeout as
    /// <see cref="CloudCliResult.TimedOut"/>. The only exception it throws is
    /// <see cref="OperationCanceledException"/> when
    /// <paramref name="cancellationToken"/> itself is cancelled (caller went
    /// away) — the child is killed first.</para>
    ///
    /// <para><b>Never</b> log <paramref name="args"/> without checking whether
    /// the command carries a secret (<c>az login -p</c>); callers own that
    /// decision because only they know the shape of the command.</para>
    /// </summary>
    public static async Task<CloudCliResult> RunAsync(
        string file,
        IReadOnlyList<string> args,
        IReadOnlyDictionary<string, string>? env,
        TimeSpan timeout,
        CancellationToken cancellationToken)
    {
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
        if (env is not null)
        {
            foreach (var (k, v) in env)
            {
                psi.Environment[k] = v;
            }
        }

        using var timeoutCts = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeoutCts.CancelAfter(timeout);
        var ct = timeoutCts.Token;

        using var process = new Process { StartInfo = psi };
        try
        {
            process.Start();
        }
        catch (Exception ex)
        {
            // The common CI path: the cloud CLI isn't installed. A soft failure
            // whose message names the binary AND its override env var so it is
            // diagnosable instead of silent (audit F12).
            return new CloudCliResult(
                Spawned: false, ExitCode: -1, StdOut: string.Empty, StdErr: string.Empty,
                LaunchError: LaunchFailureMessage(file, ex.Message), TimedOut: false);
        }

        var stdoutTask = process.StandardOutput.ReadToEndAsync(ct);
        var stderrTask = process.StandardError.ReadToEndAsync(ct);
        try
        {
            await process.WaitForExitAsync(ct).ConfigureAwait(false);
            var stdout = (await stdoutTask.ConfigureAwait(false)).Trim();
            var stderr = (await stderrTask.ConfigureAwait(false)).Trim();
            return new CloudCliResult(true, process.ExitCode, stdout, stderr, LaunchError: null, TimedOut: false);
        }
        catch (OperationCanceledException) when (timeoutCts.IsCancellationRequested
                                                 && !cancellationToken.IsCancellationRequested)
        {
            KillTree(process);
            return new CloudCliResult(
                Spawned: true, ExitCode: -1, StdOut: string.Empty, StdErr: string.Empty,
                LaunchError: null, TimedOut: true);
        }
        catch (OperationCanceledException)
        {
            KillTree(process); // caller cancelled — don't leave the child running
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

    /// <summary>
    /// The service user's home directory: <c>$HOME</c> when set, else the
    /// passwd-backed <see cref="Environment.SpecialFolder.UserProfile"/>, else
    /// <c>/root</c> (non-Windows). Never returns empty on Unix, so callers
    /// building <c>~/.ssh</c> paths can't silently degrade to a relative path.
    /// </summary>
    public static string HomeDirectory() =>
        HomeDirectory(
            Environment.GetEnvironmentVariable,
            () => Environment.GetFolderPath(Environment.SpecialFolder.UserProfile));

    /// <summary>Testable core of <see cref="HomeDirectory()"/>.</summary>
    internal static string HomeDirectory(Func<string, string?> getEnv, Func<string?> getUserProfile)
    {
        if (getEnv("HOME") is { } home && !string.IsNullOrWhiteSpace(home))
        {
            return home;
        }

        if (getUserProfile() is { } profile && !string.IsNullOrWhiteSpace(profile))
        {
            return profile;
        }

        // Last resort for a systemd unit with no User= and a stripped
        // environment: root's home, not "" (which yields relative paths).
        return OperatingSystem.IsWindows() ? string.Empty : "/root";
    }
}
