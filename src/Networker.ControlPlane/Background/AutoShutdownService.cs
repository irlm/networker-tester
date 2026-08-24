using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Provisioning;
using Networker.Data;
using Networker.Data.Entities;
using Networker.Security;

namespace Networker.ControlPlane.Background;

/// <summary>
/// Auto-shutdown loop — the C# port of the Rust dashboard's
/// <c>auto_shutdown_loop</c> in
/// <c>crates/networker-dashboard/src/services/tester_scheduler.rs</c>.
///
/// <para>Every ~60s it sweeps for testers whose scheduled shutdown window has
/// elapsed and that are <b>drained</b> (no non-terminal run references them),
/// then deallocates each via <see cref="IComputeProvisioner.DeallocateAsync"/>
/// and flips <c>power_state</c> to <c>stopped</c>, advancing
/// <c>next_shutdown_at</c> to the next daily occurrence.</para>
///
/// <para><b>Shutdown condition</b> (the LINQ equivalent of the Rust SQL):
/// <c>auto_shutdown_enabled = TRUE AND next_shutdown_at &lt; NOW() AND
/// power_state = 'running' AND allocation = 'idle' AND NOT EXISTS (a test_run
/// for that tester whose status is in queued/provisioning/running)</c>. That
/// last clause is the "drain" check — a tester holding any non-terminal run is
/// considered busy and is deferred rather than shut down.</para>
///
/// <para><b>Deferral cap</b> (mirrors Rust <c>DEFERRAL_CAP = 3</c>,
/// <c>DEFERRAL_DELAY_MINUTES = 5</c>): the sweep query already excludes busy
/// testers, so within this port a candidate that races back to busy between the
/// query and the per-tester re-check is deferred — <c>shutdown_deferral_count</c>
/// is bumped and <c>next_shutdown_at</c> is pushed +5min. When the count reaches
/// the cap we log loudly (the tester is stuck behind long-running work) but keep
/// re-checking; we never force-kill an in-flight run.</para>
///
/// <para><b>CI-safe:</b> a missing cloud CLI is a <i>soft</i> failure from the
/// provisioner (<see cref="ProvisionResult.Success"/> == false with
/// <see cref="ProvisionResult.ExitCode"/> == null). We treat that exactly like
/// the Rust "Azure said OK" path — the state converges to <c>stopped</c> — so a
/// credential-less/CI host never gets a tester wedged in <c>stopping</c>. Only a
/// genuine non-zero CLI exit rolls the row back to <c>running</c> for retry.</para>
///
/// <para><b>Scope discipline</b> (identical to <see cref="ReaperService"/> /
/// <see cref="SchedulerService"/>): <c>NetworkerDbContext</c> and
/// <see cref="IComputeProvisioner"/> are resolved from a fresh DI scope every
/// tick; the hosted service itself is a singleton.</para>
/// </summary>
public sealed class AutoShutdownService : BackgroundService
{
    /// <summary>Sweep cadence. Matches the Rust <c>TICK = 60s</c>.</summary>
    private static readonly TimeSpan TickInterval = TimeSpan.FromSeconds(60);

    /// <summary>Max number of times a due tester may be deferred before we log
    /// loudly that it is stuck. Mirrors the Rust <c>DEFERRAL_CAP = 3</c>.</summary>
    private const short DeferralCap = 3;

    /// <summary>How far to push <c>next_shutdown_at</c> when deferring a busy
    /// tester. Mirrors the Rust <c>DEFERRAL_DELAY_MINUTES = 5</c>.</summary>
    private static readonly TimeSpan DeferralDelay = TimeSpan.FromMinutes(5);

    /// <summary>How long a runner may sit in 'starting' before the sweep decides
    /// the wake failed and releases it. Must exceed
    /// <c>WatchdogService.WakeInFlightGrace</c> so the watchdog gives up waiting
    /// on the wake before this cancels it.</summary>
    internal static TimeSpan StuckStartingTimeout = TimeSpan.FromMinutes(20);

    private readonly IServiceScopeFactory _scopeFactory;
    private readonly ILogger<AutoShutdownService> _logger;
    private readonly PgAdvisoryLeaderLock? _leader;
    private readonly TickMonitor _monitor;

