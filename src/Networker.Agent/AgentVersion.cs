using System.Reflection;

namespace Networker.Agent;

/// <summary>
/// The agent's self-reported version — heartbeats, command results, and
/// <c>client_version</c> stamped on run results all read this.
///
/// <para>Derived from the assembly version that the repo-root
/// <c>Directory.Build.props</c> stamps (single-sourced with <c>Cargo.toml</c>;
/// CI enforces the match). Never hardcode a version string here.</para>
///
/// <para>Normalized to the dotted-triple form the fielded Rust agents report
/// ("0.28.31", not the 4-part "0.28.31.0" of <c>AssemblyName.Version</c>), so
/// the control plane's <c>AgentVersionGate</c> / Rust <c>parse_version</c> see
/// the exact same shape from both agent implementations.</para>
/// </summary>
public static class AgentVersion
{
    /// <summary>Dotted-triple version string, e.g. "0.28.31".</summary>
    public static readonly string Current = FromAssembly(typeof(AgentVersion).Assembly);

    /// <summary>
    /// Prefer <c>AssemblyInformationalVersion</c> (exactly the
    /// Directory.Build.props <c>&lt;Version&gt;</c>, a dotted triple), stripping
    /// any "+buildmetadata" defensively; fall back to the 4-part assembly
    /// version truncated to major.minor.patch.
    /// </summary>
    public static string FromAssembly(Assembly assembly)
    {
        var info = assembly
            .GetCustomAttribute<AssemblyInformationalVersionAttribute>()?
            .InformationalVersion;
        if (!string.IsNullOrWhiteSpace(info))
        {
            var v = info.Split('+')[0].Trim();
            if (v.Length > 0)
            {
                return v;
            }
        }

        var av = assembly.GetName().Version;
        return av is null ? "0.0.0" : $"{av.Major}.{av.Minor}.{av.Build}";
    }
}
