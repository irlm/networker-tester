namespace Networker.Agent.Tests;

/// <summary>
/// The assign_run dedupe memory (quality audit F5): <see cref="RecentRunSet"/>
/// is the seam <c>AgentWorker.HandleAssignRun</c> consults so a run this agent
/// already executed to a terminal state is dropped when the dashboard's
/// redispatcher re-delivers it — instead of executing the whole run twice and
/// emitting a duplicate run_started/run_finished pair. These tests pin the
/// contract: membership after Add, bounded capacity, and strict FIFO eviction.
/// </summary>
public class RecentRunSetTests
{
    [Fact]
    public void Added_ids_are_contained_unknown_ids_are_not()
    {
        var set = new RecentRunSet(capacity: 4);
        var known = Guid.NewGuid();

        set.Add(known);

        Assert.True(set.Contains(known));
        Assert.False(set.Contains(Guid.NewGuid()));
    }

    [Fact]
    public void Capacity_is_enforced_with_fifo_eviction()
    {
        const int capacity = 128; // the AgentWorker.RecentlyFinishedCapacity value
        var set = new RecentRunSet(capacity);

        var ids = Enumerable.Range(0, capacity + 10).Select(_ => Guid.NewGuid()).ToList();
        foreach (var id in ids)
            set.Add(id);

        // The 10 OLDEST ids were evicted; the newest `capacity` remain.
        foreach (var evicted in ids.Take(10))
            Assert.False(set.Contains(evicted));
        foreach (var kept in ids.Skip(10))
            Assert.True(set.Contains(kept));
    }

    [Fact]
    public void Duplicate_add_does_not_double_count_against_capacity()
    {
        var set = new RecentRunSet(capacity: 2);
        var a = Guid.NewGuid();
        var b = Guid.NewGuid();

        set.Add(a);
        set.Add(a); // repeat — must not consume a second slot
        set.Add(b);

        // Both still present: the duplicate add of `a` didn't evict anything.
        Assert.True(set.Contains(a));
        Assert.True(set.Contains(b));
    }

    [Fact]
    public void Concurrent_adds_and_lookups_do_not_corrupt_the_set()
    {
        // Run tasks finish on the thread pool while assigns arrive on the
        // receive loop — hammer the set from several threads and verify the
        // invariants still hold (no throw, newest ids retained).
        var set = new RecentRunSet(capacity: 64);
        var perThread = 500;

        Parallel.For(0, 4, _ =>
        {
            for (var i = 0; i < perThread; i++)
            {
                var id = Guid.NewGuid();
                set.Add(id);
                // Exercise the read path under contention (the id may already
                // have been evicted by other threads — only no-throw matters).
                _ = set.Contains(id);
            }
        });

        var last = Guid.NewGuid();
        set.Add(last);
        Assert.True(set.Contains(last));
    }
}
