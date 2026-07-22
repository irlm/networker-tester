using Networker.Agent;

namespace Networker.Agent.Tests;

/// <summary>
/// <see cref="TesterBinaryLocator"/> — resolves the networker-tester binary the
/// agent shells out to for every run. Untested until now
/// (coverage-libs-sdks-frontend-2026-07.md). The configured-path contract is the
/// deployment-critical behavior: a fielded agent pins AGENT_TESTERPATH to the
/// binary the installer dropped next to it, and that path must be honored exactly
/// — never silently overridden by a different networker-tester found on PATH.
///
/// The filesystem search (target/debug|release + parent walk + PATH probe) reads
/// the process-global current directory and cannot be exercised deterministically
/// without mutating it (parallel-unsafe), so these tests pin only the pure,
/// input-determined branch: the configured-path short-circuit.
/// </summary>
public sealed class TesterBinaryLocatorTests
{
    [Fact]
    public async Task Configured_path_is_returned_verbatim_and_short_circuits_the_search()
    {
        // A pinned path is trusted as-is — even a non-existent one is returned
        // unchanged (no File.Exists gate), so the agent runs the exact binary the
        // deployment pinned and a typo fails loudly at exec rather than silently
        // resolving a different binary from PATH.
        const string pinned = "/opt/laghound/networker-tester";

        var result = await TesterBinaryLocator.LocateAsync(pinned);

        Assert.Equal(pinned, result);
    }

    [Theory]
    [InlineData("")]
    [InlineData("   ")]
    public async Task Blank_configured_path_is_not_treated_as_a_location(string configured)
    {
        // A blank/whitespace configured path must NOT short-circuit as a path; it
        // falls through to the search, whose result is a real path or null — never
        // the blank string echoed back.
        var result = await TesterBinaryLocator.LocateAsync(configured);

        Assert.NotEqual(configured, result);
    }
}
