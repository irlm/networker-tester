using System.Text;
using Microsoft.Extensions.DependencyInjection.Extensions;
using Npgsql;

namespace Networker.ControlPlane.Background;

/// <summary>
/// Per-tick leader election on Postgres advisory locks — the M6 replacement for
/// running background loops on faith that only one replica hosts them (see
/// <see cref="BackgroundServicesGate"/>, which remains as a coarse manual gate).
///
/// <para><b>Model: per-tick locking, not a leased leadership term.</b> Every
/// background service wraps each tick in
/// <see cref="TryRunAsLeaderAsync"/>: open a pooled connection, attempt
/// <c>pg_try_advisory_lock(key)</c> (session-level, non-blocking); if another
/// replica holds the key this tick is simply skipped (it will be retried on the
/// service's own next interval); if acquired, run the tick and release with
/// <c>pg_advisory_unlock(key)</c> <b>on the same connection</b> — advisory locks
/// are session-scoped, so the unlock MUST ride the session that locked. If the
/// process dies mid-tick the session drops and Postgres releases the lock
/// automatically, so any surviving replica takes over on its next tick. No
/// lease renewal, no fencing tokens, no heartbeat table — the DB session IS the
/// lease.</para>
///
/// <para><b>Why this is sufficient here:</b> every loop in this codebase is
/// written to be idempotent-per-tick (guarded <c>ExecuteUpdate</c> transitions,
/// per-row catch blocks) and tolerant of a missed tick. The only hazard of
/// multiple replicas is two ticks of the same loop running <i>concurrently</i>
/// (duplicate schedule fan-out, double VM deallocation) — exactly what a
/// per-tick mutex removes. Ticks from different replicas may interleave over
/// time; that is fine and intended.</para>
///
/// <para><b>Failure semantics:</b> if the DB is unreachable the lock attempt
/// throws; the caller's existing per-tick catch logs it and the tick is skipped
/// — identical blast radius to the tick itself failing on its first query.</para>
/// </summary>
public sealed class PgAdvisoryLeaderLock
{
    private readonly NpgsqlDataSource _dataSource;
    private readonly ILogger<PgAdvisoryLeaderLock> _logger;

    public PgAdvisoryLeaderLock(NpgsqlDataSource dataSource, ILogger<PgAdvisoryLeaderLock> logger)
    {
        _dataSource = dataSource;
        _logger = logger;
    }

    /// <summary>
    /// Attempt to run <paramref name="tick"/> as the leader for
    /// <paramref name="lockKey"/>. Returns <c>false</c> without running the tick
    /// when another session currently holds the key (another replica is running
    /// this service's tick right now); returns <c>true</c> after the tick ran
    /// (its exceptions propagate to the caller — the lock is still released).
    /// </summary>
    public async Task<bool> TryRunAsLeaderAsync(
        long lockKey, Func<CancellationToken, Task> tick, CancellationToken ct)
    {
        // The connection must stay open for the whole tick: pg advisory locks
        // are held by the SESSION, and handing the connection back to the pool
        // early would let another consumer inherit (or npgsql reset) it.
        await using var conn = await _dataSource.OpenConnectionAsync(ct).ConfigureAwait(false);

        bool acquired;
        await using (var cmd = conn.CreateCommand())
        {
            cmd.CommandText = "SELECT pg_try_advisory_lock(@key)";
            cmd.Parameters.AddWithValue("key", lockKey);
            acquired = (bool)(await cmd.ExecuteScalarAsync(ct).ConfigureAwait(false))!;
        }

        if (!acquired)
        {
            return false;
        }

        try
        {
            await tick(ct).ConfigureAwait(false);
        }
        finally
        {
            // Unlock on the SAME connection, and never let cancellation skip it:
            // a cancelled tick must not leave the key held for the rest of this
            // session's pooled lifetime. If the unlock itself fails (connection
            // broken), disposing the connection closes the session and Postgres
            // releases the lock anyway — log at debug and move on.
            try
            {
                await using var unlock = conn.CreateCommand();
                unlock.CommandText = "SELECT pg_advisory_unlock(@key)";
                unlock.Parameters.AddWithValue("key", lockKey);
                await unlock.ExecuteScalarAsync(CancellationToken.None).ConfigureAwait(false);
            }
            catch (Exception ex)
            {
                _logger.LogDebug(ex,
                    "pg_advisory_unlock({Key}) failed — the lock is released with the session instead",
                    lockKey);
            }
        }

        return true;
    }
}