    public AutoShutdownService(
        IServiceScopeFactory scopeFactory,
        ILogger<AutoShutdownService> logger,
        PgAdvisoryLeaderLock? leaderLock = null,
        TickMonitor? tickMonitor = null)
    {
        _scopeFactory = scopeFactory;
        _logger = logger;
        // M6 ops infra (AddOpsInfrastructure); optional for bare test hosts.
        _leader = leaderLock;
        _monitor = tickMonitor ?? new TickMonitor();
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        _logger.LogInformation(
            "Auto-shutdown background service started (tick every {Seconds}s)",
            TickInterval.TotalSeconds);
        _monitor.ReportStarted(OpsServiceNames.AutoShutdown);

        using var timer = new PeriodicTimer(TickInterval);
        while (await timer.WaitForNextTickAsync(stoppingToken).ConfigureAwait(false))
        {
            try
            {
                var ranAsLeader = await _leader
                    .TryRunGuardedAsync(LeaderLockKeys.AutoShutdown, SweepAsync, stoppingToken)
                    .ConfigureAwait(false);
                if (!ranAsLeader)
                {
                    _logger.LogDebug("Auto-shutdown sweep skipped — another replica holds the leader lock");
                }
            }
            catch (OperationCanceledException) when (stoppingToken.IsCancellationRequested)
            {
                break;
            }
            catch (Exception ex)
            {
                // Never let one bad sweep kill the loop (Rust logs + continues).
                _monitor.ReportError(OpsServiceNames.AutoShutdown, ex);
                _logger.LogError(ex, "Auto-shutdown sweep failed");
            }
        }
    }

