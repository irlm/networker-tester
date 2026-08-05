using System.Diagnostics;
using Networker.ControlPlane.Provisioning;
using Xunit;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Audit P2: <c>ReinstallScript</c> is shipped to Azure VMs via
/// <c>vm run-command</c> and was only ever inspected — ASCII purity and a few
/// greps for expected substrings. Nothing ran it.
///
/// <para>That leaves the whole class of failures this script is most likely to
/// have: a bash syntax error, a renamed release asset, a tarball whose inner
/// filename changed. All of them look fine to a grep and all of them fail on a
/// customer's VM, mid-upgrade, with the agent already stopped. The installer
/// hit exactly this in v0.28.156 — the decommission renamed assets out from
/// under a download path and no test connected the two.</para>
///
/// <para><b>Two tiers.</b> The syntax check runs everywhere and is free. The
/// full execution is gated on <c>NETWORKER_REINSTALL_EXEC=1</c> because it
/// downloads real release assets and writes to <c>/usr/local/bin</c> — CI sets
/// it in a dedicated job that first installs a stub <c>networker-agent</c>
/// unit so the script's <c>systemctl restart</c> has something real to
/// restart.</para>
/// </summary>
public class ReinstallScriptExecutionTests
{
    private const string ExecEnvVar = "NETWORKER_REINSTALL_EXEC";

    /// <summary>The tag CI reinstalls. Pinned to a release known to carry both
    /// the tester and the C# agent assets, so a red result means the SCRIPT
    /// broke rather than that someone deleted an old release.</summary>
    private static string Tag =>
        Environment.GetEnvironmentVariable("NETWORKER_REINSTALL_TAG") ?? "v0.28.157";

    private static string Target => "x86_64-unknown-linux-musl";

    private static bool ExecutionRequested =>
        Environment.GetEnvironmentVariable(ExecEnvVar) == "1";

    private static (int Code, string Stdout, string Stderr) Run(
        string file, string args, TimeSpan timeout)
    {
        using var proc = new Process
        {
            StartInfo = new ProcessStartInfo(file, args)
            {
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                UseShellExecute = false,
            },
        };
        proc.Start();
        var stdout = proc.StandardOutput.ReadToEndAsync();
        var stderr = proc.StandardError.ReadToEndAsync();
        if (!proc.WaitForExit((int)timeout.TotalMilliseconds))
        {
            try { proc.Kill(entireProcessTree: true); } catch { /* already gone */ }
            return (-1, stdout.Result, "TIMEOUT: " + stderr.Result);
        }
        return (proc.ExitCode, stdout.Result, stderr.Result);
    }

    private static string WriteScript()
    {
        var path = Path.Combine(Path.GetTempPath(), $"reinstall-{Guid.NewGuid():N}.sh");
        File.WriteAllText(path, TesterInstallScripts.ReinstallScript(Tag, Target));
        return path;
    }

    [Fact]
    public void The_generated_script_is_valid_bash()
    {
        // `bash -n` parses without executing: catches an unbalanced quote, a
        // broken if/fi, a stray continuation — none of which a substring grep
        // can see, and all of which abort the run-command payload on the VM
        // before a single line executes.
        if (!OperatingSystem.IsLinux() && !OperatingSystem.IsMacOS())
        {
            return; // no bash to parse with
        }

        var path = WriteScript();
        try
        {
            var (code, _, stderr) = Run("/bin/bash", $"-n {path}", TimeSpan.FromSeconds(30));
            Assert.True(code == 0,
                $"the generated reinstall script is not valid bash — Azure run-command would "
                + $"abort before executing anything:\n{stderr}");
        }
        finally
        {
            File.Delete(path);
        }
    }

    [Fact]
    public void The_script_actually_reinstalls_the_tester()
    {
        if (!ExecutionRequested)
        {
            // Deliberately a no-op rather than a failure: this downloads real
            // release assets and writes to /usr/local/bin, so it belongs to the
            // dedicated CI job that opts in. `The_execution_gate_is_wired_in_ci`
            // below is what stops that job from quietly disappearing and taking
            // this coverage with it.
            return;
        }

        var path = WriteScript();
        try
        {
            var (code, stdout, stderr) = Run("/bin/bash", path, TimeSpan.FromMinutes(5));

            Assert.True(code == 0,
                $"the reinstall script failed (exit {code}). This is what a tester VM would see "
                + $"mid-upgrade, with its agent already stopped.\nstdout:\n{stdout}\nstderr:\n{stderr}");

            // The script's last line runs the freshly installed binary, so its
            // output is proof the install produced something executable — not
            // merely that curl and tar returned 0.
            Assert.Contains("networker-tester", stdout, StringComparison.OrdinalIgnoreCase);
            Assert.True(File.Exists("/usr/local/bin/networker-tester"),
                "the script exited 0 without leaving a tester binary in /usr/local/bin");
        }
        finally
        {
            File.Delete(path);
        }
    }

    [Fact]
    public void The_script_names_assets_that_the_release_workflow_builds()
    {
        // The cheap half of the execution test, and the one that would have
        // caught the v0.28.148 decommission fallout: cross-reference the asset
        // names against release.yml instead of trusting them.
        var script = TesterInstallScripts.ReinstallScript(Tag, Target);
        var workflow = File.ReadAllText(
            Path.Combine(RepoRoot(), ".github", "workflows", "release.yml"));

        foreach (var asset in new[] { "networker-tester-", "networker-agent-cs-linux-x64.tar.gz" })
        {
            Assert.True(script.Contains(asset, StringComparison.Ordinal),
                $"the reinstall script no longer references '{asset}'");
            Assert.True(workflow.Contains(asset, StringComparison.Ordinal),
                $"the reinstall script downloads '{asset}', which release.yml does not build — "
                + "an upgrade would 404 on every tester VM");
        }
    }

    [Fact]
    public void The_execution_gate_is_wired_in_ci()
    {
        // Guards the opt-in above. A test that only runs when an env var is set
        // provides zero coverage the moment nothing sets it — and that failure
        // is invisible, because the suite stays green either way.
        var workflows = Directory.GetFiles(
            Path.Combine(RepoRoot(), ".github", "workflows"), "*.yml");
        var wired = workflows.Any(f => File.ReadAllText(f).Contains(ExecEnvVar, StringComparison.Ordinal));

        Assert.True(wired,
            $"no workflow sets {ExecEnvVar}=1, so the reinstall script is never actually "
            + "executed anywhere — the opt-in test is dead weight. Re-add the job or delete "
            + "the test rather than leaving coverage that only looks like coverage.");
    }

    private static string RepoRoot()
    {
        var dir = new DirectoryInfo(AppContext.BaseDirectory);
        while (dir is not null && !Directory.Exists(Path.Combine(dir.FullName, ".github")))
        {
            dir = dir.Parent;
        }
        Assert.True(dir is not null, "could not locate the repository root from the test binary");
        return dir!.FullName;
    }
}
