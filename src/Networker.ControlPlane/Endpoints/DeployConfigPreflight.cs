using System.Text.Json.Nodes;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Create-time pre-flight for deploy configs — the OS↔stack/language
/// contradictions <c>install.sh validate_deploy_config</c> would reject
/// anyway, rejected BEFORE a doomed deployment row exists (user-caught
/// 2026-08-12: the wizard carried an invisible nginx selection into a
/// Windows config; the row was created, then failed at install.sh).
///
/// <para>Deliberately a MIRROR of install.sh's rules, not a superset: a
/// combination install.sh accepts must pass here (drift would block valid
/// deploys), and one it rejects should ideally fail here first. Keep the two
/// in lockstep when install.sh's validator changes.</para>
/// </summary>
public static class DeployConfigPreflight
{
    /// <summary>Languages install.sh's deploy_benchmark_server deploys on Linux.</summary>
    internal static readonly HashSet<string> LinuxLanguages = new(StringComparer.Ordinal)
    {
        "rust", "nginx", "go", "nodejs", "python", "java", "cpp", "ruby", "php",
        "csharp-net8", "csharp-net8-aot", "csharp-net9", "csharp-net10",
    };

    /// <summary>Languages install.ps1 -BenchmarkServer deploys on Windows (v0.28.204).</summary>
    internal static readonly HashSet<string> WindowsLanguages = new(StringComparer.Ordinal)
    {
        "csharp-net48", "csharp-net8", "csharp-net9", "csharp-net10",
        "go", "nodejs", "python", "java",
    };

    public static IReadOnlyList<string> Validate(JsonNode? config)
    {
        var errors = new List<string>();
        if (config?["endpoints"] is not JsonArray endpoints)
        {
            return errors;
        }

        for (var i = 0; i < endpoints.Count; i++)
        {
            var ep = endpoints[i];
            if (ep is null)
            {
                continue;
            }
            // The OS lives NESTED under the provider key (endpoints[i].azure.os
            // etc.) — exactly where install.sh's validator reads it. A missing
            // key (lan endpoints, absent os) defaults to linux, like install.sh.
            var provider = ep["provider"]?.GetValue<string>();
            var os = (provider is null ? null : ep[provider]?["os"]?.GetValue<string>()) ?? "linux";

            if (ep["http_stacks"] is JsonArray stacks)
            {
                foreach (var s in stacks)
                {
                    var name = s?.GetValue<string>();
                    if (name == "nginx" && os == "windows")
                    {
                        errors.Add($"endpoints[{i}]: nginx requires Linux but os is 'windows'");
                    }
                    if (name == "iis" && os == "linux")
                    {
                        errors.Add($"endpoints[{i}]: IIS requires Windows but os is 'linux'");
                    }
                }
            }

            if (ep["languages"] is JsonArray langs && langs.Count > 0)
            {
                // Per-OS language sets (v0.28.204) — mirrors install.sh's
                // validator: Linux deploys via deploy_benchmark_server, Windows
                // via install.ps1 -BenchmarkServer. net48 is Windows-only
                // (.NET Framework); cpp/ruby/php + AOT variants are Linux-only
                // (MSVC/devkit/swoole constraints).
                var valid = os == "windows" ? WindowsLanguages : LinuxLanguages;
                var other = os == "windows" ? LinuxLanguages : WindowsLanguages;
                foreach (var l in langs)
                {
                    var name = l?.GetValue<string>();
                    if (name is null || valid.Contains(name))
                    {
                        continue;
                    }
                    errors.Add(other.Contains(name)
                        ? $"endpoints[{i}]: '{name}' is not deployable on a {os} endpoint"
                        : $"endpoints[{i}]: unknown language '{name}'");
                }
            }
        }

        return errors;
    }
}