    private async Task SweepAsync(CancellationToken ct)
    {
        using var scope = _scopeFactory.CreateScope();
        var sp = scope.ServiceProvider;
        var db = sp.GetRequiredService<NetworkerDbContext>();
        var provisioner = sp.GetRequiredService<IComputeProvisioner>();
        // Optional: bare test/CI hosts register no cipher. Without it a GCP
        // tester's account key can't be decrypted and the call degrades to
        // ambient auth — the same posture the reaper and the deploy staging
        // take (never a throw that kills the sweep).
        var cipher = sp.GetService<CredentialCipher>();

        var now = DateTime.UtcNow;

        // ── Release a wake that never completed ─────────────────────────────
        // 'starting' is a TRANSIENT state with no self-imposed exit: it becomes
        // 'running' only when the agent heartbeats. If the VM never boots, or
        // boots without a working agent, the row sits in 'starting' forever —
        // the UI shows a runner permanently "starting" (prod, 2026-08-24), it
        // can never be started by hand (that path requires 'stopped'), and the
        // watchdog used to read it as a wake permanently in flight.
        //
        // Released to 'stopped', not 'error', on purpose: 'stopped' is the state
        // a manual start accepts, the auto-wake arm below can retry it in this
        // same tick, and if the agent does turn up late its heartbeat flips the
        // row to 'running' anyway. 'error' would need an operator to clear it,
        // turning a transient failure into a dead runner.
        //
        // Longer than WatchdogService.WakeInFlightGrace (15m) by design: the
        // watchdog stops *waiting* on the wake before this sweep *cancels* it,
        // so the two never race to interpret the same row.
        var stuckStartingFloor = now - StuckStartingTimeout;
        var released = await db.ProjectTesters
            .Where(t => t.PowerState == "starting" && t.UpdatedAt < stuckStartingFloor)
            .ExecuteUpdateAsync(st => st
                .SetProperty(t => t.PowerState, "stopped")
                .SetProperty(t => t.StatusMessage,
                    "start did not complete within " + (int)StuckStartingTimeout.TotalMinutes
                    + "m — agent never connected; released so it can be started again")
                .SetProperty(t => t.UpdatedAt, now), ct)
            .ConfigureAwait(false);
        if (released > 0)
        {
            _logger.LogWarning(
                "Released {Count} runner(s) stuck in 'starting' for over {Minutes}m — the agent never connected",
                released, (int)StuckStartingTimeout.TotalMinutes);
        }

        // ── Auto-wake: queued work targeting a stopped runner powers it on ──
        // The inverse of the shutdown policy ("only shut down when idle"):
        // when runs are QUEUED for a tester that auto-shutdown (or anything
        // else) stopped, start it instead of letting the work sit — before
        // this, a matrix launched after the nightly shutdown just queued until
        // someone started the VM by hand (2026-08-04). power_state 'starting'
        // keeps the shutdown arm (requires 'running') off its back during
        // boot; the heartbeat reconcile flips it to 'running' when the agent
        // connects.
        var toWake = await db.ProjectTesters
            .Where(t => (t.PowerState == "stopped" || t.PowerState == "deallocated")
                && db.TestRuns.Any(r => r.TesterId == t.TesterId && r.Status == "queued"))
            .ToListAsync(ct)
            .ConfigureAwait(false);
        foreach (var tester in toWake)
        {
            ct.ThrowIfCancellationRequested();
            await TryWakeAsync(db, cipher, provisioner, tester, "pinned queued run(s)", ct).ConfigureAwait(false);
        }

        // ── Auto-wake, UNPINNED arm: auto-pick launches (URL probes, plain
        // network tests) carry no TesterId, so the arm above never matched
        // them — a probe launched after the nightly shutdown sat queued with
        // no signal until the watchdog failed it (user-caught 2026-08-11).
        // For each project with unpinned queued work and NO online agent,
        // wake ONE stopped idle tester; the redispatcher hands the run over
        // when its agent connects.
        var projectsNeedingRunner = await db.TestRuns
            .Where(r => r.Status == "queued" && r.TesterId == null)
            .Select(r => r.ProjectId)
            .Distinct()
            .Where(pid => !db.Agents.Any(a => a.ProjectId == pid && a.Status == "online"))
            .ToListAsync(ct)
            .ConfigureAwait(false);
        foreach (var pid in projectsNeedingRunner)
        {
            ct.ThrowIfCancellationRequested();
            // Anything already starting? Then a wake is in flight — don't
            // start a second VM for the same queue.
            var alreadyWaking = await db.ProjectTesters
                .AnyAsync(t => t.ProjectId == pid && t.PowerState == "starting", ct)
                .ConfigureAwait(false);
            if (alreadyWaking)
            {
                continue;
            }
            var candidate = await db.ProjectTesters
                .Where(t => t.ProjectId == pid
                    && (t.PowerState == "stopped" || t.PowerState == "deallocated")
                    && t.Allocation == "idle")
                .OrderBy(t => t.Name)
                .FirstOrDefaultAsync(ct)
                .ConfigureAwait(false);
            if (candidate is null)
            {
                _logger.LogWarning(
                    "Project {ProjectId} has unpinned queued run(s), no online agent, and no wakeable tester — runs will sit until the watchdog cutoff",
                    pid);
                continue;
            }
            await TryWakeAsync(db, cipher, provisioner, candidate, "unpinned queued run(s), no online agent", ct)
                .ConfigureAwait(false);
        }

        // Shutdown condition — the LINQ equivalent of the Rust sweep SQL:
        //   WHERE auto_shutdown_enabled = TRUE
        //     AND next_shutdown_at < NOW()
        //     AND power_state = 'running'
        //     AND allocation  = 'idle'
        //     AND NOT EXISTS (SELECT 1 FROM test_run r
        //                       WHERE r.tester_id = t.tester_id
        //                         AND r.status IN ('queued','provisioning','running'))
        var candidates = await db.ProjectTesters
            .Where(t => t.AutoShutdownEnabled
                && t.NextShutdownAt != null && t.NextShutdownAt < now
                && t.PowerState == "running"
                && t.Allocation == "idle"
                && !db.TestRuns.Any(r =>
                    r.TesterId == t.TesterId
                    && (r.Status == "queued" || r.Status == "provisioning" || r.Status == "running")))
            .ToListAsync(ct)
            .ConfigureAwait(false);

        if (candidates.Count == 0)
        {
            _monitor.ReportTick(OpsServiceNames.AutoShutdown, 0, "no drained candidates");
            return;
        }

        _logger.LogDebug("Auto-shutdown sweep: {Count} drained candidate(s)", candidates.Count);

        var stopped = 0;
        var deferred = 0;
        var failed = 0;

        foreach (var tester in candidates)
        {
            ct.ThrowIfCancellationRequested();
            try
            {
                var outcome = await HandleDueTesterAsync(db, cipher, provisioner, tester.TesterId, ct)
                    .ConfigureAwait(false);
                switch (outcome)
                {
                    case Outcome.Stopped: stopped++; break;
                    case Outcome.Deferred: deferred++; break;
                    case Outcome.Failed: failed++; break;
                }
            }
            catch (Exception ex)
            {
                // One poison tester must never abort the batch (Rust per-tester catch).
                failed++;
                _logger.LogWarning(ex,
                    "Per-tester auto-shutdown failed for {TesterId} ({Name})",
                    tester.TesterId, tester.Name);
            }
        }

        _logger.LogInformation(
            "Auto-shutdown sweep: {Count} candidate(s), {Stopped} stopped, {Deferred} deferred, {Failed} failed",
            candidates.Count, stopped, deferred, failed);

        _monitor.ReportTick(
            OpsServiceNames.AutoShutdown,
            candidates.Count,
            $"stopped={stopped} deferred={deferred} failed={failed}");
    }

