using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Endpoints;
using Networker.Data;

namespace Networker.ControlPlane.Background;

/// <summary>
/// Workspace-inactivity lifecycle loop — the C# port of the Rust scheduler's
/// daily <c>check_workspace_inactivity</c> sub-routine
/// (<c>crates/networker-dashboard/src/scheduler.rs</c>).
///
/// <para>Escalation ladder (all thresholds in days, tracked in
/// <c>workspace_warning</c>):</para>
/// <list type="number">
///   <item><b>90d inactive</b> → record an <c>inactivity_90d</c> warning once
///   (member email = TODO stub, no mailer yet).</item>
///   <item><b>120d inactive</b> (warning at least 30d old, still inactive) →
///   auto-suspend: set <c>project.deleted_at</c>.</item>
///   <item><b>360d suspended</b> → record a <c>hard_delete_5d</c> notice once
///   (platform-admin email = TODO stub).</item>
///   <item><b>365d suspended</b> → permanent delete via the same FK-safe
///   cascade the admin hard-delete endpoint uses
///   (<see cref="WorkspaceCascade"/>). Logged loudly.</item>
/// </list>
///
/// <para>Inactivity is measured from the freshest of <c>project.updated_at</c>
/// and the project's latest <c>test_run.created_at</c> (the M5 adaptation — the
/// Rust side keyed off member <c>last_login_at</c>; runs are the better signal
/// once agents do the work). Protected (<c>delete_protection</c>) and
/// already-suspended workspaces are exempt from warn/suspend, matching the Rust
/// <c>find_inactive_workspaces</c> filters; protected workspaces are never
/// hard-deleted.</para>
///
/// <para>Cadence: one delayed initial pass (5 min after boot, so a crashing
/// dependency doesn't wedge startup), then a 24h <see cref="PeriodicTimer"/>.
/// Each pass opens its own DI scope; per-project failures are caught so one
/// poison row cannot abort the sweep.</para>
/// </summary>
public sealed class InactivityService : BackgroundService
{
    public static readonly TimeSpan TickInterval = TimeSpan.FromHours(24);
    public static readonly TimeSpan InitialDelay = TimeSpan.FromMinutes(5);

    public const int WarnAfterDays = 90;
    public const int SuspendAfterDays = 120;
    public const int HardDeleteNoticeAfterDays = 360;
    public const int HardDeleteAfterDays = 365;

    public const string InactivityWarningType = "inactivity_90d";
    public const string HardDeleteNoticeType = "hard_delete_5d";

    private readonly IServiceScopeFactory _scopeFactory;
    private readonly ILogger<InactivityService> _logger;

    public InactivityService(IServiceScopeFactory scopeFactory, ILogger<InactivityService> logger)
    {
        _scopeFactory = scopeFactory;
        _logger = logger;
    }

    /// <summary>What the sweep should do to a LIVE (non-suspended) workspace.</summary>
    public enum LifecycleAction
    {
        None = 0,
        Warn = 1,
        Suspend = 2,
    }

    /// <summary>What the sweep should do to a SUSPENDED workspace.</summary>
    public enum SuspendedAction
    {
        None = 0,
        NoticeHardDelete = 1,
        HardDelete = 2,
    }

    /// <summary>
    /// Pure threshold math for live workspaces (unit-testable).
    /// <list type="bullet">
    ///   <item>&lt; 90d inactive → <see cref="LifecycleAction.None"/>.</item>
    ///   <item>≥ 90d and no active warning → <see cref="LifecycleAction.Warn"/>.</item>
    ///   <item>≥ 120d, warning recorded ≥ 30d ago (the Rust
    ///   <c>warnings_older_than(.., 30)</c> grace) → <see cref="LifecycleAction.Suspend"/>.</item>
    /// </list>
    /// </summary>
    public static LifecycleAction DecideLiveAction(
        DateTime lastActivityUtc, DateTime? warningSentAtUtc, DateTime nowUtc)
    {
        var inactiveDays = (nowUtc - lastActivityUtc).TotalDays;
        if (inactiveDays < WarnAfterDays)
        {
            return LifecycleAction.None;
        }

        if (warningSentAtUtc is null)
        {
            return LifecycleAction.Warn;
        }

        var warnedDays = (nowUtc - warningSentAtUtc.Value).TotalDays;
        if (inactiveDays >= SuspendAfterDays && warnedDays >= SuspendAfterDays - WarnAfterDays)
        {
            return LifecycleAction.Suspend;
        }

        return LifecycleAction.None;
    }

