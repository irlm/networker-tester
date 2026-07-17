using System.Reflection;
using System.Text.RegularExpressions;
using Networker.Endpoint;
using Xunit;

namespace Networker.Endpoint.Tests;

/// <summary>
/// Guards the endpoint's slice of the single-sourced version scheme:
/// <see cref="ServerInfo.Version"/> must derive from the assembly version
/// stamped by the repo-root Directory.Build.props (which CI keeps equal to
/// Cargo.toml) — the drift this catches is exactly the bug that left
/// ServerInfo hardcoded at 0.28.28 while the workspace was at 0.28.30.
/// Lives here rather than tests/Networker.Tests because referencing the
/// endpoint exe there would collide two top-level Program classes (CS0433).
/// </summary>
public class VersionSingleSourceTests
{
    [Fact]
    public void ServerInfo_version_matches_the_single_sourced_assembly_version()
    {
        // This test assembly is stamped by the same Directory.Build.props, so
        // equality proves ServerInfo derives from the single source rather
        // than a hand-maintained constant.
        var thisAssembly = typeof(VersionSingleSourceTests).Assembly
            .GetCustomAttribute<AssemblyInformationalVersionAttribute>()!
            .InformationalVersion.Split('+')[0];

        Assert.Equal(thisAssembly, ServerInfo.Version);
    }

    [Fact]
    public void ServerInfo_version_is_a_normalized_dotted_triple()
    {
        // "0.28.31", never the 4-part "0.28.31.0" — the same shape the Rust
        // endpoint reports via CARGO_PKG_VERSION, so /health consumers (e.g.
        // the dashboard's endpoint version probe) see one format.
        Assert.Matches(new Regex(@"^\d+\.\d+\.\d+$"), ServerInfo.Version);
    }
}