    private enum Outcome { Stopped, Deferred, Failed }

    private async Task<Outcome> HandleDueTesterAsync(
        NetworkerDbContext db,
        CredentialCipher? cipher,
        IComputeProvisioner provisioner,
        Guid testerId,
        CancellationToken ct)
    {
        // Re-load fresh + tracked; the row may have changed between the sweep
        // query and now.
        var tester = await db.ProjectTesters
            .FirstOrDefaultAsync(t => t.TesterId == testerId, ct)
            .ConfigureAwait(false);
        if (tester is null)
        {
            return Outcome.Deferred;
        }

        // Race re-check (Rust `still_drained`): confirm the tester is still
        // running+idle and holds no non-terminal run. If it re-locked between the
        // sweep and now, defer instead of shutting down live work.
        var stillDrained = tester.PowerState == "running"
            && tester.Allocation == "idle"
            && !await db.TestRuns.AnyAsync(r =>
                    r.TesterId == tester.TesterId
                    && (r.Status == "queued" || r.Status == "provisioning" || r.Status == "running"),
                ct)
                .ConfigureAwait(false);

        if (!stillDrained)
        {
            await DeferAsync(db, tester, ct).ConfigureAwait(false);
            return Outcome.Deferred;
        }

        // Flip running → stopping as a guarded transition. If someone else moved
        // it, skip this cycle (mirrors the Rust `try_power_transition`).
        var transitioned = await db.ProjectTesters
            .Where(t => t.TesterId == tester.TesterId && t.PowerState == "running")
            .ExecuteUpdateAsync(
                s => s.SetProperty(t => t.PowerState, "stopping")
                      .SetProperty(t => t.UpdatedAt, DateTime.UtcNow),
                ct)
            .ConfigureAwait(false);
        if (transitioned == 0)
        {
            _logger.LogDebug(
                "Auto-shutdown skipped {TesterId}: power_state no longer 'running'", tester.TesterId);
            return Outcome.Deferred;
        }
        tester.PowerState = "stopping";

        // Deallocate via the CLI provisioner (Azure vm deallocate / AWS stop /
        // GCP stop). Total: never throws — a missing CLI is a soft failure.
        var creds = await TesterLifecycleCredentials
            .LoadAsync(db, cipher, tester, _logger, ct)
            .ConfigureAwait(false);
        var res = await provisioner.DeallocateAsync(tester, creds, ct).ConfigureAwait(false);

        // A genuine non-zero CLI exit (ExitCode present) is the only "real"
        // failure. A missing CLI (ExitCode == null) is treated as success so a
        // credential-less/CI host still converges to 'stopped' rather than
        // wedging the tester in 'stopping' — this is exactly the endpoint path's
        // FinishAsync rule.
        var realFailure = !res.Success && res.ExitCode is not null;
        if (realFailure)
        {
            // Roll power_state back out of 'stopping' so the next tick retries
            // (mirrors the Rust rollback-to-running on deallocate failure).
            await db.ProjectTesters
                .Where(t => t.TesterId == tester.TesterId)
                .ExecuteUpdateAsync(
                    s => s.SetProperty(t => t.PowerState, "running")
                          .SetProperty(t => t.StatusMessage, $"auto-shutdown deallocate failed: {res.Error ?? res.StdErr}")
                          .SetProperty(t => t.UpdatedAt, DateTime.UtcNow),
                    ct)
                .ConfigureAwait(false);
            _logger.LogWarning(
                "Auto-shutdown deallocate failed for {TesterId} ({Name}); rolled power_state back to running: {Err}",
                tester.TesterId, tester.Name, res.Error ?? res.StdErr);
            return Outcome.Failed;
        }

        // Success (real deallocate, or CLI-unavailable assumed): sync 'stopped',
        // reset the deferral count, and advance next_shutdown_at to the next
        // daily occurrence.
        var next = NextShutdownAtUtc(tester.AutoShutdownLocalHour, DateTime.UtcNow);
        var statusMessage = res.ExitCode is null
            ? "auto-shutdown completed (cloud CLI unavailable — state assumed)"
            : "auto-shutdown completed";
        await db.ProjectTesters
            .Where(t => t.TesterId == tester.TesterId)
            .ExecuteUpdateAsync(
                s => s.SetProperty(t => t.PowerState, "stopped")
                      .SetProperty(t => t.NextShutdownAt, next)
                      .SetProperty(t => t.ShutdownDeferralCount, (short)0)
                      .SetProperty(t => t.StatusMessage, statusMessage)
                      .SetProperty(t => t.UpdatedAt, DateTime.UtcNow),
                ct)
            .ConfigureAwait(false);

        _logger.LogInformation(
            "Auto-shutdown completed for {TesterId} ({Name}) — next_shutdown_at={Next:o}",
            tester.TesterId, tester.Name, next);
        return Outcome.Stopped;
    }

