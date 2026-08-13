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
                if (os == "windows")
                {
                    errors.Add($"endpoints[{i}]: reference-API languages require a Linux endpoint (os is 'windows')");
                }
                foreach (var l in langs)
                {
                    // Contradiction by construction: deploy-config languages
                    // are Linux-only and .NET Framework 4.8 is Windows-only.
                    if (l?.GetValue<string>() == "csharp-net48")
                    {
                        errors.Add($"endpoints[{i}]: csharp-net48 cannot be deployed as a reference API "
                            + "(languages require Linux; .NET Framework 4.8 requires Windows — "
                            + "measure it via the Application Benchmark flow)");
                    }
                }
            }
        }

        return errors;
    }
}
