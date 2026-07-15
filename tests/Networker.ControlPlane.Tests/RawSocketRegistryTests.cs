using Microsoft.Extensions.Logging.Abstractions;
using Networker.ControlPlane.Realtime.RawWs;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Unit tests for the raw-WS send-queue/backpressure machinery, using fake send
/// delegates instead of real sockets (the channel logic is socket-independent by
/// design). Covers: overflow ejection (the Rust slow-subscriber drop), the
/// replay-watermark dedup, ordered single-writer pumping, registry fan-out, and
/// the subscribe rate limiter.
/// </summary>
public sealed class RawSocketRegistryTests
{
    private static RawSocketRegistry NewRegistry() =>
        new(NullLogger<RawSocketRegistry>.Instance);

    // ── RawSocketConnection: overflow ejection ────────────────────────────────

    [Fact]
    public void TryEnqueue_QueueOverflow_DropsConnection_AndFiresCallback()
    {
        var droppedCallbacks = 0;
        var conn = new RawSocketConnection(
            "c1",
            (_, _) => Task.CompletedTask, // pump never started — queue only fills
            onDropped: _ => Interlocked.Increment(ref droppedCallbacks),
            capacity: 3);

        Assert.True(conn.TryEnqueue("{\"a\":1}"));
        Assert.True(conn.TryEnqueue("{\"a\":2}"));
        Assert.True(conn.TryEnqueue("{\"a\":3}"));
        Assert.False(conn.IsDropped);

        // 4th frame overflows the bounded queue → ejected.
        Assert.False(conn.TryEnqueue("{\"a\":4}"));
        Assert.True(conn.IsDropped);
        Assert.Equal(1, droppedCallbacks);

        // Once dropped, everything is refused and the callback never re-fires.
        Assert.False(conn.TryEnqueue("{\"a\":5}"));
        Assert.Equal(1, droppedCallbacks);
    }

    [Fact]
    public async Task SendPump_DeliversFramesInOrder_ThroughSingleSender()
    {
        var sent = new List<string>();
        var conn = new RawSocketConnection(
            "c1",
            (json, _) =>
            {
                lock (sent) { sent.Add(json); }
                return Task.CompletedTask;
            });

        Assert.True(conn.TryEnqueue("first"));
        Assert.True(conn.TryEnqueue("second"));
        Assert.True(conn.TryEnqueue("third"));
        conn.CompleteQueue();

        await conn.RunSendPumpAsync(CancellationToken.None);

        Assert.Equal(new[] { "first", "second", "third" }, sent);
    }

    [Fact]
    public async Task SendPump_SkipsFramesAtOrBelowReplayWatermark()
    {
        var sent = new List<string>();
        var conn = new RawSocketConnection(
            "c1",
            (json, _) =>
            {
                lock (sent) { sent.Add(json); }
                return Task.CompletedTask;
            });

        // Live frames buffered while the endpoint flushes the replay batch.
        Assert.True(conn.TryEnqueue(4, "seq4-covered-by-replay"));
        Assert.True(conn.TryEnqueue(5, "seq5-covered-by-replay"));
        Assert.True(conn.TryEnqueue(6, "seq6-new"));
        Assert.True(conn.TryEnqueue(0, "no-replay-semantics")); // seq 0 bypasses the gate

        conn.SetReplayWatermark(5);
        conn.CompleteQueue();
        await conn.RunSendPumpAsync(CancellationToken.None);

        Assert.Equal(new[] { "seq6-new", "no-replay-semantics" }, sent);
    }

    [Fact]
    public async Task SendPump_SendFailure_DropsConnection()
    {
        var conn = new RawSocketConnection(
            "c1",
            (_, _) => throw new InvalidOperationException("socket torn down"));

        Assert.True(conn.TryEnqueue("frame"));
        await conn.RunSendPumpAsync(CancellationToken.None);

        Assert.True(conn.IsDropped);
        Assert.False(conn.TryEnqueue("more"));
    }

    // ── Registry: browser fan-out + ejection ──────────────────────────────────

    [Fact]
    public void BroadcastBrowserEvent_ReachesAllRegisteredClients()
    {
        var registry = NewRegistry();
        var a = new RawSocketConnection("a", (_, _) => Task.CompletedTask, capacity: 8);
        var b = new RawSocketConnection("b", (_, _) => Task.CompletedTask, capacity: 8);
        registry.RegisterBrowser(a);
        registry.RegisterBrowser(b);

        registry.BroadcastBrowserEvent(1, "{\"seq\":1}");

        Assert.Equal(2, registry.BrowserClientCount);
        Assert.False(a.IsDropped);
        Assert.False(b.IsDropped);
    }