    /// <summary>
    /// Defer shutdown for a tester that is no longer drained: bump
    /// <c>shutdown_deferral_count</c> and push <c>next_shutdown_at</c> +5min.
    /// When the count reaches <see cref="DeferralCap"/> we log loudly (stuck
    /// behind long-running work) but never force-kill the in-flight run — the
    /// C# analogue of the Rust <c>defer_shutdown</c>.
    /// </summary>
    private async Task DeferAsync(NetworkerDbContext db, ProjectTester tester, CancellationToken ct)
    {
        var newCount = (short)Math.Min(tester.ShutdownDeferralCount + 1, short.MaxValue);
        var newNext = DateTime.UtcNow + DeferralDelay;

        await db.ProjectTesters
            .Where(t => t.TesterId == tester.TesterId)
            .ExecuteUpdateAsync(
                s => s.SetProperty(t => t.ShutdownDeferralCount, newCount)
                      .SetProperty(t => t.NextShutdownAt, newNext)
                      .SetProperty(t => t.UpdatedAt, DateTime.UtcNow),
                ct)
            .ConfigureAwait(false);

        if (newCount >= DeferralCap)
        {
            // Surface the runs blocking shutdown so the operator log is useful.
            var blockers = await db.TestRuns
                .Where(r => r.TesterId == tester.TesterId
                    && (r.Status == "queued" || r.Status == "provisioning" || r.Status == "running"))
                .OrderBy(r => r.CreatedAt)
                .Select(r => r.Id)
                .Take(10)
                .ToListAsync(ct)
                .ConfigureAwait(false);
            _logger.LogWarning(
                "Tester {TesterId} ({Name}) auto-shutdown deferred {Count} times; cap reached. Blocked by run(s): {Blockers}",
                tester.TesterId, tester.Name, newCount, string.Join(", ", blockers));
        }
        else
        {
            _logger.LogInformation(
                "Auto-shutdown deferred for {TesterId} ({Name}) (deferral count = {Count})",
                tester.TesterId, tester.Name, newCount);
        }
    }

