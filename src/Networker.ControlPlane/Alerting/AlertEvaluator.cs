using Microsoft.EntityFrameworkCore;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Alerting;

/// <summary>
/// Evaluates every enabled alert rule against a run that just reached a
/// terminal status, records state transitions as <c>alert_event</c> rows, and
/// delivers them through the rule's channel.
///
/// <para><b>Hook point.</b> Called from
/// <see cref="Realtime.RawWs.AgentMessageProcessor"/> after a
/// <c>run_finished</c> frame is persisted (both transports — raw WS and
/// SignalR — share that processor). Evaluation is best-effort by contract:
/// any failure is logged and swallowed so alerting can never fail run
/// processing.</para>
///
/// <para><b>Semantics.</b> A rule matches when it belongs to the run's
/// project and either names the run's test_config or names none (project-wide
/// rule). The window check requires the last <c>window_runs</c> terminal runs
/// of THAT config (newest first, including this one) to all breach; a run
/// where the metric is not measurable breaks the streak, and when the
/// triggering run itself has no data the rule is skipped entirely (no fire,
/// no resolve — missing data is not evidence of recovery). Dedup/state is
/// tracked per (rule, config) via the latest recorded event: quiet → breach
/// fires once, stays silent while breaching, and fires a single
/// <c>resolved</c> when back within threshold.</para>
///
/// <para>Delivery is synchronous (webhook timeout 10s + one retry bounds it)
/// and its outcome is recorded on the event's <c>delivery_status</c>.</para>
/// </summary>
public sealed class AlertEvaluator(
    NetworkerDbContext db,
    RunMetricProvider metrics,
    IAlertNotifier notifier,
    ILogger<AlertEvaluator> logger)
{
    private static readonly string[] TerminalStatuses = ["completed", "failed"];

    /// <summary>Evaluate all matching rules for a run that just finished. Never throws.</summary>
    public async Task EvaluateRunAsync(Guid runId, CancellationToken ct = default)
    {
        try
        {
            await EvaluateCoreAsync(runId, ct);
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            // Shutdown/disconnect — nothing to log.
        }
        catch (Exception ex)
        {
            logger.LogError(ex, "Alert evaluation failed for run {RunId} (non-fatal)", runId);
        }
    }

    private async Task EvaluateCoreAsync(Guid runId, CancellationToken ct)
    {
        var run = await db.TestRuns
            .AsNoTracking()
            .Where(r => r.Id == runId)
            .Select(r => new { r.Id, r.ProjectId, r.TestConfigId, r.Status, r.SuccessCount, r.FailureCount, r.CreatedAt })
            .FirstOrDefaultAsync(ct);

        if (run is null || !TerminalStatuses.Contains(run.Status))
        {
            return;
        }

        var rules = await db.AlertRules
            .AsNoTracking()
            .Where(r => r.ProjectId == run.ProjectId
                && r.Enabled
                && (r.TestConfigId == null || r.TestConfigId == run.TestConfigId))
            .ToListAsync(ct);

        if (rules.Count == 0)
        {
            return; // fast path: no rules → no metric extraction at all
        }

        // Prior terminal runs of the same config (newest first), enough for the
        // widest window. The triggering run is prepended explicitly so the
        // window is anchored on it even if newer runs finished meanwhile.
        var maxWindow = rules.Max(r => r.WindowRuns);
        var priorRuns = new List<(Guid Id, int SuccessCount, int FailureCount)>();
        if (maxWindow > 1)
        {
            var rows = await db.TestRuns
                .AsNoTracking()
                .Where(r => r.TestConfigId == run.TestConfigId
                    && r.Id != run.Id
                    && TerminalStatuses.Contains(r.Status)
                    && r.CreatedAt <= run.CreatedAt)
                .OrderByDescending(r => r.CreatedAt)
                .Take(maxWindow - 1)
                .Select(r => new { r.Id, r.SuccessCount, r.FailureCount })
                .ToListAsync(ct);
            priorRuns.AddRange(rows.Select(r => (r.Id, r.SuccessCount, r.FailureCount)));
        }

        foreach (var rule in rules)
        {
            var currentValue = await metrics.GetAsync(
                run.Id, run.SuccessCount, run.FailureCount, rule.Metric, ct);
            if (currentValue is null)
            {
                continue; // no data on the triggering run: neither fire nor resolve
            }

            var values = new List<double?> { currentValue };
            foreach (var prior in priorRuns.Take(rule.WindowRuns - 1))
            {
                values.Add(await metrics.GetAsync(
                    prior.Id, prior.SuccessCount, prior.FailureCount, rule.Metric, ct));
            }

            var breached = AlertRuleLogic.WindowBreached(
                values, rule.WindowRuns, rule.Comparator, rule.Threshold);

            // Latest recorded state for this (rule, config) pair — project-wide
            // rules track state independently per config so one breaching
            // config can't mask or flap another.
            var lastState = await db.AlertEvents
                .AsNoTracking()
                .Where(e => e.RuleId == rule.RuleId
                    && db.TestRuns.Any(r => r.Id == e.RunId && r.TestConfigId == run.TestConfigId))
                .OrderByDescending(e => e.FiredAt)
                .Select(e => e.State)
                .FirstOrDefaultAsync(ct);
            var currentlyFiring = lastState == AlertRuleLogic.StateFiring;

            var transition = AlertRuleLogic.NextTransition(currentlyFiring, breached);
            if (transition is null)
            {
                continue;
            }

            var evt = new AlertEvent
            {
                EventId = Guid.NewGuid(),
                RuleId = rule.RuleId,
                RunId = run.Id,
                FiredAt = DateTime.UtcNow,
                State = transition,
                Value = currentValue,
                Message = AlertRuleLogic.BuildMessage(
                    transition, rule.Metric, rule.Comparator, rule.Threshold,
                    currentValue.Value, rule.WindowRuns),
                DeliveryStatus = "pending",
            };
            db.AlertEvents.Add(evt);
            await db.SaveChangesAsync(ct);

            evt.DeliveryStatus = await DeliverAsync(rule, run.ProjectId, evt, ct);
            await db.SaveChangesAsync(ct);

            logger.LogInformation(
                "Alert {State} for rule {RuleId} on run {RunId} ({Message}) — delivery: {Delivery}",
                evt.State, rule.RuleId, run.Id, evt.Message, evt.DeliveryStatus);
        }
    }

    private async Task<string> DeliverAsync(
        AlertRule rule, string projectId, AlertEvent evt, CancellationToken ct)
    {
        var channel = await db.AlertChannels
            .AsNoTracking()
            .FirstOrDefaultAsync(c => c.ChannelId == rule.ChannelId, ct);

        if (channel is null)
        {
            return "skipped: channel missing";
        }
        if (!channel.Enabled)
        {
            return "skipped: channel disabled";
        }

        var notification = new AlertNotification(
            evt.EventId,
            rule.RuleId,
            projectId.TrimEnd(), // char(14) column pads with spaces
            rule.TestConfigId,
            evt.RunId,
            rule.Metric,
            rule.Comparator,
            rule.Threshold,
            evt.Value,
            evt.State,
            evt.Message ?? string.Empty,
            evt.FiredAt);

        return await notifier.DeliverAsync(channel, notification, ct);
    }
}
