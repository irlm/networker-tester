using Networker.ControlPlane.Dispatch;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Unit tests for <see cref="AgentVersionGate"/> — the min-version gate that
/// keeps <c>assign_run</c> away from agents older than 0.28.0 (they silently
/// drop the message; regressed PR #380). Pure/host-free: the parse and the
/// comparison are exercised directly, mirroring the Rust <c>parse_version</c>
/// semantics (leading <c>v</c> stripped, pre-release suffix after <c>-</c>
/// stripped per part, malformed parts fall back to 0 — never a throw).
/// </summary>
public class AgentVersionGateTests
{
    // ── IsCompatible: the ≥ 0.28.0 gate ─────────────────────────────────────

    [Theory]
    [InlineData("0.28.0")] // exact minimum
    [InlineData("0.28.4")] // patch above minimum
    [InlineData("0.28.13")] // double-digit patch
    [InlineData("0.29.0")] // minor above
    [InlineData("1.0.0")] // major above
    [InlineData("v0.28.1")] // leading 'v' stripped (Rust trim_start_matches('v'))
    [InlineData("0.28.0-rc1")] // pre-release suffix stripped per part
    [InlineData("0.28")] // missing patch falls back to 0 => (0,28,0) == min
    public void Accepts_versions_at_or_above_the_minimum(string version)
        => Assert.True(AgentVersionGate.IsCompatible(version));

    [Theory]
    [InlineData("0.27.9")] // just below the minimum
    [InlineData("0.27.99")]
    [InlineData("0.1.0")]
    [InlineData("0.0.0")]
    public void Rejects_versions_below_the_minimum(string version)
        => Assert.False(AgentVersionGate.IsCompatible(version));

    [Theory]
    [InlineData("garbage")] // parses to 0.0.0 → rejected by comparison
    [InlineData("not.a.version")]
    [InlineData("x.y.z")]
    [InlineData("..")]
    [InlineData("")]
    [InlineData("   ")]
    [InlineData(null)]
    public void Rejects_garbage_null_and_blank(string? version)
        => Assert.False(AgentVersionGate.IsCompatible(version));

    // ── Parse: the Rust parse_version tuple semantics ────────────────────────

    [Fact]
    public void Parse_reads_a_dotted_triple()
        => Assert.Equal((0, 28, 4), AgentVersionGate.Parse("0.28.4"));

    [Fact]
    public void Parse_strips_a_leading_v()
        => Assert.Equal((0, 28, 1), AgentVersionGate.Parse("v0.28.1"));

    [Fact]
    public void Parse_strips_prerelease_suffix_per_part()
        => Assert.Equal((1, 2, 3), AgentVersionGate.Parse("1.2.3-beta.1"));

    [Fact]
    public void Parse_missing_parts_fall_back_to_zero()
    {
        Assert.Equal((1, 0, 0), AgentVersionGate.Parse("1"));
        Assert.Equal((1, 2, 0), AgentVersionGate.Parse("1.2"));
    }

    [Fact]
    public void Parse_malformed_parts_fall_back_to_zero_without_throwing()
    {
        Assert.Equal((0, 0, 0), AgentVersionGate.Parse("garbage"));
        Assert.Equal((1, 0, 3), AgentVersionGate.Parse("1.x.3"));
    }

    [Fact]
    public void Min_version_constant_matches_the_rust_gate()
        => Assert.Equal("0.28.0", AgentVersionGate.MinAssignRunVersionString);
}
