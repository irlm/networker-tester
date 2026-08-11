using Microsoft.EntityFrameworkCore;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Background;

/// <summary>
/// Hourly system-health checker + retention pass — the C# port of the Rust
/// scheduler's health sweep, which was never carried over in phase 3: the
/// READ endpoint (<c>/api/system/health</c>) shipped, the WRITER didn't, so
/// the panel served fossil rows from the retired stack's last sweep
/// (2026-07-15) as if current — including a red <c>logs_retention</c> that
/// was itself honest evidence the designed perf_log pruning had never been
/// implemented ANYWHERE (Rust only documented it; found 2026-08-11).
///
/// <para>Adaptations from the Rust original (single-DB world):</para>
/// <list type="bullet">
///   <item><c>core_db</c> — connectivity; <c>core_db_size</c> — same 3/5 GB
///   thresholds.</item>
///   <item><c>logs_retention</c> — oldest perf_log row age, 7/8-day
///   thresholds, kept green by the NEW pruning pass below. perf_log is
///   unmapped Rust-era schema, so a fresh install may not have the table:
///   guarded via <c>to_regclass</c>, reported green/"no table".</item>
///   <item><c>logs_db</c>/<c>logs_db_size</c> are retired — there is no
///   separate logs database; their fossil rows age out via cleanup.</item>
///   <item>Pruning implemented (7-day window per the DR design docs):
///   perf_log rows and system_health rows older than the window are
///   deleted each tick.</item>
/// </list>
/// </summary>
public sealed class SystemHealthService : BackgroundService
{
    public static readonly TimeSpan TickInterval = TimeSpan.FromHours(1);

    /// <summary>Retention window for perf_log and system_health rows.</summary>
    public const int RetentionDays = 7;

    private readonly IServiceScopeFactory _scopeFactory;
    private readonly ILogger<SystemHealthService> _logger;
    private readonly TickMonitor _monitor;
    private readonly PgAdvisoryLeaderLock? _leader;

    public SystemHealthService(
        IServiceScopeFactory scopeFactory,
        ILogger<SystemHealthService> logger,
        TickMonitor monitor,
        PgAdvisoryLeaderLock? leaderLock = null)
    {
        _scopeFactory = scopeFactory;
        _logger = logger;
        _monitor = monitor;
        _leader = leaderLock;
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        _logger.LogInformation(
            "System-health checker started (tick every {Minutes}m, retention {Days}d)",
            TickInterval.TotalMinutes, RetentionDays);
        _monitor.ReportStarted(OpsServiceNames.SystemHealth);

        // Immediate first tick: a deploy should replace fossil rows with
        // fresh ones right away, not an hour later.
        await GuardedTickAsync(stoppingToken).ConfigureAwait(false);

        using var timer = new PeriodicTimer(TickInterval);
        while (await timer.WaitForNextTickAsync(stoppingToken).ConfigureAwait(false))
        {
            await GuardedTickAsync(stoppingToken).ConfigureAwait(false);
        }
    }

    private async Task GuardedTickAsync(CancellationToken stoppingToken)
    {
        try
        {
            var ranAsLeader = await _leader
                .TryRunGuardedAsync(LeaderLockKeys.SystemHealth, TickAsync, stoppingToken)
                .ConfigureAwait(false);
            if (!ranAsLeader)
            {
                _logger.LogDebug("System-health tick skipped — another replica holds the leader lock");
            }
        }
        catch (OperationCanceledException) when (stoppingToken.IsCancellationRequested)
        {
            // shutting down
        }
        catch (Exception ex)
        {
            _monitor.ReportError(OpsServiceNames.SystemHealth, ex);
            _logger.LogError(ex, "System-health tick failed");
        }
    }

    private async Task TickAsync(CancellationToken ct)
    {
        using var scope = _scopeFactory.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
        var written = await RunOnceAsync(db, _logger, ct).ConfigureAwait(false);
        _monitor.ReportTick(OpsServiceNames.SystemHealth, written, "checks written");
    }