    /// <summary>
    /// Compute the next <c>next_shutdown_at</c>: the next occurrence of the
    /// tester's local shutdown hour, today if still ahead of <paramref name="now"/>
    /// otherwise tomorrow. This is the UTC-based analogue of the Rust
    /// <c>azure_regions::next_shutdown_at_for_provider</c>.
    ///
    /// <para><b>Simplification vs Rust:</b> the Rust version resolves the tester's
    /// region to an IANA timezone and computes the target hour in <i>local</i>
    /// time. Porting the full per-region → tz table is out of scope for this
    /// slice, so <paramref name="localHour"/> is interpreted as a UTC hour here.
    /// The observable behaviour — a stable, always-forward daily window ~24h out —
    /// is preserved; only the wall-clock alignment of the window differs by the
    /// region's UTC offset. Swap in a tz-aware resolver later without changing
    /// the call site.</para>
    /// </summary>
    internal static DateTime NextShutdownAtUtc(short localHour, DateTime now)
    {
        var hour = Math.Clamp((int)localHour, 0, 23);
        var todayTarget = new DateTime(now.Year, now.Month, now.Day, hour, 0, 0, DateTimeKind.Utc);
        return todayTarget > now ? todayTarget : todayTarget.AddDays(1);
    }

    /// <summary>
    /// Guarded single-tester wake: CAS power_state → 'starting', call the
    /// provisioner, roll back on hard failure. Shared by the pinned and
    /// unpinned auto-wake arms.
    /// </summary>
    private async Task TryWakeAsync(
        NetworkerDbContext db,
        CredentialCipher? cipher,
        IComputeProvisioner provisioner,
        ProjectTester tester,
        string reason,
        CancellationToken ct)
    {
        var prior = tester.PowerState;
        var claimed = await db.ProjectTesters
            .Where(t => t.TesterId == tester.TesterId && t.PowerState == prior)
            // StatusMessage moves WITH PowerState. Setting only the state left
            // the previous lifecycle's message in place, so a woken runner read
            // "starting" next to "auto-shutdown completed" — the two halves of
            // the badge disagreeing about what the machine was doing (reported
            // from prod, 2026-08-24). The manual start path already stamps
            // "Start requested"; this is the same courtesy for the automatic one,
            // and it also carries the REASON, which the manual path has no need of.
            .ExecuteUpdateAsync(s => s
                .SetProperty(t => t.PowerState, "starting")
                .SetProperty(t => t.StatusMessage, $"auto-wake: {reason}")
                .SetProperty(t => t.UpdatedAt, DateTime.UtcNow), ct)
            .ConfigureAwait(false);
        if (claimed == 0)
        {
            return; // raced another actor
        }

        var wakeCreds = await TesterLifecycleCredentials
            .LoadAsync(db, cipher, tester, _logger, ct)
            .ConfigureAwait(false);
        var res = await provisioner.StartAsync(tester, wakeCreds, ct).ConfigureAwait(false);
        if (res is { Success: true } || res is { Success: false, ExitCode: null })
        {
            // Started (or soft-failed on a CLI-less host — same convergence
            // posture as the deallocate path). Heartbeat completes the flip.
            _logger.LogInformation(
                "Auto-wake: started {Name} ({TesterId}) — {Reason}",
                tester.Name, tester.TesterId, reason);
        }
        else
        {
            await db.ProjectTesters
                .Where(t => t.TesterId == tester.TesterId && t.PowerState == "starting")
                .ExecuteUpdateAsync(s => s
                    .SetProperty(t => t.PowerState, prior)
                    .SetProperty(t => t.StatusMessage, "auto-wake failed — start was not accepted")
                    .SetProperty(t => t.UpdatedAt, DateTime.UtcNow), ct)
                .ConfigureAwait(false);
            _logger.LogWarning(
                "Auto-wake failed for {Name} ({TesterId}): {Err}",
                tester.Name, tester.TesterId, res.Error ?? res.StdErr);
        }
    }
}