    [Fact]
    public void BroadcastBrowserEvent_SlowSubscriber_IsEjectedFromRegistry()
    {
        var registry = NewRegistry();
        var slow = new RawSocketConnection("slow", (_, _) => Task.CompletedTask, capacity: 1);
        var healthy = new RawSocketConnection("healthy", (_, _) => Task.CompletedTask, capacity: 16);
        registry.RegisterBrowser(slow);
        registry.RegisterBrowser(healthy);

        // First frame fills slow's queue; second overflows it.
        registry.BroadcastBrowserEvent(1, "{\"seq\":1}");
        registry.BroadcastBrowserEvent(2, "{\"seq\":2}");

        Assert.True(slow.IsDropped);
        Assert.False(healthy.IsDropped);
        Assert.Equal(1, registry.BrowserClientCount); // slow removed, healthy remains
    }

    // ── Registry: tester group fan-out ────────────────────────────────────────

    [Fact]
    public async Task BroadcastTesterGroup_OnlyReachesSubscribersOfThatGroup()
    {
        var registry = NewRegistry();
        var sentA = new List<string>();
        var sentB = new List<string>();
        var a = new RawSocketConnection("a", (j, _) => { lock (sentA) { sentA.Add(j); } return Task.CompletedTask; });
        var b = new RawSocketConnection("b", (j, _) => { lock (sentB) { sentB.Add(j); } return Task.CompletedTask; });

        registry.SubscribeTesterGroup("tq:proj-1:t1", a);
        registry.SubscribeTesterGroup("tq:proj-1:t2", b);

        registry.BroadcastTesterGroup("tq:proj-1:t1", "{\"type\":\"tester_queue_update\"}");
        registry.BroadcastTesterGroup("tq:proj-9:missing", "{\"type\":\"noop\"}"); // no such group — no-op

        a.CompleteQueue();
        b.CompleteQueue();
        await a.RunSendPumpAsync(CancellationToken.None);
        await b.RunSendPumpAsync(CancellationToken.None);

        Assert.Equal(new[] { "{\"type\":\"tester_queue_update\"}" }, sentA);
        Assert.Empty(sentB);
    }

    [Fact]
    public void BroadcastTesterGroup_OverflowingSubscriber_IsEjectedFromGroup()
    {
        var registry = NewRegistry();
        var slow = new RawSocketConnection("slow", (_, _) => Task.CompletedTask, capacity: 1);
        registry.SubscribeTesterGroup("tq:p:t", slow);
        Assert.True(registry.HasTesterGroup("tq:p:t"));

        registry.BroadcastTesterGroup("tq:p:t", "one");   // fills the queue
        registry.BroadcastTesterGroup("tq:p:t", "two");   // overflow → eject

        Assert.True(slow.IsDropped);
        Assert.False(registry.HasTesterGroup("tq:p:t"));
    }

    [Fact]
    public void RemoveTesterConnection_ClearsEveryGroupMembership()
    {
        var registry = NewRegistry();
        var conn = new RawSocketConnection("c", (_, _) => Task.CompletedTask);
        registry.SubscribeTesterGroup("tq:p1:t1", conn);
        registry.SubscribeTesterGroup("tq:p2:t2", conn);

        registry.RemoveTesterConnection(conn);

        Assert.False(registry.HasTesterGroup("tq:p1:t1"));
        Assert.False(registry.HasTesterGroup("tq:p2:t2"));
    }

    // ── Rate limiter ──────────────────────────────────────────────────────────

    [Fact]
    public void SlidingWindowRateLimiter_EnforcesCap()
    {
        var limiter = new SlidingWindowRateLimiter(cap: 3);
        Assert.True(limiter.Allow());
        Assert.True(limiter.Allow());
        Assert.True(limiter.Allow());
        Assert.False(limiter.Allow()); // 4th within the window is rejected
    }

    [Fact]
    public async Task SlidingWindowRateLimiter_WindowSlides()
    {
        var limiter = new SlidingWindowRateLimiter(cap: 1, window: TimeSpan.FromMilliseconds(50));
        Assert.True(limiter.Allow());
        Assert.False(limiter.Allow());

        await Task.Delay(120);
        Assert.True(limiter.Allow()); // old stamp expired out of the window
    }
}
