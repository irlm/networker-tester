using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Dispatch;
using Networker.Data;

namespace Networker.ControlPlane.Background;

/// <summary>
/// The scheduled-run driver — the C# port of the Rust scheduler's <c>tick()</c>
/// loop in <c>crates/networker-dashboard/src/scheduler.rs</c>.
///
/// <para>Every ~30s it wakes, loads every enabled <c>test_schedule</c> that is
/// due (<c>next_fire_at &lt;= now</c>, or has never been scheduled), and for each
/// creates + dispatches a run via <see cref="IRunDispatcher.LaunchAsync"/>, then
/// advances the schedule's <c>next_fire_at</c> from its cron + time zone. A bad
/// cron or a single failing schedule is caught per-iteration so one poison row
/// can never wedge the whole loop (mirrors the Rust per-schedule error handling).</para>
///
/// <para><b>Deferred / not ported here</b> (out of scope for this M3 slice — they
/// are the other sub-routines of the Rust scheduler loop): the "no agent online
/// =&gt; skip &amp; advance" churn guard, workspace-inactivity checks, invite/approval
/// expiry, the stale-assigned-job reaper, and hourly system health checks. The
/// <c>Pending</c>-endpoint provisioning branch is owned by <see cref="IRunDispatcher"/>
/// (deferred to M4). This service does the schedule=&gt;run fan-out only.</para>
/// </summary>
public sealed class SchedulerService : BackgroundService
{
    private static readonly TimeSpan TickInterval = TimeSpan.FromSeconds(30);

    /// <summary>
    /// Synthetic caller for scheduler-launched runs. There is no HTTP principal on
    /// a background tick, so we present a fixed system identity: platform-admin +
    /// operator role, which is the minimum <see cref="IRunDispatcher.LaunchAsync"/>
    /// needs to launch on any project. The id is a well-known nil-adjacent constant
    /// so system-originated runs are auditable/greppable.
    /// </summary>
    private static readonly AuthUser SystemUser = new(
        UserId: new Guid("00000000-0000-0000-0000-000000000001"),
        Email: "system@scheduler.networker",
        Role: "operator",
        IsPlatformAdmin: true);

    private readonly IServiceScopeFactory _scopeFactory;
    private readonly ILogger<SchedulerService> _logger;

    public SchedulerService(IServiceScopeFactory scopeFactory, ILogger<SchedulerService> logger)
    {
        _scopeFactory = scopeFactory;
        _logger = logger;
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        _logger.LogInformation("Scheduler background service started (tick every {Seconds}s)",
            TickInterval.TotalSeconds);

        using var timer = new PeriodicTimer(TickInterval);
        while (await timer.WaitForNextTickAsync(stoppingToken).ConfigureAwait(false))
        {
            try
            {
                await TickAsync(stoppingToken).ConfigureAwait(false);
            }
            catch (OperationCanceledException) when (stoppingToken.IsCancellationRequested)
            {
                break;
            }
            catch (Exception ex)
            {
                // Never let a tick throw out of the loop — log and keep ticking.
                _logger.LogError(ex, "Scheduler tick failed");
            }
        }
    }

    private async Task TickAsync(CancellationToken ct)
    {
        // IRunDispatcher + NetworkerDbContext are scoped; a hosted service is a
        // singleton, so every tick opens its own DI scope.
        using var scope = _scopeFactory.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
        var dispatcher = scope.ServiceProvider.GetRequiredService<IRunDispatcher>();

        var now = DateTime.UtcNow;

        // Enabled schedules that are due (next_fire_at <= now) or have never been
        // scheduled (null next_fire_at => compute + persist the first occurrence).
        var due = await db.TestSchedules
            .Where(s => s.Enabled && (s.NextFireAt == null || s.NextFireAt <= now))
            .ToListAsync(ct)
            .ConfigureAwait(false);

        if (due.Count == 0)
        {
            return;
        }

        var launched = 0;
        var advanced = 0;
        var failed = 0;

        foreach (var schedule in due)
        {
            ct.ThrowIfCancellationRequested();
            try
            {
                // A null next_fire_at means the schedule has never fired: seed its
                // first occurrence and skip launching this pass (nothing is "due"
                // yet — it was simply never initialised).
                if (schedule.NextFireAt is null)
                {
                    schedule.NextFireAt =
                        ScheduleTiming.NextFireUtc(schedule.CronExpr, schedule.Timezone, now);
                    advanced++;
                    if (schedule.NextFireAt is null)
                    {
                        _logger.LogWarning(
                            "Schedule {ScheduleId} has an unparseable cron '{Cron}' — cannot seed next_fire_at",
                            schedule.Id, schedule.CronExpr);
                    }

                    continue;
                }

                var runId = await dispatcher
                    .LaunchAsync(schedule.TestConfigId, comparisonGroupId: null, SystemUser, ct)
                    .ConfigureAwait(false);

                schedule.LastFiredAt = now;
                schedule.LastRunId = runId;
                schedule.NextFireAt =
                    ScheduleTiming.NextFireUtc(schedule.CronExpr, schedule.Timezone, now);
                launched++;

                if (schedule.NextFireAt is null)
                {
                    _logger.LogWarning(
                        "Schedule {ScheduleId} fired run {RunId} but its cron '{Cron}' is unparseable — next_fire_at left null (won't re-fire)",
                        schedule.Id, runId, schedule.CronExpr);
                }
            }
            catch (Exception ex)
            {
                // One bad schedule (missing config, bad cron, dispatch error) must
                // not abort the rest of the batch.
                failed++;
                _logger.LogError(ex,
                    "Failed to process schedule {ScheduleId} (config {ConfigId})",
                    schedule.Id, schedule.TestConfigId);
            }
        }

        await db.SaveChangesAsync(ct).ConfigureAwait(false);

        _logger.LogInformation(
            "Scheduler tick: {Due} due, {Launched} launched, {Advanced} seeded, {Failed} failed",
            due.Count, launched, advanced, failed);
    }
}
