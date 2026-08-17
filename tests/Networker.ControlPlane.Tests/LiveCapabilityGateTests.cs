using Networker.ControlPlane.Endpoints;
using Report = Networker.ControlPlane.Endpoints.TargetCapabilities.HostCapabilityReport;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Rule 3 of the config-create gate — the target's LIVE <c>/health</c>
/// <c>services</c> self-report (v0.28.210). The gate must reject a mode only
/// on POSITIVE knowledge that the endpoint instance cannot serve it, relay the
/// endpoint's own reason (the same text the UI shows), never wait on a probe
/// (cache-only, fail open on miss / stale, background refresh queued), and
/// leave the kind and h3-by-stack rules exactly as they were.
/// </summary>
public class LiveCapabilityGateTests
{
    // ── Fixtures ──────────────────────────────────────────────────────────────

    private const string FullHealth = """
        {"status":"ok","version":"0.28.210","services":{"download":8080,"upload":8080,"udp_echo":9999,"udp_throughput":9998,"stamp":9997,"ws_echo":true,"page_assets":true}}
        """;

    /// <summary>An instance with the UDP + STAMP listeners disabled (port 0 →
    /// the endpoint reports them null) — the exact per-instance case rule 3
    /// exists for.</summary>
    private const string NoUdpHealth = """
        {"status":"ok","version":"0.28.210","services":{"download":8080,"upload":8080,"udp_echo":null,"udp_throughput":null,"stamp":null,"ws_echo":true,"page_assets":true}}
        """;

    /// <summary>Pre-0.28.202 endpoint: reachable, no self-report.</summary>
    private const string LegacyHealth = """{"status":"ok","version":"0.28.100"}""";

    private static Report Full(string host = "10.0.0.1") => TargetCapabilities.ParseHealth(host, FullHealth);
    private static Report NoUdp(string host = "10.0.0.1") => TargetCapabilities.ParseHealth(host, NoUdpHealth);
    private static Report Legacy(string host = "10.0.0.1") => TargetCapabilities.ParseHealth(host, LegacyHealth);
    private static Report Dead(string host = "10.0.0.1") => new(host, false, null, null, null, null);

    /// <summary>Manual clock so TTL expiry is deterministic.</summary>
    private sealed class FakeClock : TimeProvider
    {
        private long _ticks;
        public override long TimestampFrequency => TimeSpan.TicksPerSecond;
        public override long GetTimestamp() => _ticks;
        public void Advance(TimeSpan by) => _ticks += by.Ticks;
    }

    private sealed class FakeProber
    {
        private readonly Func<string, Report> _answer;
        public int Calls;
        public TaskCompletionSource Gate = new(TaskCreationOptions.RunContinuationsAsynchronously);
        public FakeProber(Func<string, Report> answer) => _answer = answer;
        public async Task<Report> ProbeAsync(string host)
        {
            Interlocked.Increment(ref Calls);
            await Gate.Task;
            return _answer(host);
        }
    }

    // ── ParseHealth: the endpoint's self-report → capability report ──────────

    [Fact]
    public void ParseHealth_maps_disabled_listeners_to_unsupported_modes_with_reasons()
    {
        var r = NoUdp();
        Assert.True(r.Reachable);
        Assert.True(r.HasReport);
        Assert.Equal("0.28.210", r.Version);
        var off = r.UnsupportedMap();
        Assert.Equal("UDP echo listener disabled on this target", off["udp"]);
        Assert.Equal("UDP throughput listener disabled on this target", off["udpdownload"]);
        Assert.Equal("UDP throughput listener disabled on this target", off["udpupload"]);
        Assert.Equal("STAMP reflector disabled on this target", off["stamp"]);
        Assert.DoesNotContain("download", off.Keys);
        Assert.DoesNotContain("websocket", off.Keys);
        Assert.DoesNotContain("pageload", off.Keys);
        Assert.Contains("download", r.Supported!);
    }