    /// <summary>
    /// One full sweep: prune, compute the three checks, persist, clean old
    /// health rows. Static so tests drive it directly against the fixture's
    /// real Postgres without hosting the service.
    /// </summary>
    internal static async Task<int> RunOnceAsync(
        NetworkerDbContext db,
        ILogger logger,
        CancellationToken ct)
    {
        var now = DateTime.UtcNow;

        // ── Retention pass (what keeps logs_retention green) ──────────────
        var perfLogExists = await PerfLogTableExistsAsync(db, ct).ConfigureAwait(false);
        if (perfLogExists)
        {
            var pruned = await db.Database.ExecuteSqlRawAsync(
                $"DELETE FROM perf_log WHERE logged_at < now() - interval '{RetentionDays} days'",
                ct).ConfigureAwait(false);
            if (pruned > 0)
            {
                logger.LogInformation("Pruned {Count} perf_log rows past the {Days}d window", pruned, RetentionDays);
            }
        }

        // ── Checks ─────────────────────────────────────────────────────────
        var checks = new List<SystemHealth>();

        // core_db: connectivity.
        bool canConnect;
        try
        {
            canConnect = await db.Database.CanConnectAsync(ct).ConfigureAwait(false);
        }
        catch (Exception)
        {
            canConnect = false;
        }
        checks.Add(new SystemHealth
        {
            CheckName = "core_db",
            Status = canConnect ? "green" : "red",
            Message = canConnect ? null : "Connect failed",
            CheckedAt = now,
        });

        // core_db_size: same 3/5 GB thresholds as the Rust checker.
        try
        {
            var bytes = await db.Database
                .SqlQueryRaw<long>("SELECT pg_database_size(current_database()) AS \"Value\"")
                .SingleAsync(ct).ConfigureAwait(false);
            var gb = bytes / 1_073_741_824.0;
            checks.Add(new SystemHealth
            {
                CheckName = "core_db_size",
                Status = gb > 5.0 ? "red" : gb > 3.0 ? "yellow" : "green",
                Value = $"{gb.ToString("F2", System.Globalization.CultureInfo.InvariantCulture)} GB",
                CheckedAt = now,
            });
        }
        catch (Exception ex)
        {
            logger.LogError(ex, "Health check: DB size query failed");
            checks.Add(new SystemHealth
            {
                CheckName = "core_db_size",
                Status = "red",
                Message = "Query error",
                CheckedAt = now,
            });
        }

        // logs_retention: oldest perf_log row age (7/8-day thresholds).
        if (!perfLogExists)
        {
            checks.Add(new SystemHealth
            {
                CheckName = "logs_retention",
                Status = "green",
                Value = "no table",
                Message = "perf_log table not present on this install",
                CheckedAt = now,
            });
        }
        else
        {
            try
            {
                var oldest = await db.Database
                    .SqlQueryRaw<DateTime?>("SELECT MIN(logged_at) AS \"Value\" FROM perf_log")
                    .SingleAsync(ct).ConfigureAwait(false);
                if (oldest is null)
                {
                    checks.Add(new SystemHealth
                    {
                        CheckName = "logs_retention",
                        Status = "green",
                        Value = "empty",
                        CheckedAt = now,
                    });
                }
                else
                {
                    var ageDays = (int)(now - oldest.Value).TotalDays;
                    checks.Add(new SystemHealth
                    {
                        CheckName = "logs_retention",
                        Status = ageDays > RetentionDays + 1 ? "red" : ageDays > RetentionDays ? "yellow" : "green",
                        Value = $"{ageDays} days",
                        CheckedAt = now,
                    });
                }
            }
            catch (Exception ex)
            {
                logger.LogError(ex, "Health check: retention query failed");
                checks.Add(new SystemHealth
                {
                    CheckName = "logs_retention",
                    Status = "red",
                    Message = "Query error",
                    CheckedAt = now,
                });
            }
        }

        db.SystemHealths.AddRange(checks);
        await db.SaveChangesAsync(ct).ConfigureAwait(false);

        // ── Cleanup: age out old health rows (and with them, the retired
        // logs_db/logs_db_size fossil names). ─────────────────────────────
        var cutoff = now.AddDays(-RetentionDays);
        await db.SystemHealths
            .Where(h => h.CheckedAt < cutoff)
            .ExecuteDeleteAsync(ct).ConfigureAwait(false);

        return checks.Count;
    }

    private static async Task<bool> PerfLogTableExistsAsync(NetworkerDbContext db, CancellationToken ct)
    {
        try
        {
            var reg = await db.Database
                .SqlQueryRaw<string?>("SELECT to_regclass('perf_log')::text AS \"Value\"")
                .SingleAsync(ct).ConfigureAwait(false);
            return reg is not null;
        }
        catch (Exception)
        {
            return false;
        }
    }
}
