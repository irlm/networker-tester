using System.Diagnostics;
using System.Runtime.InteropServices;

namespace Networker.Agent;

/// <summary>
/// Detects the runner's optional tool inventory once at startup so the
/// heartbeat can carry it (<see cref="AgentCapabilities"/>): whether a Chrome /
/// Chromium binary is present (the tester's <c>browser1/2/3</c> modes spawn
/// it via chromiumoxide) and whether <c>tshark</c> is on PATH (packet capture).
///
/// <para>The Chrome search mirrors <c>find_chrome()</c> in
/// <c>crates/networker-tester/src/runner/browser.rs</c> — <c>NETWORKER_CHROME_PATH</c>
/// first, then the well-known Windows / Linux / macOS install paths, then a
/// PATH lookup — so the agent reports exactly what the tester will find.
/// Best-effort and never throws: an unreadable filesystem or a missing
/// <c>which</c>/<c>where</c> just yields <c>false</c>.</para>
/// </summary>
public static class RunnerCapabilities
{
    private static readonly Lazy<AgentCapabilities> Cached = new(() =>
        new AgentCapabilities(Chrome: DetectChrome(), Tshark: DetectTshark()));

    /// <summary>The detected inventory (computed once, cached for the process).</summary>
    public static AgentCapabilities Current => Cached.Value;

    /// <summary>Whether a Chrome/Chromium binary the tester would use exists.</summary>
    public static bool DetectChrome()
    {
        try
        {
            var env = Environment.GetEnvironmentVariable("NETWORKER_CHROME_PATH");
            if (!string.IsNullOrWhiteSpace(env) && File.Exists(env))
            {
                return true;
            }

            foreach (var candidate in ChromeCandidatePaths())
            {
                if (File.Exists(candidate))
                {
                    return true;
                }
            }

            foreach (var name in new[] { "google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "chrome" })
            {
                if (OnPath(name))
                {
                    return true;
                }
            }
        }
        catch
        {
            // best-effort
        }
        return false;
    }

    /// <summary>Whether <c>tshark</c> is resolvable (env override or PATH).</summary>
    public static bool DetectTshark()
    {
        try
        {
            var env = Environment.GetEnvironmentVariable("NETWORKER_TSHARK_PATH");
            if (!string.IsNullOrWhiteSpace(env) && File.Exists(env))
            {
                return true;
            }
            if (OnPath("tshark"))
            {
                return true;
            }
            if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
            {
                foreach (var root in new[] { Environment.GetEnvironmentVariable("PROGRAMFILES"), Environment.GetEnvironmentVariable("PROGRAMFILES(X86)") })
                {
                    if (!string.IsNullOrEmpty(root) && File.Exists(Path.Combine(root, "Wireshark", "tshark.exe")))
                    {
                        return true;
                    }
                }
            }
        }
        catch
        {
            // best-effort
        }
        return false;
    }

    private static IEnumerable<string> ChromeCandidatePaths()
    {
        if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
        {
            var roots = new[] { "PROGRAMFILES", "LOCALAPPDATA", "PROGRAMFILES(X86)" }
                .Select(Environment.GetEnvironmentVariable)
                .Where(r => !string.IsNullOrEmpty(r))
                .Select(r => r!);
            foreach (var root in roots)
            {
                yield return Path.Combine(root, "Google", "Chrome", "Application", "chrome.exe");
                yield return Path.Combine(root, "Chromium", "Application", "chrome.exe");
            }
            yield break;
        }

        yield return "/usr/bin/google-chrome";
        yield return "/usr/bin/chromium-browser";
        yield return "/usr/bin/chromium";
        yield return "/usr/bin/google-chrome-stable";
        yield return "/snap/bin/chromium";
        var home = Environment.GetEnvironmentVariable("HOME");
        if (!string.IsNullOrEmpty(home))
        {
            yield return Path.Combine(home, ".local", "bin", "google-chrome");
            yield return Path.Combine(home, ".local", "bin", "chromium");
            yield return Path.Combine(home, ".local", "google-chrome", "google-chrome");
        }
        yield return "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
        yield return "/Applications/Chromium.app/Contents/MacOS/Chromium";
        yield return "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary";
    }

    /// <summary>Walk PATH ourselves (no child process) — the same answer
    /// <c>which</c>/<c>where</c> would give, without spawning at startup.</summary>
    private static bool OnPath(string name)
    {
        var path = Environment.GetEnvironmentVariable("PATH");
        if (string.IsNullOrEmpty(path))
        {
            return false;
        }
        var isWindows = RuntimeInformation.IsOSPlatform(OSPlatform.Windows);
        var names = isWindows ? new[] { name + ".exe", name + ".cmd", name + ".bat", name } : [name];
        foreach (var dir in path.Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries))
        {
            foreach (var n in names)
            {
                try
                {
                    if (File.Exists(Path.Combine(dir, n)))
                    {
                        return true;
                    }
                }
                catch
                {
                    // unreadable PATH entry — skip
                }
            }
        }
        return false;
    }
}
