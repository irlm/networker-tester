using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.Logging.Abstractions;
using Networker.ControlPlane.Background;
using Npgsql;
using Xunit;

namespace Networker.Tests;

/// <summary>
/// Audit P2: leader election was covered only by <c>LeaderLockKeysTests</c>,
/// which checks the FNV-1a key derivation — a pure function. Nothing exercised
/// the behaviour the whole mechanism exists for.
///
/// <para><c>PgAdvisoryLeaderLock</c> is what stops two control-plane replicas
/// running the same background tick concurrently: duplicate schedule fan-out,
/// double VM deallocation, two reapers racing on the same rows. Its contract
/// has four parts, and a break in any of them is silent — the app keeps
/// working, it just does everything twice, or stops doing it at all:</para>
///
/// <list type="number">
///   <item><b>Mutual exclusion</b> — while one replica's tick is in flight,
///   another must be refused and must NOT run its tick body.</item>
///   <item><b>Release on every path</b> — after success, after the tick throws,
///   and after cancellation. A leaked key silently disables that loop for the
///   rest of the pooled session's life, which looks exactly like "the loop
///   isn't finding anything to do".</item>
///   <item><b>Failover on process death</b> — the design has no lease and no
///   heartbeat: the DB session IS the lease. If a replica dies mid-tick,
///   Postgres must release the key so a survivor takes over.</item>
///   <item><b>Key isolation</b> — different services must not block each
///   other.</item>
/// </list>
/// </summary>
public class LeaderElectionFailoverTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fx;

    public LeaderElectionFailoverTests(ControlPlaneFixture fx) => _fx = fx;

    /// <summary>Distinct key per test so tests can run in any order without
    /// contending with each other (the whole point of the mechanism).</summary>
    private static long FreshKey() => Random.Shared.NextInt64(1_000_000, long.MaxValue);

    private string ConnString()
    {
        using var db = _fx.NewDbContext();
        return db.Database.GetConnectionString()!;
    }

    /// <summary>A lock instance with its OWN data source — i.e. a separate
    /// "replica". Sharing one data source would still give separate sessions,
    /// but separate sources model the real topology more honestly.</summary>
    private (PgAdvisoryLeaderLock Lock, NpgsqlDataSource Source) NewReplica()
    {
        var source = NpgsqlDataSource.Create(ConnString());
        return (new PgAdvisoryLeaderLock(source, NullLogger<PgAdvisoryLeaderLock>.Instance), source);
    }

    [Fact]
    public async Task Only_one_replica_runs_a_tick_at_a_time()
    {
        var key = FreshKey();
        var (a, srcA) = NewReplica();
        var (b, srcB) = NewReplica();
        await using var _1 = srcA;
        await using var _2 = srcB;

        var aInside = new TaskCompletionSource();
        var releaseA = new TaskCompletionSource();
        var bTickRan = false;

        // Replica A takes the key and parks inside its tick.
        var aRun = a.TryRunAsLeaderAsync(key, async _ =>
        {
            aInside.SetResult();
            await releaseA.Task;
        }, CancellationToken.None);

        await aInside.Task.WaitAsync(TimeSpan.FromSeconds(10));

        // …while B tries the same key.
        var bAcquired = await b.TryRunAsLeaderAsync(key, _ =>
        {
            bTickRan = true;
            return Task.CompletedTask;
        }, CancellationToken.None);

        Assert.False(bAcquired,
            "a second replica acquired a key already held — both would run the same tick concurrently");
        Assert.False(bTickRan,
            "the tick body RAN on the replica that lost the election — returning false is not enough, "
            + "the work must not happen");

        releaseA.SetResult();
        Assert.True(await aRun);
    }

    [Fact]
    public async Task The_key_is_released_after_a_successful_tick()
    {
        var key = FreshKey();
        var (a, srcA) = NewReplica();
        var (b, srcB) = NewReplica();
        await using var _1 = srcA;
        await using var _2 = srcB;

        Assert.True(await a.TryRunAsLeaderAsync(key, _ => Task.CompletedTask, CancellationToken.None));

        var bRan = false;
        var acquired = await b.TryRunAsLeaderAsync(key, _ => { bRan = true; return Task.CompletedTask; },
            CancellationToken.None);

        Assert.True(acquired, "the key was still held after a clean tick — that loop is now wedged");
        Assert.True(bRan);
    }

    [Fact]
    public async Task A_throwing_tick_still_releases_the_key()
    {
        // The nastiest leak: one bad tick would disable the loop permanently on
        // this session, and it looks identical to "nothing to do".
        var key = FreshKey();
        var (a, srcA) = NewReplica();
        var (b, srcB) = NewReplica();
        await using var _1 = srcA;
        await using var _2 = srcB;

        await Assert.ThrowsAsync<InvalidOperationException>(() =>
            a.TryRunAsLeaderAsync(key, _ => throw new InvalidOperationException("tick blew up"),
                CancellationToken.None));

        var acquired = await b.TryRunAsLeaderAsync(key, _ => Task.CompletedTask, CancellationToken.None);
        Assert.True(acquired,
            "a tick that threw left its advisory lock held — the loop stays dead until the "
            + "pooled session is recycled");
    }

    [Fact]
    public async Task A_cancelled_tick_still_releases_the_key()
    {
        // Shutdown cancels ticks. If cancellation skipped the unlock, a rolling
        // restart would progressively wedge loops.
        var key = FreshKey();
        var (a, srcA) = NewReplica();
        var (b, srcB) = NewReplica();
        await using var _1 = srcA;
        await using var _2 = srcB;

        using var cts = new CancellationTokenSource();
        await Assert.ThrowsAnyAsync<OperationCanceledException>(() =>
            a.TryRunAsLeaderAsync(key, async ct =>
            {
                cts.Cancel();
                await Task.Delay(Timeout.Infinite, ct);
            }, cts.Token));

        var acquired = await b.TryRunAsLeaderAsync(key, _ => Task.CompletedTask, CancellationToken.None);
        Assert.True(acquired, "a cancelled tick left its advisory lock held");
    }

    [Fact]
    public async Task A_dead_replica_hands_leadership_to_a_survivor()
    {
        // The design has no lease and no heartbeat — "the DB session IS the
        // lease". That claim is only true if Postgres really drops the key when
        // the session dies, so assert it rather than trusting the comment.
        var key = FreshKey();
        var connString = ConnString();

        // A raw connection standing in for a replica that is about to be killed.
        var doomed = new NpgsqlConnection(connString);
        await doomed.OpenAsync();

        int doomedPid;
        await using (var cmd = doomed.CreateCommand())
        {
            cmd.CommandText = "SELECT pg_backend_pid()";
            doomedPid = (int)(await cmd.ExecuteScalarAsync())!;
        }
        await using (var cmd = doomed.CreateCommand())
        {
            cmd.CommandText = "SELECT pg_try_advisory_lock(@key)";
            cmd.Parameters.AddWithValue("key", key);
            Assert.True((bool)(await cmd.ExecuteScalarAsync())!);
        }

        var (survivor, srcS) = NewReplica();
        await using var _1 = srcS;

        // While the doomed session lives, the survivor must be locked out.
        Assert.False(
            await survivor.TryRunAsLeaderAsync(key, _ => Task.CompletedTask, CancellationToken.None),
            "the survivor acquired a key held by a live replica");

        // Kill it the way a crash or an OOM would.
        await using (var killer = NpgsqlDataSource.Create(connString))
        await using (var conn = await killer.OpenConnectionAsync())
        await using (var cmd = conn.CreateCommand())
        {
            cmd.CommandText = "SELECT pg_terminate_backend(@pid)";
            cmd.Parameters.AddWithValue("pid", doomedPid);
            await cmd.ExecuteScalarAsync();
        }
        try { await doomed.DisposeAsync(); } catch { /* already terminated */ }

        // Postgres releases session-scoped locks asynchronously on backend exit.
        var deadline = DateTime.UtcNow.AddSeconds(15);
        var tookOver = false;
        while (DateTime.UtcNow < deadline && !tookOver)
        {
            tookOver = await survivor.TryRunAsLeaderAsync(
                key, _ => Task.CompletedTask, CancellationToken.None);
            if (!tookOver)
            {
                await Task.Delay(250);
            }
        }

        Assert.True(tookOver,
            "no replica took over after the leader's session died — with no lease and no "
            + "heartbeat, that loop would never run again");
    }

    [Fact]
    public async Task Different_services_do_not_block_each_other()
    {
        var (a, srcA) = NewReplica();
        var (b, srcB) = NewReplica();
        await using var _1 = srcA;
        await using var _2 = srcB;

        var aInside = new TaskCompletionSource();
        var releaseA = new TaskCompletionSource();

        var aRun = a.TryRunAsLeaderAsync(LeaderLockKeys.Scheduler, async _ =>
        {
            aInside.SetResult();
            await releaseA.Task;
        }, CancellationToken.None);
        await aInside.Task.WaitAsync(TimeSpan.FromSeconds(10));

        var otherRan = await b.TryRunAsLeaderAsync(
            LeaderLockKeys.QueuedRedispatch, _ => Task.CompletedTask, CancellationToken.None);

        Assert.True(otherRan,
            "holding the scheduler's key blocked a DIFFERENT service — the keys collide, "
            + "which would serialize unrelated loops");

        releaseA.SetResult();
        await aRun;
    }

    [Fact]
    public void Every_service_key_is_distinct()
    {
        // Guards the guard above: if two services derived the SAME key, the
        // isolation test would be asserting nothing meaningful.
        var keys = new[]
        {
            LeaderLockKeys.Scheduler,
            LeaderLockKeys.QueuedRedispatch,
        };
        Assert.Equal(keys.Length, keys.Distinct().Count());
    }
}
