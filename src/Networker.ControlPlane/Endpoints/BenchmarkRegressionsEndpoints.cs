using System.Text.Json.Serialization;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Read endpoints for detected benchmark regressions — the rows
/// <see cref="Provisioning.BenchmarkRegressionDetector"/> writes on run
/// completion (V047 <c>benchmark_regression</c>) — plus the comparison-activity
/// summary and the pin/unpin-baseline actions (#810). Serves the dashboard's
/// Benchmark Regressions page; snake_case wire shape.
/// </summary>
public static class BenchmarkRegressionsEndpoints
{
    private const int DefaultLimit = 100;
    private const int MaxLimit = 500;

    public static IEndpointRouteBuilder MapBenchmarkRegressionsEndpoints(
        this IEndpointRouteBuilder app)
    {
        // GET /api/projects/{projectId}/benchmark-regressions?limit= — newest
        // first, joined to the config for its display name (member).
        app.MapGet("/api/projects/{projectId}/benchmark-regressions", async (
            string projectId,
            int? limit,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var take = Math.Clamp(limit ?? DefaultLimit, 1, MaxLimit);

            var rows = await db.BenchmarkRegressions
                .AsNoTracking()
                .Where(r => r.TestConfig.ProjectId == projectId)
                .OrderByDescending(r => r.DetectedAt)
                .ThenByDescending(r => r.RegressionId)
                .Take(take)
                .Select(r => new
                {
                    regression_id = r.RegressionId,
                    config_id = r.TestConfigId,
                    config_name = r.TestConfig.Name,
                    run_id = r.TestRunId,
                    baseline_run_id = r.BaselineRunId,
                    case_id = r.CaseId,
                    metric = r.Metric,
                    metric_unit = r.MetricUnit,
                    baseline_value = r.BaselineValue,
                    current_value = r.CurrentValue,
                    delta_percent = r.DeltaPercent,
                    severity = r.Severity,
                    detected_at = r.DetectedAt,
                })
                .ToListAsync(ct);

            return Results.Ok(rows);
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        // GET /api/projects/{projectId}/benchmark-regressions/summary —
        // comparison-activity observability (#810): lets the page distinguish
        // "detection has never compared anything" from "comparisons ran, zero
        // regressions". Derived entirely from existing tables (the detector
        // keeps no counter) — see ComputeSummaryAsync for the exact semantics,
        // which the response's `semantics` field restates on the wire.
        app.MapGet("/api/projects/{projectId}/benchmark-regressions/summary", async (
            string projectId,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var summary = await ComputeSummaryAsync(db, projectId, ct);
            return Results.Ok(summary);
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        // POST /api/v2/test-runs/{id}/pin-baseline — set the run's config
        // baseline_run_id to this run (flat route; Operator on the run's
        // project, 404 on absent/no-access — no existence oracle). Lives here,
        // next to the detector's read side, rather than in the general config
        // PATCH: the pin is only valid for a run the detector could actually
        // compare against (completed + artifact), which the PATCH cannot know.
        app.MapPost("/api/v2/test-runs/{id:guid}/pin-baseline", async (
            Guid id,
            HttpContext ctx,
            NetworkerDbContext db,
            ProjectAccessChecker access,
            CancellationToken ct) =>
        {
            var projectId = await RunProjectIdAsync(db, id, ct);
            if (projectId is null
                || !await access.HasRoleAsync(ctx, projectId, ProjectRole.Operator, ct))
            {
                return Results.NotFound();
            }

            var (result, dto) = await PinBaselineAsync(db, id, ct);
            return result switch
            {
                PinBaselineOutcome.Pinned => Results.Ok(dto),
                PinBaselineOutcome.NotComparable => ApiError.BadRequest(
                    "only a completed run with a benchmark artifact can be pinned as baseline"),
                _ => Results.NotFound(),
            };
        }).RequireAuthorization();

        // DELETE /api/v2/test-runs/{id}/pin-baseline — clear the config's
        // pinned baseline, but only when this run IS the pinned baseline
        // (unpinning from a run that is not pinned is a no-op 409, so two
        // operators cannot silently clobber each other's pins).
        app.MapDelete("/api/v2/test-runs/{id:guid}/pin-baseline", async (
            Guid id,
            HttpContext ctx,
            NetworkerDbContext db,
            ProjectAccessChecker access,
            CancellationToken ct) =>
        {
            var projectId = await RunProjectIdAsync(db, id, ct);
            if (projectId is null
                || !await access.HasRoleAsync(ctx, projectId, ProjectRole.Operator, ct))
            {
                return Results.NotFound();
            }

            var (result, dto) = await UnpinBaselineAsync(db, id, ct);
            return result switch
            {
                PinBaselineOutcome.Unpinned => Results.Ok(dto),
                PinBaselineOutcome.NotPinned => ApiError.Conflict(
                    "this run is not the config's pinned baseline"),
                _ => Results.NotFound(),
            };
        }).RequireAuthorization();

        return app;
    }

    // ── Summary (derived — see field docs on BenchmarkRegressionSummary) ────

    /// <summary>
    /// Comparison-activity summary for a project, derived from existing
    /// tables only (no counter, no migration):
    ///
    /// <list type="bullet">
    /// <item><c>runs_compared</c> — completed artifact-bearing runs that had at
    /// least one prior completed artifact-bearing run of the same config, i.e.
    /// per config <c>max(n − 1, 0)</c>. This mirrors the detector's own
    /// eligibility gate (completed + artifact + a resolvable baseline), so it
    /// is exact for the previous-run pathway; for pinned baselines it is an
    /// approximation (the pinned run is assumed to be one of the config's own
    /// runs), and runs completed before the detector existed (or while
    /// artifacts were stubs, pre-v0.28.239) are counted even though their
    /// comparison was vacuous.</item>
    /// <item><c>comparable_configs</c> — configs whose NEXT completed run will
    /// be compared: ≥2 completed artifact-bearing runs, or a pinned
    /// <c>baseline_run_id</c> with ≥1 such run.</item>
    /// <item><c>last_comparison_at</c> — finish time of the newest run counted
    /// in <c>runs_compared</c>.</item>
    /// <item><c>total_regressions</c>/<c>last_regression_at</c> — actual
    /// breach rows, exact (V047 table).</item>
    /// </list>
    /// </summary>
    public static async Task<BenchmarkRegressionSummary> ComputeSummaryAsync(
        NetworkerDbContext db, string projectId, CancellationToken ct)
    {
        // Per-config eligible-run counts. "Eligible" is exactly the detector's
        // gate: completed with a benchmark artifact.
        var perConfig = await db.TestRuns
            .AsNoTracking()
            .Where(r => r.ProjectId == projectId
                && r.Status == "completed"
                && r.ArtifactId != null)
            .GroupBy(r => r.TestConfigId)
            .Select(g => new
            {
                ConfigId = g.Key,
                Runs = g.Count(),
                LastRunAt = g.Max(r => r.FinishedAt ?? r.CreatedAt),
            })
            .ToListAsync(ct);

        var pinnedConfigIds = await db.TestConfigs
            .AsNoTracking()
            .Where(c => c.ProjectId == projectId && c.BaselineRunId != null)
            .Select(c => c.Id)
            .ToListAsync(ct);
        var pinned = pinnedConfigIds.ToHashSet();

        var comparableConfigs = perConfig
            .Count(p => p.Runs >= 2 || (p.Runs >= 1 && pinned.Contains(p.ConfigId)));
        var runsCompared = perConfig.Sum(p => Math.Max(p.Runs - 1, 0));
        var lastComparisonAt = perConfig
            .Where(p => p.Runs >= 2)
            .Select(p => (DateTime?)p.LastRunAt)
            .DefaultIfEmpty(null)
            .Max();

        var regressionStats = await db.BenchmarkRegressions
            .AsNoTracking()
            .Where(r => r.TestConfig.ProjectId == projectId)
            .GroupBy(_ => 1)
            .Select(g => new { Count = g.Count(), Last = (DateTime?)g.Max(r => r.DetectedAt) })
            .FirstOrDefaultAsync(ct);

        return new BenchmarkRegressionSummary(
            ComparableConfigs: comparableConfigs,
            PinnedBaselineConfigs: pinnedConfigIds.Count,
            RunsCompared: runsCompared,
            LastComparisonAt: lastComparisonAt,
            TotalRegressions: regressionStats?.Count ?? 0,
            LastRegressionAt: regressionStats?.Last,
            Semantics: SummarySemantics);
    }

    /// <summary>
    /// Honest wire-level statement of what the derived numbers mean —
    /// runs_compared is reconstructed from run history, not a stored counter.
    /// </summary>
    public const string SummarySemantics =
        "runs_compared is derived from run history (completed artifact-bearing runs "
        + "preceded by another completed artifact-bearing run of the same config — the "
        + "detector's own eligibility gate), not a stored counter: exact for the "
        + "previous-run pathway, approximate for pinned baselines, and it includes "
        + "runs whose comparison predates the current detector. total_regressions "
        + "and last_regression_at are exact.";

    // ── Pin/unpin baseline (#810) ────────────────────────────────────────────

    public enum PinBaselineOutcome
    {
        Pinned,
        Unpinned,
        /// <summary>Run exists but is not completed-with-artifact (pin only).</summary>
        NotComparable,
        /// <summary>Run exists but is not the config's current pin (unpin only).</summary>
        NotPinned,
        NotFound,
    }

    /// <summary>The run's project id, or null when the run does not exist.</summary>
    private static async Task<string?> RunProjectIdAsync(
        NetworkerDbContext db, Guid runId, CancellationToken ct) =>
        await db.TestRuns.AsNoTracking()
            .Where(r => r.Id == runId)
            .Select(r => r.ProjectId)
            .FirstOrDefaultAsync(ct);

    /// <summary>
    /// Pin <paramref name="runId"/> as its config's <c>baseline_run_id</c>.
    /// Only a run the detector could actually use is accepted (completed +
    /// artifact) — pinning anything else would silently disable detection for
    /// the config (the detector treats an unusable explicit pin as a no-op,
    /// not a fallback).
    /// </summary>
    public static async Task<(PinBaselineOutcome, PinBaselineDto?)> PinBaselineAsync(
        NetworkerDbContext db, Guid runId, CancellationToken ct)
    {
        var run = await db.TestRuns.AsNoTracking()
            .Where(r => r.Id == runId)
            .Select(r => new { r.TestConfigId, r.Status, r.ArtifactId })
            .FirstOrDefaultAsync(ct);
        if (run is null)
        {
            return (PinBaselineOutcome.NotFound, null);
        }
        if (run.Status != "completed" || run.ArtifactId is null)
        {
            return (PinBaselineOutcome.NotComparable, null);
        }

        var cfg = await db.TestConfigs.FirstOrDefaultAsync(c => c.Id == run.TestConfigId, ct);
        if (cfg is null)
        {
            return (PinBaselineOutcome.NotFound, null);
        }

        cfg.BaselineRunId = runId;
        cfg.UpdatedAt = DateTime.UtcNow;
        await db.SaveChangesAsync(ct);

        return (PinBaselineOutcome.Pinned, new PinBaselineDto(cfg.Id, runId));
    }

    /// <summary>
    /// Clear the config's pin, but only when <paramref name="runId"/> IS the
    /// current pin — detection then falls back to previous-run baselines.
    /// </summary>
    public static async Task<(PinBaselineOutcome, PinBaselineDto?)> UnpinBaselineAsync(
        NetworkerDbContext db, Guid runId, CancellationToken ct)
    {
        var configId = await db.TestRuns.AsNoTracking()
            .Where(r => r.Id == runId)
            .Select(r => (Guid?)r.TestConfigId)
            .FirstOrDefaultAsync(ct);
        if (configId is null)
        {
            return (PinBaselineOutcome.NotFound, null);
        }

        var cfg = await db.TestConfigs.FirstOrDefaultAsync(c => c.Id == configId, ct);
        if (cfg is null)
        {
            return (PinBaselineOutcome.NotFound, null);
        }
        if (cfg.BaselineRunId != runId)
        {
            return (PinBaselineOutcome.NotPinned, null);
        }

        cfg.BaselineRunId = null;
        cfg.UpdatedAt = DateTime.UtcNow;
        await db.SaveChangesAsync(ct);

        return (PinBaselineOutcome.Unpinned, new PinBaselineDto(cfg.Id, null));
    }
}

/// <summary>
/// Wire shape of <c>GET /api/projects/{id}/benchmark-regressions/summary</c>.
/// Field semantics: see
/// <see cref="BenchmarkRegressionsEndpoints.ComputeSummaryAsync"/>; the
/// <c>semantics</c> field carries the same statement on the wire so consumers
/// never mistake the derived counts for stored counters.
/// </summary>
public sealed record BenchmarkRegressionSummary(
    [property: JsonPropertyName("comparable_configs")] int ComparableConfigs,
    [property: JsonPropertyName("pinned_baseline_configs")] int PinnedBaselineConfigs,
    [property: JsonPropertyName("runs_compared")] int RunsCompared,
    [property: JsonPropertyName("last_comparison_at")] DateTime? LastComparisonAt,
    [property: JsonPropertyName("total_regressions")] int TotalRegressions,
    [property: JsonPropertyName("last_regression_at")] DateTime? LastRegressionAt,
    [property: JsonPropertyName("semantics")] string Semantics);

/// <summary>Wire shape of the pin/unpin-baseline responses.</summary>
public sealed record PinBaselineDto(
    [property: JsonPropertyName("config_id")] Guid ConfigId,
    [property: JsonPropertyName("baseline_run_id")] Guid? BaselineRunId);
