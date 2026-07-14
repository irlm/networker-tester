using Networker.ControlPlane.Background;
using static Networker.ControlPlane.Background.InactivityService;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pure threshold math of the workspace-inactivity lifecycle
/// (warn @90d → suspend @120d → notice @360d → hard-delete @365d), mirroring
/// the Rust scheduler's <c>check_workspace_inactivity</c> ladder.
/// </summary>
public class InactivityPolicyTests
{
    private static readonly DateTime Now = new(2026, 7, 14, 12, 0, 0, DateTimeKind.Utc);

    private static DateTime DaysAgo(double days) => Now.AddDays(-days);

    // ── DecideLiveAction ───────────────────────────────────────────────────

    [Theory]
    [InlineData(0)]
    [InlineData(30)]
    [InlineData(89.9)]
    public void Active_workspace_gets_no_action(double inactiveDays)
    {
        Assert.Equal(LifecycleAction.None,
            DecideLiveAction(DaysAgo(inactiveDays), warningSentAtUtc: null, Now));
    }

    [Theory]
    [InlineData(90)]
    [InlineData(120)]
    [InlineData(400)]
    public void Inactive_unwarned_workspace_gets_warned_first(double inactiveDays)
    {
        // Even far past the suspend threshold, an unwarned workspace must get
        // the 90d warning first — never a surprise suspension.
        Assert.Equal(LifecycleAction.Warn,
            DecideLiveAction(DaysAgo(inactiveDays), warningSentAtUtc: null, Now));
    }

    [Fact]
    public void Warned_but_below_120d_is_not_suspended()
    {
        Assert.Equal(LifecycleAction.None,
            DecideLiveAction(DaysAgo(100), warningSentAtUtc: DaysAgo(10), Now));
    }

    [Fact]
    public void Suspends_at_120d_when_warning_is_30d_old()
    {
        Assert.Equal(LifecycleAction.Suspend,
            DecideLiveAction(DaysAgo(120), warningSentAtUtc: DaysAgo(30), Now));
    }

    [Fact]
    public void Fresh_warning_defers_suspension_even_past_120d()
    {
        // Warning went out yesterday (e.g. service was down for months): the
        // 30-day grace still applies, matching Rust warnings_older_than(.., 30).
        Assert.Equal(LifecycleAction.None,
            DecideLiveAction(DaysAgo(200), warningSentAtUtc: DaysAgo(1), Now));
    }

    [Fact]
    public void Long_suspended_candidates_suspend_once_grace_elapsed()
    {
        Assert.Equal(LifecycleAction.Suspend,
            DecideLiveAction(DaysAgo(200), warningSentAtUtc: DaysAgo(31), Now));
    }

    // ── DecideSuspendedAction ──────────────────────────────────────────────

    [Theory]
    [InlineData(1)]
    [InlineData(359.9)]
    public void Recently_suspended_workspace_is_left_alone(double suspendedDays)
    {
        Assert.Equal(SuspendedAction.None,
            DecideSuspendedAction(DaysAgo(suspendedDays), hasDeleteNotice: false, Now));
    }

    [Fact]
    public void Notice_fires_at_360d_once()
    {
        Assert.Equal(SuspendedAction.NoticeHardDelete,
            DecideSuspendedAction(DaysAgo(360), hasDeleteNotice: false, Now));
        Assert.Equal(SuspendedAction.None,
            DecideSuspendedAction(DaysAgo(360), hasDeleteNotice: true, Now));
    }

    [Theory]
    [InlineData(365)]
    [InlineData(1000)]
    public void Hard_delete_fires_at_365d(double suspendedDays)
    {
        // Delete wins regardless of whether the notice was recorded.
        Assert.Equal(SuspendedAction.HardDelete,
            DecideSuspendedAction(DaysAgo(suspendedDays), hasDeleteNotice: true, Now));
        Assert.Equal(SuspendedAction.HardDelete,
            DecideSuspendedAction(DaysAgo(suspendedDays), hasDeleteNotice: false, Now));
    }

    // ── EffectiveLastActivity ──────────────────────────────────────────────

    [Fact]
    public void Last_activity_is_the_max_of_updated_at_and_last_run()
    {
        var older = DaysAgo(200);
        var newer = DaysAgo(10);

        Assert.Equal(newer, EffectiveLastActivity(older, newer));
        Assert.Equal(newer, EffectiveLastActivity(newer, older));
        Assert.Equal(newer, EffectiveLastActivity(newer, null));
    }

    [Fact]
    public void Recent_run_keeps_a_stale_project_alive()
    {
        // project.updated_at is a year old but a run happened last week.
        var action = DecideLiveAction(
            EffectiveLastActivity(DaysAgo(365), DaysAgo(7)), warningSentAtUtc: null, Now);
        Assert.Equal(LifecycleAction.None, action);
    }

    [Fact]
    public void Threshold_constants_match_the_rust_ladder()
    {
        Assert.Equal(90, WarnAfterDays);
        Assert.Equal(120, SuspendAfterDays);
        Assert.Equal(360, HardDeleteNoticeAfterDays);
        Assert.Equal(365, HardDeleteAfterDays);
        Assert.Equal("inactivity_90d", InactivityWarningType);
        Assert.Equal("hard_delete_5d", HardDeleteNoticeType);
        Assert.Equal(TimeSpan.FromHours(24), TickInterval);
    }
}
