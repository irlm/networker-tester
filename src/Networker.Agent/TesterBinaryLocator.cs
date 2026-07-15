using System.Diagnostics;
using System.Runtime.InteropServices;

namespace Networker.Agent;

/// <summary>
/// Locates the <c>networker-tester</c> binary on this host — a faithful port of
/// the Rust executor's <c>find_tester_binary</c>
/// (crates/networker-agent/src/executor.rs).
///
/// Search order (first hit wins), matching Rust exactly:
///   1. <c>target/debug/networker-tester</c>, <c>target/release/networker-tester</c>
///      relative to the process working directory (bare relative check).
///   2. The same two sub-paths joined onto the current directory, then onto
///      each of up to 5 parent directories.
///   3. PATH lookup via <c>which</c> (Unix) / <c>where</c> (Windows).
///
/// An explicit configured path (<c>AGENT_TESTERPATH</c>) short-circuits the
/// whole search — the Rust agent has no such override, but a fielded deployment
/// wants a pinned path; when set it is used verbatim (the tester is the same
/// binary the installer drops next to the agent).
/// </summary>
public static class TesterBinaryLocator
{
    private static string BinaryName =>
        RuntimeInformation.IsOSPlatform(OSPlatform.Windows)
            ? "networker-tester.exe"
            : "networker-tester";

    private static string[] RelativeSubPaths =>
    [
        Path.Combine("target", "debug", BinaryName),
        Path.Combine("target", "release", BinaryName),
    ];

    /// <summary>Resolve the tester path, or <c>null</c> if it cannot be found.</summary>
    public static async Task<string?> LocateAsync(
        string? configuredPath,
        CancellationToken cancellationToken = default)
    {
        if (!string.IsNullOrWhiteSpace(configuredPath))
            return configuredPath;

        // 1. Bare relative sub-paths (Rust checks these literally first).
        foreach (var rel in RelativeSubPaths)
        {
            if (File.Exists(rel))
                return rel;
        }

        // 2. cwd, then up to 5 parents, joined with each sub-path.
        var cwd = Directory.GetCurrentDirectory();
        foreach (var rel in RelativeSubPaths)
        {
            var p = Path.Combine(cwd, rel);
            if (File.Exists(p))
                return p;
        }

        var dir = new DirectoryInfo(cwd);
        for (var i = 0; i < 5; i++)
        {
            var parent = dir.Parent;
            if (parent is null)
                break;
            foreach (var rel in RelativeSubPaths)
            {
                var p = Path.Combine(parent.FullName, rel);
                if (File.Exists(p))
                    return p;
            }
            dir = parent;
        }

        // 3. PATH lookup (which / where).
        return await ProbePathAsync(cancellationToken).ConfigureAwait(false);
    }

    private static async Task<string?> ProbePathAsync(CancellationToken cancellationToken)
    {
        var lookup = RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? "where" : "which";
        try
        {
            using var proc = new Process
            {
                StartInfo = new ProcessStartInfo
                {
                    FileName = lookup,
                    RedirectStandardOutput = true,
                    RedirectStandardError = true,
                    UseShellExecute = false,
                    CreateNoWindow = true,
                },
            };
            // `which networker-tester` / `where networker-tester` — the tester
            // may or may not carry the .exe suffix on PATH; ask for the bare
            // name (Rust asks for "networker-tester").
            proc.StartInfo.ArgumentList.Add("networker-tester");
            proc.Start();
            var stdout = await proc.StandardOutput.ReadToEndAsync(cancellationToken).ConfigureAwait(false);
            await proc.WaitForExitAsync(cancellationToken).ConfigureAwait(false);
            if (proc.ExitCode == 0)
            {
                // `where` can return multiple lines; take the first, like the
                // Rust trim() on a single-line `which`.
                var first = stdout
                    .Split('\n', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
                    .FirstOrDefault();
                if (!string.IsNullOrEmpty(first))
                    return first;
            }
        }
        catch
        {
            // Best-effort — no `which`/`where` on PATH, or spawn failed.
        }

        return null;
    }
}
