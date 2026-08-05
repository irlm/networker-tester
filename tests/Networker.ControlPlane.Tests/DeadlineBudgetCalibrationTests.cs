using Networker.ControlPlane.Endpoints;
using Xunit;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Audit P2: the cell-deadline budget is calibrated from a real incident, but
/// the evidence lived only in a code comment.
///
/// <para><b>What actually happened.</b> A fixed 900s deadline killed every
/// matrix cell at ~16 minutes (2026-08-03). The replacement budgeted 4s per
/// (run × mode); on 2026-08-04, with four cells sharing one runner, the slower
/// proxies (haproxy, traefik) still needed more than 4.2s per unit and were
/// killed at 78-85% complete, while nginx and caddy squeaked through. The
/// constant became 8s.</para>
///
/// <para>A number justified by an incident is only safe while the incident is
/// remembered. These tests turn those observations into assertions, so
/// lowering the constant back toward the failure fails here rather than on a
/// six-hour matrix run. <c>scripts/deadline-calibration.sql</c> is the other
/// half: it re-derives the distribution from real completed runs, so the
/// constant can be checked against production instead of against memory.</para>
/// </summary>
public class DeadlineBudgetCalibrationTests
{
    // ── The measured facts (2026-08-04, four cells on one runner) ────────────

    /// <summary>Worst per-unit rate actually observed, in seconds.</summary>
    private const double ObservedWorstSecondsPerUnit = 4.2;

    /// <summary>The 4s budget expired with cells this far along, so the real
    /// requirement is at least worst-rate ÷ this fraction.</summary>
    private const double ProgressWhenKilled = 0.78;

    /// <summary>Minimum acceptable headroom over the measured requirement.
    /// A deadline that merely matches observation has a 50% chance of killing
    /// the next slightly-slower run.</summary>
    private const double RequiredSafetyFactor = 1.3;

    /// <summary>Seconds per unit implied by the current formula, recovered by
    /// measuring the model rather than restating the constant — if someone
    /// changes the arithmetic instead of the constant, this still tracks it.</summary>
    private static double ImpliedSecondsPerUnit()
    {
        // Large enough that the +600s fixed buffer and the floor don't distort
        // the slope, small enough to stay under the 8h cap.
        const int runs = 100;
        const int modes = 10;
        var withBuffer = ComparisonGroupsEndpoints.CellMaxDurationSecs(
            $$"""{"runs":{{runs}},"modes":[{{string.Join(",", Enumerable.Range(0, modes).Select(i => $"\"m{i}\""))}}]}""");
        return (withBuffer - 600.0) / (runs * modes);
    }

    [Fact]
    public void The_budget_clears_the_measured_worst_case_with_headroom()
    {
        // 4.2s/unit observed at 78% complete ⇒ the full run needed ≈5.4s/unit.
        var requiredSecondsPerUnit = ObservedWorstSecondsPerUnit / ProgressWhenKilled;
        var minimumAcceptable = requiredSecondsPerUnit * RequiredSafetyFactor;
        var actual = ImpliedSecondsPerUnit();

        Assert.True(actual >= minimumAcceptable,
            $"the cell deadline budgets {actual:F1}s per (run × mode), but the 2026-08-04 "
            + $"measurement implies ≥{requiredSecondsPerUnit:F1}s was needed and a "
            + $"{RequiredSafetyFactor:F1}x safety factor puts the floor at "
            + $"{minimumAcceptable:F1}s. Lowering it re-creates the failure where haproxy and "
            + "traefik cells were killed at 78-85% complete after hours of work. "
            + "Re-derive from production first: scripts/deadline-calibration.sql.");
    }

    [Fact]
    public void The_budget_is_not_absurdly_generous()
    {
        // The opposite failure: a deadline so loose it never fires lets a
        // genuinely stuck cell hold a runner (and its cloud bill) for hours.
        var actual = ImpliedSecondsPerUnit();
        Assert.True(actual <= 30.0,
            $"{actual:F1}s per unit is far past anything measured — a wedged cell would sit on "
            + "its runner for hours before the deadline noticed.");
    }

    [Theory]
    // The exact workload that was being killed, with its post-fix estimate.
    [InlineData(100, 26, 21400)]
    // A small workload must still get the historical floor, not a tiny deadline.
    [InlineData(10, 2, 900)]
    // A single-mode smoke run — floor again.
    [InlineData(1, 1, 900)]
    public void Known_workloads_get_their_calibrated_deadline(int runs, int modes, int expected)
    {
        var json = $$"""{"runs":{{runs}},"modes":[{{string.Join(",", Enumerable.Range(0, modes).Select(i => $"\"m{i}\""))}}]}""";
        Assert.Equal(expected, ComparisonGroupsEndpoints.CellMaxDurationSecs(json));
    }

    [Fact]
    public void The_full_matrix_workload_fits_under_the_cap()
    {
        // The cap exists to stop a malformed workload producing an unbounded
        // run — but if the LEGITIMATE worst case is clamped by it, cells die at
        // the cap instead of finishing, which is the original bug wearing a
        // different number. 100 runs × 26 modes is the real full matrix.
        var json = $$"""{"runs":100,"modes":[{{string.Join(",", Enumerable.Range(0, 26).Select(i => $"\"m{i}\""))}}]}""";
        var secs = ComparisonGroupsEndpoints.CellMaxDurationSecs(json);

        Assert.True(secs < 8 * 3600,
            $"the full matrix workload estimates {secs}s, which the 8h cap clamps — the "
            + "largest legitimate workload must fit under the cap or it gets killed mid-run.");
    }

    [Fact]
    public void The_calibration_query_ships_with_the_repo()
    {
        // The assertions above encode a snapshot. The query is how it gets
        // re-derived; if it disappears, the constants become folklore again.
        var dir = new DirectoryInfo(AppContext.BaseDirectory);
        while (dir is not null && !Directory.Exists(Path.Combine(dir.FullName, "scripts")))
        {
            dir = dir.Parent;
        }
        Assert.True(dir is not null, "could not locate the repository root");

        var path = Path.Combine(dir!.FullName, "scripts", "deadline-calibration.sql");
        Assert.True(File.Exists(path),
            "scripts/deadline-calibration.sql is missing — without it these constants can only "
            + "be re-derived by repeating the incident that produced them.");

        var sql = File.ReadAllText(path);
        Assert.Contains("seconds_per_unit", sql, StringComparison.Ordinal);
        // Runs killed by a deadline must stay excluded, or each recalibration
        // ratchets the budget down toward the failure it exists to prevent.
        Assert.Contains("deadline", sql, StringComparison.OrdinalIgnoreCase);
    }
}