    /// <summary>
    /// Pure threshold math for suspended workspaces (unit-testable): the
    /// hard-delete notice fires once at ≥ 360d suspended; the delete itself at
    /// ≥ 365d — and only for workspaces that are ALREADY suspended (the caller
    /// filters protection).
    /// </summary>
    public static SuspendedAction DecideSuspendedAction(
        DateTime deletedAtUtc, bool hasDeleteNotice, DateTime nowUtc)
    {
        var suspendedDays = (nowUtc - deletedAtUtc).TotalDays;
        if (suspendedDays >= HardDeleteAfterDays)
        {
            return SuspendedAction.HardDelete;
        }

        if (suspendedDays >= HardDeleteNoticeAfterDays && !hasDeleteNotice)
        {
            return SuspendedAction.NoticeHardDelete;
        }

        return SuspendedAction.None;
    }

    /// <summary>The freshest activity signal: project metadata touch vs latest run.</summary>
    public static DateTime EffectiveLastActivity(DateTime projectUpdatedAtUtc, DateTime? lastRunCreatedAtUtc)
        => lastRunCreatedAtUtc is { } run && run > projectUpdatedAtUtc ? run : projectUpdatedAtUtc;

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        _logger.LogInformation(
            "Workspace-inactivity lifecycle service started (initial pass in {InitialMinutes}m, then every {Hours}h; warn {Warn}d / suspend {Suspend}d / hard-delete {Delete}d)",
            InitialDelay.TotalMinutes, TickInterval.TotalHours,
            WarnAfterDays, SuspendAfterDays, HardDeleteAfterDays);

