using Microsoft.EntityFrameworkCore;
using Networker.Data;

namespace Networker.ControlPlane.Realtime;

/// <summary>
/// THE producer of live <c>tester_queue_update</c> deltas. The dashboard's
/// <c>useTesterSubscription</c> hook has always subscribed to these, and
/// <see cref="TesterQueueBroadcaster"/> could always send them — but nothing
/// ever called it (a gap carried over from the Rust hub, which also only sent
/// snapshots; found by the 2026-07 dead-code sweep). Until now the queue panel
/// only refreshed on reconnect snapshots.
///
/// <para>Wired as an <see cref="IDashboardEventObserver"/> on the
/// <see cref="EventBus"/>: every run transition already publishes there
/// (<see cref="JobUpdate"/> from the dispatcher / agent processor / watchdog,
/// <see cref="JobComplete"/> on finish), so this needs no changes to any
/// producer. On each transition it resolves the run's tester, skips cheaply
/// when nobody is subscribed, rebuilds the queue via the same query the
/// snapshots use, and pushes through the broadcaster (which stamps the
/// monotonic per-tester seq).</para>
///
/// <para><b>Ordering:</b> two near-simultaneous transitions can build their
/// states concurrently; each build reads the CURRENT DB state, and the client
/// drops any update whose seq is ≤ the last seen, so a stale build can only be
/// applied if it wins the seq race — in which case the next transition (or the
/// reconnect snapshot, which is authoritative) re-syncs. Deltas are
/// best-effort by design, exactly like the browser event bus.</para>
/// </summary>
public sealed class TesterQueueUpdateProducer(
    IServiceScopeFactory scopeFactory,
    TesterQueueRegistry registry,
    ITesterQueuePush push,
    ILogger<TesterQueueUpdateProducer> logger) : IDashboardEventObserver
{
    public void OnEvent(DashboardEvent evt)
    {
        var work = evt switch
        {
            JobUpdate j => (RunId: j.JobId, Trigger: TriggerFor(j.Status)),
            JobComplete c => (RunId: c.RunId, Trigger: "run_completed"),
            _ => default,
        };
        if (work == default)
        {
            return;
        }

        // Detached: OnEvent runs on the publisher's hot path and must not block.
        _ = HandleAsync(work.RunId, work.Trigger);
    }

    /// <summary>Maps a run status to the update's <c>trigger</c> field
    /// (named causes, mirroring the broadcaster's doc examples).</summary>
    internal static string TriggerFor(string status) => "run_" + status;

    internal async Task HandleAsync(Guid runId, string trigger)
    {
        try
        {
            using var scope = scopeFactory.CreateScope();
            var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();

            var run = await db.TestRuns
                .AsNoTracking()
                .Where(r => r.Id == runId)
                .Select(r => new { r.ProjectId, r.TesterId })
                .FirstOrDefaultAsync();

            // Runs not bound to a persistent tester have no queue to update.
            if (run?.TesterId is not Guid testerId)
            {
                return;
            }

            // Cheap in-memory gate before any queue query — the common case
            // (nobody watching this tester) costs one PK lookup and exits here.
            if (!registry.HasSubscribers(run.ProjectId, testerId.ToString()))
            {
                return;
            }

            var (running, queued) = await RawWs.TesterQueueSocketEndpoint
                .BuildQueueStateAsync(db, run.ProjectId, testerId, CancellationToken.None);

            await push.NotifyQueueUpdateAsync(
                run.ProjectId, testerId.ToString(), trigger, running, queued.ToList());
        }
        catch (Exception ex)
        {
            // Best-effort delta — never let a push failure surface anywhere.
            logger.LogWarning(
                ex, "tester_queue_update push failed for run {RunId} ({Trigger})", runId, trigger);
        }
    }
}