/// <summary>
/// The advisory-lock key for each background service, derived as
/// <c>FNV-1a 64-bit over UTF-8 of "networker-controlplane:&lt;service name&gt;"</c>
/// (<see cref="KeyFor"/>). Deriving from the stable <see cref="OpsServiceNames"/>
/// strings (rather than hand-picked integers) makes collisions with other
/// advisory-lock users in the shared database astronomically unlikely and keeps
/// the key reproducible from the name alone — an operator can recompute it, and
/// it never changes across releases unless the service is deliberately renamed.
///
/// <para><b>Debugging:</b> a held key shows up in <c>pg_locks</c> as
/// <c>locktype='advisory'</c> with the 64-bit key split as
/// <c>classid</c> = high 32 bits, <c>objid</c> = low 32 bits, <c>objsubid=1</c>:
/// <code>
/// SELECT (classid::bigint &lt;&lt; 32) | objid::bigint AS key, pid, granted
///   FROM pg_locks WHERE locktype = 'advisory';
/// </code></para>
/// </summary>
public static class LeaderLockKeys
{
    /// <summary>Namespace prefix hashed into every key — keeps our keys out of
    /// the way of any other advisory-lock user sharing the database.</summary>
    public const string KeyNamespace = "networker-controlplane:";

    public static readonly long Scheduler = KeyFor(OpsServiceNames.Scheduler);
    public static readonly long QueuedRedispatch = KeyFor(OpsServiceNames.QueuedRedispatch);
    public static readonly long Watchdog = KeyFor(OpsServiceNames.Watchdog);
    public static readonly long AgentReaper = KeyFor(OpsServiceNames.AgentReaper);
    public static readonly long AutoShutdown = KeyFor(OpsServiceNames.AutoShutdown);
    public static readonly long OrphanReaper = KeyFor(OpsServiceNames.OrphanReaper);
    public static readonly long WorkspaceInactivity = KeyFor(OpsServiceNames.WorkspaceInactivity);
    public static readonly long ProvisioningOrchestrator = KeyFor(OpsServiceNames.ProvisioningOrchestrator);

    /// <summary>
    /// FNV-1a 64-bit of <c>UTF-8("networker-controlplane:" + serviceName)</c>,
    /// reinterpreted as a signed <see cref="long"/> (Postgres advisory keys are
    /// int8). Pure + stable: same input, same key, forever.
    /// </summary>
    public static long KeyFor(string serviceName)
    {
        const ulong fnvOffsetBasis = 14695981039346656037UL;
        const ulong fnvPrime = 1099511628211UL;

        var hash = fnvOffsetBasis;
        foreach (var b in Encoding.UTF8.GetBytes(KeyNamespace + serviceName))
        {
            hash ^= b;
            hash *= fnvPrime; // wraps mod 2^64 (unchecked by default for ulong)
        }

        return unchecked((long)hash);
    }
}

/// <summary>
/// Null-tolerant tick wrapper used by the background services. The lock is an
/// optional constructor dependency (registered by
/// <see cref="OpsInfrastructureExtensions.AddOpsInfrastructure"/>): when the ops
/// infrastructure isn't wired (bare test hosts), the tick runs unguarded —
/// exactly the pre-M6 single-replica behaviour.
/// </summary>
public static class LeaderLockExtensions
{
    /// <summary>
    /// Run <paramref name="tick"/> under the advisory lock when
    /// <paramref name="leader"/> is available, or directly when it is not.
    /// Returns <c>false</c> only when another replica held the lock.
    /// </summary>
    public static async Task<bool> TryRunGuardedAsync(
        this PgAdvisoryLeaderLock? leader,
        long lockKey,
        Func<CancellationToken, Task> tick,
        CancellationToken ct)
    {
        if (leader is null)
        {
            await tick(ct).ConfigureAwait(false);
            return true;
        }

        return await leader.TryRunAsLeaderAsync(lockKey, tick, ct).ConfigureAwait(false);
    }
}

/// <summary>
/// DI wiring for the M6 ops infrastructure. One line in Program.cs (before the
/// background-service registrations, after <c>AddNetworkerAuth</c> which
/// registers the <see cref="NpgsqlDataSource"/> the leader lock rides):
/// <code>
/// builder.Services.AddOpsInfrastructure();
/// </code>
/// and map the endpoints alongside the other Map* calls:
/// <code>
/// app.MapOpsEndpoints();
/// </code>
/// </summary>
public static class OpsInfrastructureExtensions
{
    /// <summary>
    /// Register the <see cref="TickMonitor"/> (background-service observability
    /// registry) and the <see cref="PgAdvisoryLeaderLock"/> (per-tick leader
    /// election) as singletons. <c>TryAdd</c> so a test host can substitute
    /// fakes before calling this.
    /// </summary>
    public static IServiceCollection AddOpsInfrastructure(this IServiceCollection services)
    {
        services.TryAddSingleton<TickMonitor>();
        services.TryAddSingleton<PgAdvisoryLeaderLock>();
        return services;
    }
}