        try
        {
            await Task.Delay(InitialDelay, stoppingToken).ConfigureAwait(false);
            await RunPassSafeAsync(stoppingToken).ConfigureAwait(false);

            using var timer = new PeriodicTimer(TickInterval);
            while (await timer.WaitForNextTickAsync(stoppingToken).ConfigureAwait(false))
            {
                await RunPassSafeAsync(stoppingToken).ConfigureAwait(false);
            }
        }
        catch (OperationCanceledException) when (stoppingToken.IsCancellationRequested)
        {
            // normal shutdown
        }
    }

    private async Task RunPassSafeAsync(CancellationToken ct)
    {
        try
        {
            await RunPassAsync(ct).ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            throw;
        }
        catch (Exception ex)
        {
            // Never let one pass kill the loop (mirrors the Rust per-tick guards).
            _logger.LogError(ex, "Workspace-inactivity pass failed");
        }
    }

    /// <summary>One full sweep: warn → suspend → notice → hard-delete.</summary>
    internal async Task RunPassAsync(CancellationToken ct)
    {
        using var scope = _scopeFactory.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
        var now = DateTime.UtcNow;

        // ── Live workspaces: warn @90d, suspend @120d ────────────────────────
        var live = await db.Projects
            .AsNoTracking()
            .Where(p => p.DeletedAt == null && !p.DeleteProtection)
            .Select(p => new
            {
                p.ProjectId,
                p.Name,
                p.UpdatedAt,
                LastRunAt = db.TestRuns
                    .Where(r => r.ProjectId == p.ProjectId)
                    .Max(r => (DateTime?)r.CreatedAt),
            })
            .ToListAsync(ct)
            .ConfigureAwait(false);

        var warnings = await db.WorkspaceWarnings
            .AsNoTracking()
            .Where(w => w.WarningType == InactivityWarningType || w.WarningType == HardDeleteNoticeType)
            .ToListAsync(ct)
            .ConfigureAwait(false);
        var inactivityWarnedAt = warnings
            .Where(w => w.WarningType == InactivityWarningType)
            .ToDictionary(w => w.ProjectId.TrimEnd(), w => w.SentAt);
        var deleteNoticed = warnings
            .Where(w => w.WarningType == HardDeleteNoticeType)
            .Select(w => w.ProjectId.TrimEnd())
            .ToHashSet();

        var warned = 0;
        var suspended = 0;
        foreach (var ws in live)
        {
            ct.ThrowIfCancellationRequested();
            try
            {
                var projectId = ws.ProjectId.TrimEnd();
                var lastActivity = EffectiveLastActivity(ws.UpdatedAt, ws.LastRunAt);
                var warnedAt = inactivityWarnedAt.TryGetValue(projectId, out var sent)
                    ? sent
                    : (DateTime?)null;

                switch (DecideLiveAction(lastActivity, warnedAt, now))
                {
                    case LifecycleAction.Warn:
                        db.WorkspaceWarnings.Add(new Data.Entities.WorkspaceWarning
                        {
                            WarningId = Guid.NewGuid(),
                            ProjectId = ws.ProjectId,
                            WarningType = InactivityWarningType,
                            SentAt = now,
                        });
                        await db.SaveChangesAsync(ct).ConfigureAwait(false);
                        // TODO(M6 mailer): email every member "workspace will be
                        // suspended in 30 days if no one logs in" (Rust parity).
                        _logger.LogWarning(
                            "TODO email stub: 90-day inactivity warning for workspace {ProjectId} ({Name}) — member emails NOT sent (no mailer wired yet)",
                            projectId, ws.Name);
                        warned++;
                        break;

                    case LifecycleAction.Suspend:
                        await db.Projects
                            .Where(p => p.ProjectId == ws.ProjectId && p.DeletedAt == null)
                            .ExecuteUpdateAsync(
                                s => s.SetProperty(p => p.DeletedAt, now), ct)
                            .ConfigureAwait(false);
                        _logger.LogWarning(
                            "Auto-suspended workspace {ProjectId} ({Name}) after {Days}+ days of inactivity",
                            projectId, ws.Name, SuspendAfterDays);
                        suspended++;
                        break;
                }
            }
            catch (Exception ex) when (ex is not OperationCanceledException)
            {
                _logger.LogError(ex,
                    "Inactivity handling failed for workspace {ProjectId}", ws.ProjectId);
            }
        }

        // ── Suspended workspaces: notice @360d, hard-delete @365d ────────────
        var suspendedRows = await db.Projects
            .AsNoTracking()
            .Where(p => p.DeletedAt != null && !p.DeleteProtection)
            .Select(p => new { p.ProjectId, p.Name, p.DeletedAt })
            .ToListAsync(ct)
            .ConfigureAwait(false);

        var deleted = 0;
        foreach (var ws in suspendedRows)
        {
            ct.ThrowIfCancellationRequested();
            try
            {
                var projectId = ws.ProjectId.TrimEnd();
                switch (DecideSuspendedAction(ws.DeletedAt!.Value, deleteNoticed.Contains(projectId), now))
                {
                    case SuspendedAction.NoticeHardDelete:
                        db.WorkspaceWarnings.Add(new Data.Entities.WorkspaceWarning
                        {
                            WarningId = Guid.NewGuid(),
                            ProjectId = ws.ProjectId,
                            WarningType = HardDeleteNoticeType,
                            SentAt = now,
                        });
                        await db.SaveChangesAsync(ct).ConfigureAwait(false);
                        // TODO(M6 mailer): email platform admins "permanent
                        // deletion in 5 days" (Rust parity).
                        _logger.LogWarning(
                            "TODO email stub: workspace {ProjectId} ({Name}) suspended {Days}+ days — permanent deletion in 5 days; admin emails NOT sent (no mailer wired yet)",
                            projectId, ws.Name, HardDeleteNoticeAfterDays);
                        break;

                    case SuspendedAction.HardDelete:
                        _logger.LogWarning(
                            "AUTO-DELETING workspace {ProjectId} ({Name}) — suspended for {Days}+ days; this is PERMANENT",
                            projectId, ws.Name, HardDeleteAfterDays);
                        await WorkspaceCascade.HardDeleteAsync(db, ws.ProjectId, ct)
                            .ConfigureAwait(false);
                        deleted++;
                        break;
                }
            }
            catch (Exception ex) when (ex is not OperationCanceledException)
            {
                _logger.LogError(ex,
                    "Suspended-workspace lifecycle failed for {ProjectId}", ws.ProjectId);
            }
        }

        _logger.LogInformation(
            "Workspace-inactivity pass: {Live} live checked, {Warned} warned, {Suspended} suspended, {SuspendedTotal} suspended checked, {Deleted} hard-deleted",
            live.Count, warned, suspended, suspendedRows.Count, deleted);
    }
}
