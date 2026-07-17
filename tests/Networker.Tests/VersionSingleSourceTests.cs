using System.Text.RegularExpressions;
using Networker.Agent;
using Networker.ControlPlane.Dispatch;
using Networker.ControlPlane.Endpoints;
using Networker.Endpoint;
using Xunit;

namespace Networker.Tests;

/// <summary>
/// Guards the single-sourced version scheme: every C# runtime-reported version
/// derives from the assembly version stamped by the repo-root
/// Directory.Build.props (which CI keeps equal to Cargo.toml). These tests
/// fail if a per-project &lt;Version&gt; override reappears (drift, the exact
/// bug that left the agent at 0.28.26 while the endpoint said 0.28.28) or if
/// the derived string stops being gate-compatible.
/// </summary>
public class VersionSingleSourceTests
{
    private static readonly Regex DottedTriple = new(@"^\d+\.\d+\.\d+$");

    [Fact]
    public void Agent_controlplane_and_endpoint_report_the_same_version()
    {
        // One Directory.Build.props stamps all three assemblies; any per-csproj
        // override would break this immediately.
        Assert.Equal(VersionEndpoints.DashboardVersion, AgentVersion.Current);
        Assert.Equal(VersionEndpoints.DashboardVersion, ServerInfo.Version);
    }

    [Fact]
    public void Reported_versions_are_normalized_dotted_triples()
    {
        // The fielded Rust agents report "0.28.31", not the 4-part "0.28.31.0"
        // of AssemblyName.Version. The C# side must match that shape so every
        // consumer (AgentVersionGate, Rust parse_version, the frontend badge)
        // sees the same format from both implementations.
        Assert.Matches(DottedTriple, AgentVersion.Current);
        Assert.Matches(DottedTriple, VersionEndpoints.DashboardVersion);
        Assert.Matches(DottedTriple, ServerInfo.Version);
    }

    [Fact]
    public void Agent_reported_version_passes_the_dispatch_version_gate()
    {
        // RunDispatcher only assigns runs to agents whose reported version is
        // ≥ 0.28.0 (AgentVersionGate). The assembly-derived version must parse
        // and pass, or every C# agent would silently stop receiving runs.
        Assert.True(AgentVersionGate.IsCompatible(AgentVersion.Current));

        var parsed = AgentVersionGate.Parse(AgentVersion.Current);
        var parts = AgentVersion.Current.Split('.');
        Assert.Equal((int.Parse(parts[0]), int.Parse(parts[1]), int.Parse(parts[2])), parsed);
    }

    [Fact]
    public void Gate_parses_four_part_assembly_versions_identically()
    {
        // Defence in depth: even if a future refactor reverts to the raw 4-part
        // AssemblyName.Version string, the gate must read the same triple
        // (fielded 0.28.26 C# agents reported "0.28.26.0" and were dispatched).
        Assert.Equal(AgentVersionGate.Parse("0.28.31"), AgentVersionGate.Parse("0.28.31.0"));
        Assert.True(AgentVersionGate.IsCompatible("0.28.26.0"));
    }
}
