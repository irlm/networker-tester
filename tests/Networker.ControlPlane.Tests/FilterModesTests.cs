using Networker.ControlPlane.Dispatch;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Dispatch-time mode filtering (RunDispatcher.FilterModesText). A config
/// created before v0.28.118 (which removed `native` from the runnable catalog)
/// still lists native → the release tester binary can't run it → every native
/// attempt failed "recompile to enable" (user report 2026-07-31, run f57e5ff9).
/// Dispatch now drops catalog:false modes so those guaranteed-failing attempts
/// disappear — without stripping runner modes (apibench/sdkprobe) or stranding
/// a run with an empty workload.
/// </summary>
public class FilterModesTests
{
    [Fact]
    public void Drops_native_but_keeps_runnable_modes()
    {
        var (text, dropped) = RunDispatcher.FilterModesText(
            """{"runs":10,"modes":["dns","tcp","tls","native","http1","pageload3","websocket"]}""");

        Assert.Equal(new[] { "native" }, dropped);
        Assert.DoesNotContain("native", text);
        foreach (var kept in new[] { "dns", "tcp", "tls", "http1", "pageload3", "websocket" })
        {
            Assert.Contains(kept, text);
        }
    }

    [Fact]
    public void Keeps_runner_level_modes_apibench_and_sdkprobe()
    {
        // These are in the catalog (the agent expands them) — must never be dropped.
        var (_, dropped) = RunDispatcher.FilterModesText("""{"modes":["apibench"]}""");
        Assert.Empty(dropped);

        var (_, dropped2) = RunDispatcher.FilterModesText("""{"modes":["sdkprobe"]}""");
        Assert.Empty(dropped2);
    }

    [Fact]
    public void Drops_bare_browser_stub_but_keeps_browser_variants()
    {
        var (text, dropped) = RunDispatcher.FilterModesText(
            """{"modes":["browser","browser1","browser2","browser3"]}""");

        Assert.Equal(new[] { "browser" }, dropped);
        Assert.Contains("browser1", text);
    }

    [Fact]
    public void A_native_only_config_is_left_untouched_not_stranded()
    {
        // Filtering to empty would silently no-op the run; keep the original so
        // the (real, honest) native failure stays visible instead.
        var input = """{"runs":5,"modes":["native"]}""";
        var (text, dropped) = RunDispatcher.FilterModesText(input);

        Assert.Empty(dropped);
        Assert.Equal(input, text);
    }

    [Theory]
    [InlineData("not json")]
    [InlineData("""{"runs":10}""")]          // no modes array
    [InlineData("""{"modes":"oops"}""")]      // modes not an array
    public void Malformed_or_modeless_workloads_pass_through(string input)
    {
        var (text, dropped) = RunDispatcher.FilterModesText(input);
        Assert.Empty(dropped);
        Assert.Equal(input, text);
    }
}
