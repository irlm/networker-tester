using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Realtime;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// Runs the <see cref="RegressionAnalyzer"/> policy when a benchmark run
/// completes, persists breaches as <c>benchmark_regression</c> rows (served by
/// <c>GET /api/projects/{projectId}/benchmark-regressions</c>) and publishes
/// the <see cref="BenchmarkRegression"/> live event through
/// <see cref="RegressionAnalyzer.EmitRegressionEvent"/>.
///
/// <para><b>Hook point.</b> Called from
/// <see cref="Realtime.RawWs.AgentMessageProcessor"/> after a completed
/// <c>run_finished</c> frame's artifact is persisted — the same best-effort
/// contract as <see cref="Alerting.AlertEvaluator"/>: any failure is logged
/// and swallowed, never failing run processing.</para>
///
/// <para><b>Baseline resolution.</b> The config's pinned
/// <c>baseline_run_id</c> when set (and not the triggering run itself),
/// otherwise the most recent prior completed run of the same config that has
/// an artifact. First run of a config → no baseline → nothing to compare,
/// nothing flagged.</para>
/// </summary>
public sealed class BenchmarkRegressionDetector(
    NetworkerDbContext db,
    EventBus bus,
    ILogger<BenchmarkRegressionDetector> logger)
{
    /// <summary>Detect and persist regressions for a completed run. Never throws.</summary>
    public async Task DetectAsync(Guid runId, CancellationToken ct = default)
    {
        try
        {
            await DetectCoreAsync(runId, ct);
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            // Shutdown/disconnect — nothing to log.
        }
        catch (Exception ex)
        {
            logger.LogError(ex, "Regression detection failed for run {RunId} (non-fatal)", runId);
        }
    }

    private async Task DetectCoreAsync(Guid runId, CancellationToken ct)
    {
        var run = await db.TestRuns
            .AsNoTracking()
            .Where(r => r.Id == runId && r.Status == "completed" && r.ArtifactId != null)
            .Select(r => new { r.Id, r.TestConfigId, r.ArtifactId, r.CreatedAt })
            .FirstOrDefaultAsync(ct);

        if (run is null)
        {
            return; // not a completed benchmark run — nothing to analyze
        }

        // Idempotence: a duplicate/late invocation must not double-flag.
        if (await db.BenchmarkRegressions.AnyAsync(r => r.TestRunId == runId, ct))
        {
            return;
        }

        var config = await db.TestConfigs
            .AsNoTracking()
            .Where(c => c.Id == run.TestConfigId)
            .Select(c => new { c.Id, c.Name, c.BaselineRunId })
            .FirstOrDefaultAsync(ct);

        if (config is null)
        {
            return;
        }

        // Baseline: pinned baseline_run_id first, else previous completed run
        // of the same config with an artifact.
        Guid? baselineRunId = null;
        Guid? baselineArtifactId = null;
        if (config.BaselineRunId is { } pinned && pinned != runId)
        {
            var pinnedRun = await db.TestRuns
                .AsNoTracking()
                .Where(r => r.Id == pinned && r.Status == "completed" && r.ArtifactId != null)
                .Select(r => new { r.Id, r.ArtifactId })
                .FirstOrDefaultAsync(ct);
            if (pinnedRun is null)
            {
                logger.LogDebug(
                    "Run {RunId}: pinned baseline {BaselineRunId} has no completed artifact — skipping regression check",
                    runId, pinned);
                return; // an explicit baseline that can't be compared is a no-op, not a fallback
            }
            baselineRunId = pinnedRun.Id;
            baselineArtifactId = pinnedRun.ArtifactId;
        }
        else
        {
            var prior = await db.TestRuns
                .AsNoTracking()
                .Where(r => r.TestConfigId == run.TestConfigId
                    && r.Id != runId
                    && r.Status == "completed"
                    && r.ArtifactId != null
                    && r.CreatedAt <= run.CreatedAt)
                .OrderByDescending(r => r.CreatedAt)
                .Select(r => new { r.Id, r.ArtifactId })
                .FirstOrDefaultAsync(ct);
            if (prior is null)
            {
                return; // first run of this config — no baseline yet
            }
            baselineRunId = prior.Id;
            baselineArtifactId = prior.ArtifactId;
        }

        var currentSummaries = await LoadSummariesAsync(run.ArtifactId!.Value, ct);
        var baselineSummaries = await LoadSummariesAsync(baselineArtifactId!.Value, ct);

        var regressions = RegressionAnalyzer.Detect(
            RegressionAnalyzer.ParseSummaries(currentSummaries),
            RegressionAnalyzer.ParseSummaries(baselineSummaries));

        if (regressions.Count == 0)
        {
            return;
        }

        var now = DateTime.UtcNow;
        foreach (var reg in regressions)
        {
            db.BenchmarkRegressions.Add(new BenchmarkRegressionRecord
            {
                RegressionId = Guid.NewGuid(),
                TestConfigId = config.Id,
                TestRunId = runId,
                BaselineRunId = baselineRunId,
                CaseId = reg.CaseId,
                Metric = reg.Metric,
                MetricUnit = reg.MetricUnit,
                BaselineValue = reg.BaselineValue,
                CurrentValue = reg.CurrentValue,
                DeltaPercent = reg.DeltaPercent,
                Severity = reg.Severity,
                DetectedAt = now,
            });
        }
        await db.SaveChangesAsync(ct);

        RegressionAnalyzer.EmitRegressionEvent(bus, config.Id, config.Name, regressions);

        logger.LogInformation(
            "Flagged {Count} regression(s) for run {RunId} (config {ConfigId} '{ConfigName}', baseline run {BaselineRunId})",
            regressions.Count, runId, config.Id, config.Name, baselineRunId);
    }

    private async Task<string?> LoadSummariesAsync(Guid artifactId, CancellationToken ct) =>
        await db.BenchmarkArtifacts
            .AsNoTracking()
            .Where(a => a.Id == artifactId)
            .Select(a => a.Summaries)
            .FirstOrDefaultAsync(ct);
}