    [Fact]
    public void ParseHealth_without_services_or_malformed_has_no_report()
    {
        Assert.False(Legacy().HasReport);
        Assert.True(Legacy().Reachable);
        Assert.Empty(Legacy().UnsupportedMap());
        var junk = TargetCapabilities.ParseHealth("h", "not json");
        Assert.True(junk.Reachable);
        Assert.False(junk.HasReport);
        Assert.False(Dead().HasReport);
    }

    // ── IncompatibleModes with a live report (rule 3) ────────────────────────

    [Fact]
    public void Live_unsupported_mode_is_rejected_with_the_endpoints_own_reason()
    {
        var live = NoUdp().UnsupportedMap();
        var bad = ModeTargetCompatibility.IncompatibleModes(
            ["http1", "download", "udp", "stamp", "udpdownload"], "proxy", "nginx", live);
        Assert.Equal(["udp", "stamp", "udpdownload"], bad.Select(b => b.Mode).ToArray());
        Assert.All(bad, b => Assert.Equal(ModeTargetCompatibility.LiveRequirement, b.Requirement));
        Assert.Equal("UDP echo listener disabled on this target", bad[0].Reason);
        Assert.Equal("STAMP reflector disabled on this target", bad[1].Reason);
    }

    [Fact]
    public void Live_supported_modes_pass_and_unknown_or_empty_reports_fail_open()
    {
        var modes = new[] { "udp", "stamp", "download", "pageload", "websocket", "http3" };
        // Everything on → nothing rejected.
        Assert.Empty(ModeTargetCompatibility.IncompatibleModes(modes, "proxy", "nginx", Full().UnsupportedMap()));
        // No knowledge (null / empty) → identical to the pre-0.28.210 gate.
        Assert.Empty(ModeTargetCompatibility.IncompatibleModes(modes, "proxy", "nginx", null));
        Assert.Empty(ModeTargetCompatibility.IncompatibleModes(modes, "proxy", "nginx", new Dictionary<string, string>()));
        Assert.Empty(ModeTargetCompatibility.IncompatibleModes(modes, "proxy", null, Legacy().UnsupportedMap()));
    }

    [Fact]
    public void Live_rule_is_case_insensitive_on_mode_ids()
    {
        var live = NoUdp().UnsupportedMap();
        var bad = ModeTargetCompatibility.IncompatibleModes(["UDP", "Stamp"], "proxy", "nginx", live);
        Assert.Equal(["UDP", "Stamp"], bad.Select(b => b.Mode).ToArray());
        // Even a case-sensitive dictionary from a caller is tolerated.
        var strict = new Dictionary<string, string>(StringComparer.Ordinal) { ["udp"] = "off" };
        Assert.Single(ModeTargetCompatibility.IncompatibleModes(["UDP"], "proxy", null, strict));
    }

    [Fact]
    public void Kind_and_h3_rules_win_over_live_and_each_mode_is_reported_once()
    {
        // A live report saying `udp` is off must not change how a raw URL is
        // judged (kind rule fires first, reported once, with the kind reason).
        var live = new Dictionary<string, string> { ["udp"] = "UDP echo listener disabled on this target", ["http3"] = "no h3" };
        var url = ModeTargetCompatibility.IncompatibleModes(["udp"], "network", null, live);
        Assert.Single(url);
        Assert.Equal("networker-endpoint", url[0].Requirement);

        // apache + h3 mode: the stack rule fires (h3-by-stack intact) with the
        // stack reason, and the mode is not double-reported.
        var h3 = ModeTargetCompatibility.IncompatibleModes(["http3", "http1"], "proxy", "apache", live);
        Assert.Single(h3);
        Assert.Equal(ModeTargetCompatibility.H3Requirement, h3[0].Requirement);
        Assert.Contains("apache has no HTTP/3", h3[0].Reason);
    }

    [Fact]
    public void Live_rule_applies_to_pending_kind_too_when_a_report_exists()
    {
        // pending fails open on kind; a live report is positive knowledge and applies.
        var bad = ModeTargetCompatibility.IncompatibleModes(["apibench", "udp"], "pending", null, NoUdp().UnsupportedMap());
        Assert.Equal(["udp"], bad.Select(b => b.Mode).ToArray());
    }

