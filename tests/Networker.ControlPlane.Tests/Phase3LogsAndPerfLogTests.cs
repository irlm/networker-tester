using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pure-logic parity for the logs / perf-log ports: log-level → DB-value mapping
/// (mirrors <c>networker_log::Level::from_str</c> + <c>as_db</c>), and the
/// perf-log ILIKE wildcard escaping.
/// </summary>
public class Phase3LogsAndPerfLogTests
{
    [Theory]
    [InlineData("ERROR", (short)1)]
    [InlineData("err", (short)1)]
    [InlineData("FATAL", (short)1)]
    [InlineData("1", (short)1)]
    [InlineData("WARN", (short)2)]
    [InlineData("warning", (short)2)]
    [InlineData("2", (short)2)]
    [InlineData("INFO", (short)3)]
    [InlineData("information", (short)3)]
    [InlineData("3", (short)3)]
    [InlineData("DEBUG", (short)4)]
    [InlineData("dbg", (short)4)]
    [InlineData("4", (short)4)]
    [InlineData("TRACE", (short)5)]
    [InlineData("trc", (short)5)]
    [InlineData("5", (short)5)]
    [InlineData("  info  ", (short)3)]
    public void ParseLevelToDb_maps_known_levels(string input, short expected)
    {
        Assert.Equal(expected, LogsEndpoints.ParseLevelToDb(input));
    }

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("nonsense")]
    [InlineData("6")]
    public void ParseLevelToDb_returns_null_for_unknown(string? input)
    {
        Assert.Null(LogsEndpoints.ParseLevelToDb(input));
    }

    [Fact]
    public void EscapeIlike_escapes_wildcards_and_backslash()
    {
        // Backslash first (so already-escaped output isn't double-processed the
        // wrong way), then % and _ — matches the Rust escape order.
        Assert.Equal("100\\%", PerfLogEndpoints.EscapeIlike("100%"));
        Assert.Equal("a\\_b", PerfLogEndpoints.EscapeIlike("a_b"));
        Assert.Equal("a\\\\b", PerfLogEndpoints.EscapeIlike("a\\b"));
        Assert.Equal("plain", PerfLogEndpoints.EscapeIlike("plain"));
    }
}
