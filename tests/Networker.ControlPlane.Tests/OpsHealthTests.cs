using Networker.ControlPlane.Background;
using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// The staleness math behind GET /api/health/background: healthy = ticked
/// within 3× the service's expected interval, with the loop start standing in
/// for a first tick that hasn't happened yet.
/// </summary>
public class OpsHealthTests
{
    private static readonly DateTimeOffset Now = new(2026, 7, 14, 12, 0, 0, TimeSpan.Zero);

    private static ServiceTickSnapshot Snapshot(
        string service = OpsServiceNames.Watchdog,
        DateTimeOffset? startedAt = null,
        DateTimeOffset? lastTickAt = null,
        long ticksTotal = 0) =>
        new(service, startedAt ?? Now.AddHours(-1), lastTickAt, ticksTotal, 0, null, null, null);

    // ── seconds_since_tick ──────────────────────────────────────────────────

    [Fact]
    public void SecondsSinceTick_uses_last_tick_when_present()
    {
        var s = Snapshot(lastTickAt: Now.AddSeconds(-45), ticksTotal: 3);
        Assert.Equal(45, OpsEndpoints.SecondsSinceTick(s, Now));
    }

    [Fact]
    public void SecondsSinceTick_falls_back_to_started_at_when_never_ticked()
    {
        var s = Snapshot(startedAt: Now.AddSeconds(-90));
        Assert.Equal(90, OpsEndpoints.SecondsSinceTick(s, Now));
    }

    [Fact]
    public void SecondsSinceTick_clamps_clock_skew_to_zero()
    {
        var s = Snapshot(lastTickAt: Now.AddSeconds(5), ticksTotal: 1);
        Assert.Equal(0, OpsEndpoints.SecondsSinceTick(s, Now));
    }

    // ── healthy threshold: 3× expected interval ─────────────────────────────

    [Theory]
    [InlineData(0, true)]
    [InlineData(60, true)]
    [InlineData(180, true)]   // exactly 3× — still healthy (inclusive)
    [InlineData(181, false)]  // one second past 3× — unhealthy
    [InlineData(3600, false)]
    public void IsHealthy_threshold_is_three_times_expected_interval(int ageSecs, bool expected)
    {
        var s = Snapshot(lastTickAt: Now.AddSeconds(-ageSecs), ticksTotal: 1);
        Assert.Equal(expected, OpsEndpoints.IsHealthy(s, TimeSpan.FromSeconds(60), Now));
    }

    [Fact]
    public void Never_ticked_service_is_healthy_within_startup_grace()
    {
        // Watchdog started 30s ago, hasn't completed a tick yet (first tick at
        // 60s): still healthy — the loop start anchors the staleness window.
        var s = Snapshot(startedAt: Now.AddSeconds(-30));
        Assert.True(OpsEndpoints.IsHealthy(s, TimeSpan.FromSeconds(60), Now));
    }

    [Fact]
    public void Never_ticked_service_ages_into_unhealthy()
    {
        // Wedged from birth: started 10 minutes ago, zero ticks on a 60s loop.
        var s = Snapshot(startedAt: Now.AddMinutes(-10));
        Assert.False(OpsEndpoints.IsHealthy(s, TimeSpan.FromSeconds(60), Now));
    }

    // ── expected-interval map ───────────────────────────────────────────────

    [Fact]
    public void Every_known_service_has_an_expected_interval()
    {
        foreach (var service in OpsServiceNames.All)
        {
            Assert.True(
                OpsEndpoints.ExpectedIntervals.ContainsKey(service),
                $"OpsEndpoints.ExpectedIntervals is missing '{service}'");
        }
    }

    [Fact]
    public void Expected_interval_map_matches_the_service_tick_constants()
    {
        // Hand-synced mirror of each service's private TickInterval — if a
        // cadence changes, this test forces the health map to follow.
        Assert.Equal(TimeSpan.FromSeconds(30), OpsEndpoints.ExpectedIntervalFor(OpsServiceNames.Scheduler));
        Assert.Equal(TimeSpan.FromSeconds(30), OpsEndpoints.ExpectedIntervalFor(OpsServiceNames.QueuedRedispatch));
        Assert.Equal(TimeSpan.FromSeconds(60), OpsEndpoints.ExpectedIntervalFor(OpsServiceNames.Watchdog));
        Assert.Equal(TimeSpan.FromSeconds(60), OpsEndpoints.ExpectedIntervalFor(OpsServiceNames.AgentReaper));
        Assert.Equal(TimeSpan.FromSeconds(60), OpsEndpoints.ExpectedIntervalFor(OpsServiceNames.AutoShutdown));
        Assert.Equal(TimeSpan.FromMinutes(10), OpsEndpoints.ExpectedIntervalFor(OpsServiceNames.OrphanReaper));
        Assert.Equal(TimeSpan.FromHours(24), OpsEndpoints.ExpectedIntervalFor(OpsServiceNames.WorkspaceInactivity));
        Assert.Equal(TimeSpan.FromSeconds(5), OpsEndpoints.ExpectedIntervalFor(OpsServiceNames.ProvisioningOrchestrator));
    }

    [Fact]
    public void Unknown_service_gets_the_generous_default_interval()
        => Assert.Equal(OpsEndpoints.DefaultExpectedInterval, OpsEndpoints.ExpectedIntervalFor("some-new-loop"));

    [Fact]
    public void Inactivity_service_first_pass_delay_is_within_grace()
    {
        // The 24h loop takes its first pass 5 minutes after boot; with 3×24h of
        // grace anchored on StartedAt it must never alarm during startup.
        var s = Snapshot(OpsServiceNames.WorkspaceInactivity, startedAt: Now.AddMinutes(-6));
        Assert.True(OpsEndpoints.IsHealthy(
            s, OpsEndpoints.ExpectedIntervalFor(OpsServiceNames.WorkspaceInactivity), Now));
    }
}