    // ── FoldLiveReports: multi-host semantics (mirror of NetworkTestPage) ────

    [Fact]
    public void Fold_marks_a_mode_off_only_when_every_host_says_so()
    {
        var folded = ModeTargetCompatibility.FoldLiveReports([NoUdp("a"), Full("b")]);
        Assert.NotNull(folded);
        Assert.Empty(folded!); // b serves udp/stamp → the deployment does

        var both = ModeTargetCompatibility.FoldLiveReports([NoUdp("a"), NoUdp("b")]);
        Assert.NotNull(both);
        Assert.Contains("udp", both!.Keys);
        Assert.Contains("stamp", both.Keys);

        var single = ModeTargetCompatibility.FoldLiveReports([NoUdp("a")]);
        Assert.Equal(4, single!.Count);
    }

    [Fact]
    public void Fold_is_unknown_when_any_host_lacks_a_report_or_no_hosts()
    {
        Assert.Null(ModeTargetCompatibility.FoldLiveReports([]));
        Assert.Null(ModeTargetCompatibility.FoldLiveReports([NoUdp("a"), null]));
        Assert.Null(ModeTargetCompatibility.FoldLiveReports([NoUdp("a"), Legacy("b")]));
        Assert.Null(ModeTargetCompatibility.FoldLiveReports([NoUdp("a"), Dead("b")]));
    }

    // ── LiveCapabilityCache: never blocks, fail-open on miss, background refresh ──

    [Fact]
    public async Task Cache_miss_returns_null_immediately_and_queues_one_background_probe()
    {
        var clock = new FakeClock();
        var prober = new FakeProber(NoUdp);
        var cache = new LiveCapabilityCache(prober.ProbeAsync, clock, TimeSpan.FromSeconds(60));

        // The probe is gated open-ended: the lookup must still return at once.
        Assert.Null(cache.TryGetFresh("10.0.0.1"));
        Assert.Null(cache.TryGetFresh("10.0.0.1"));
        Assert.Null(cache.TryGetFresh("10.0.0.1"));
        // Give the Task.Run a moment to start, then check the dedup: 3 misses → 1 probe.
        await WaitUntilAsync(() => Volatile.Read(ref prober.Calls) >= 1);
        Assert.Equal(1, prober.Calls);

        prober.Gate.SetResult();
        await WaitUntilAsync(() => cache.Peek("10.0.0.1") is not null);

        var snap = cache.TryGetFresh("10.0.0.1");
        Assert.NotNull(snap);
        Assert.True(snap!.Value.Report.HasReport);
        Assert.Contains("udp", snap.Value.Report.UnsupportedMap().Keys);
        Assert.Equal(TimeSpan.Zero, snap.Value.Age);
        Assert.Equal(1, prober.Calls); // fresh → no re-probe
    }

    [Fact]
    public async Task Cache_entry_older_than_ttl_is_not_used_and_triggers_a_refresh()
    {
        var clock = new FakeClock();
        var prober = new FakeProber(NoUdp);
        prober.Gate.SetResult();
        var cache = new LiveCapabilityCache(prober.ProbeAsync, clock, TimeSpan.FromSeconds(60));
        cache.Store(NoUdp());

        clock.Advance(TimeSpan.FromSeconds(59));
        var snap = cache.TryGetFresh("10.0.0.1");
        Assert.NotNull(snap);
        Assert.Equal(TimeSpan.FromSeconds(59), snap!.Value.Age);
        Assert.Equal(0, prober.Calls);

        clock.Advance(TimeSpan.FromSeconds(2)); // 61s → stale → fail open + refresh
        Assert.Null(cache.TryGetFresh("10.0.0.1"));
        await WaitUntilAsync(() => Volatile.Read(ref prober.Calls) >= 1);
        await WaitUntilAsync(() => cache.Peek("10.0.0.1") is { Age: var a } && a == TimeSpan.Zero);
        Assert.NotNull(cache.TryGetFresh("10.0.0.1"));
    }

