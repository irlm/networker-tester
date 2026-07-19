using Networker.ControlPlane.Alerting;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// The pure alert-evaluation core: comparator semantics, window decisions,
/// firing/resolved state transitions, and count-based rate metrics. These are
/// the rules that decide whether anyone gets paged — every branch is pinned.
/// </summary>
public sealed class AlertRuleLogicTests
{
    // ── Comparators ──────────────────────────────────────────────────────────

    [Theory]
    [InlineData("gt", 501.0, 500.0, true)]
    [InlineData("gt", 500.0, 500.0, false)] // strict: at-threshold is NOT a breach
    [InlineData("gt", 499.0, 500.0, false)]
    [InlineData("lt", 0.98, 0.99, true)]
    [InlineData("lt", 0.99, 0.99, false)] // strict
    [InlineData("lt", 1.00, 0.99, false)]
    [InlineData("eq", 500.0, 500.0, false)] // unknown comparator never breaches
    public void Breaches_is_strict_and_rejects_unknown_comparators(
        string comparator, double value, double threshold, bool expected)
    {
        Assert.Equal(expected, AlertRuleLogic.Breaches(comparator, value, threshold));
    }

    // ── Window ───────────────────────────────────────────────────────────────

    [Fact]
    public void Window_of_one_fires_on_a_single_breach()
    {
        Assert.True(AlertRuleLogic.WindowBreached([600.0], 1, "gt", 500));
        Assert.False(AlertRuleLogic.WindowBreached([400.0], 1, "gt", 500));
    }

    [Fact]
    public void Window_requires_all_n_newest_runs_to_breach()
    {
        // Newest-first: current run 600, previous 700, before that 800.
        Assert.True(AlertRuleLogic.WindowBreached([600.0, 700.0, 800.0], 3, "gt", 500));

        // One healthy run inside the window breaks the streak.
        Assert.False(AlertRuleLogic.WindowBreached([600.0, 400.0, 800.0], 3, "gt", 500));

        // Breaches OUTSIDE the window are ignored (only the newest 2 count).
        Assert.True(AlertRuleLogic.WindowBreached([600.0, 700.0, 100.0], 2, "gt", 500));
    }

    [Fact]
    public void Window_with_fewer_runs_than_required_never_fires()
    {
        // Only 2 terminal runs exist but the rule wants 3 consecutive breaches.
        Assert.False(AlertRuleLogic.WindowBreached([600.0, 700.0], 3, "gt", 500));
        Assert.False(AlertRuleLogic.WindowBreached([], 1, "gt", 500));
    }

    [Fact]
    public void Null_metric_values_break_the_streak()
    {
        // A run with no data inside the window prevents firing — missing data
        // is never treated as a breach.
        Assert.False(AlertRuleLogic.WindowBreached([600.0, null, 800.0], 3, "gt", 500));
        Assert.False(AlertRuleLogic.WindowBreached([null], 1, "gt", 500));
    }

    [Fact]
    public void Window_below_minimum_never_fires()
    {
        Assert.False(AlertRuleLogic.WindowBreached([600.0], 0, "gt", 500));
    }

    // ── State transitions (dedup) ────────────────────────────────────────────

    [Fact]
    public void Quiet_rule_fires_on_breach()
    {
        Assert.Equal("firing", AlertRuleLogic.NextTransition(currentlyFiring: false, windowBreached: true));
    }

    [Fact]
    public void Firing_rule_does_not_refire_while_still_breaching()
    {
        Assert.Null(AlertRuleLogic.NextTransition(currentlyFiring: true, windowBreached: true));
    }

    [Fact]
    public void Firing_rule_resolves_when_back_under_threshold()
    {
        Assert.Equal("resolved", AlertRuleLogic.NextTransition(currentlyFiring: true, windowBreached: false));
    }

    [Fact]
    public void Quiet_rule_stays_quiet_without_breach()
    {
        Assert.Null(AlertRuleLogic.NextTransition(currentlyFiring: false, windowBreached: false));
    }

    // ── Rate metrics from run counts ─────────────────────────────────────────

    [Theory]
    [InlineData(9, 1, 0.9)]
    [InlineData(10, 0, 1.0)]
    [InlineData(0, 10, 0.0)]
    public void Success_rate_from_counts(int success, int failure, double expected)
    {
        Assert.Equal(expected, AlertRuleLogic.SuccessRate(success, failure)!.Value, precision: 10);
    }

    [Theory]
    [InlineData(9, 1, 0.1)]
    [InlineData(10, 0, 0.0)]
    [InlineData(0, 10, 1.0)]
    public void Error_rate_from_counts(int success, int failure, double expected)
    {
        Assert.Equal(expected, AlertRuleLogic.ErrorRate(success, failure)!.Value, precision: 10);
    }

    [Fact]
    public void Zero_attempts_yield_null_rates_not_zero()
    {
        // No data ≠ 0% error rate — a run that recorded nothing must not
        // resolve (or breach) a rate rule.
        Assert.Null(AlertRuleLogic.SuccessRate(0, 0));
        Assert.Null(AlertRuleLogic.ErrorRate(0, 0));
    }

    // ── Vocabulary / messages ────────────────────────────────────────────────

    [Fact]
    public void Metric_and_comparator_vocabularies_match_the_v041_checks()
    {
        Assert.Equal(
            new[] { "error_rate", "mean_ms", "p95_ms", "success_rate" },
            AlertRuleLogic.Metrics.OrderBy(m => m, StringComparer.Ordinal));
        Assert.Equal(
            new[] { "gt", "lt" },
            AlertRuleLogic.Comparators.OrderBy(c => c, StringComparer.Ordinal));
    }

    [Fact]
    public void Messages_are_invariant_culture_and_state_specific()
    {
        Assert.Equal(
            "p95_ms 812.35 > 500 for 3 consecutive run(s)",
            AlertRuleLogic.BuildMessage("firing", "p95_ms", "gt", 500, 812.35, 3));
        Assert.Equal(
            "success_rate 0.999 back within threshold 0.99 (<)",
            AlertRuleLogic.BuildMessage("resolved", "success_rate", "lt", 0.99, 0.999, 1));
    }
}