    [Fact]
    public async Task Cache_survives_a_throwing_prober_and_stays_fail_open()
    {
        var clock = new FakeClock();
        var calls = 0;
        var cache = new LiveCapabilityCache(
            _ => { Interlocked.Increment(ref calls); throw new HttpRequestException("boom"); },
            clock, TimeSpan.FromSeconds(60));
        Assert.Null(cache.TryGetFresh("dead"));
        await cache.TriggerRefresh("dead"); // no-op while in flight, else runs and swallows
        await WaitUntilAsync(() => Volatile.Read(ref calls) >= 1);
        Assert.Null(cache.Peek("dead"));
        Assert.Null(cache.TryGetFresh("dead"));
    }

    // ── ResolveLiveCapabilities: what the create handler feeds the gate ──────

    [Fact]
    public void Resolve_uses_only_fresh_reports_and_reports_the_oldest_age()
    {
        var clock = new FakeClock();
        var cache = new LiveCapabilityCache(h => Task.FromResult(NoUdp(h)), clock, TimeSpan.FromSeconds(90));

        // No hosts (network / runtime / pending target) → unknown, no probe.
        Assert.Null(TestConfigWriteEndpoints.ResolveLiveCapabilities(cache, []).Unsupported);

        // Host known + fresh → the map, with its age.
        cache.Store(NoUdp("a"));
        clock.Advance(TimeSpan.FromSeconds(12));
        var one = TestConfigWriteEndpoints.ResolveLiveCapabilities(cache, ["a"]);
        Assert.NotNull(one.Unsupported);
        Assert.Contains("udp", one.Unsupported!.Keys);
        Assert.Equal(12, one.AgeSecs);

        // Second host with no fresh report → whole answer unknown (fail open).
        var two = TestConfigWriteEndpoints.ResolveLiveCapabilities(cache, ["a", "b"]);
        Assert.Null(two.Unsupported);

        // Once b reports too, both are consulted (every-host rule) → oldest age.
        cache.Store(Full("b"));
        var both = TestConfigWriteEndpoints.ResolveLiveCapabilities(cache, ["a", "b"]);
        Assert.NotNull(both.Unsupported);
        Assert.Empty(both.Unsupported!); // b serves udp
        Assert.Equal(12, both.AgeSecs);
    }

    [Fact]
    public void Wire_shape_of_a_report_matches_the_capabilities_route_contract()
    {
        var wire = System.Text.Json.JsonSerializer.SerializeToElement(NoUdp().ToWire());
        Assert.Equal("10.0.0.1", wire.GetProperty("host").GetString());
        Assert.True(wire.GetProperty("reachable").GetBoolean());
        Assert.Equal("0.28.210", wire.GetProperty("version").GetString());
        Assert.Equal(System.Text.Json.JsonValueKind.Object, wire.GetProperty("services").ValueKind);
        Assert.Contains("download", wire.GetProperty("supported_modes").EnumerateArray().Select(e => e.GetString()));
        var off = wire.GetProperty("unsupported_modes").EnumerateArray().ToList();
        Assert.Contains(off, e => e.GetProperty("mode").GetString() == "udp"
            && e.GetProperty("reason").GetString() == "UDP echo listener disabled on this target");

        var dead = System.Text.Json.JsonSerializer.SerializeToElement(Dead().ToWire());
        Assert.Equal(System.Text.Json.JsonValueKind.Null, dead.GetProperty("services").ValueKind);
        Assert.Equal(System.Text.Json.JsonValueKind.Null, dead.GetProperty("supported_modes").ValueKind);
        Assert.Equal(System.Text.Json.JsonValueKind.Null, dead.GetProperty("unsupported_modes").ValueKind);
    }

    private static async Task WaitUntilAsync(Func<bool> cond, int timeoutMs = 5000)
    {
        var deadline = DateTime.UtcNow.AddMilliseconds(timeoutMs);
        while (!cond())
        {
            if (DateTime.UtcNow > deadline)
            {
                throw new TimeoutException("condition not met");
            }
            await Task.Delay(10);
        }
    }
}
