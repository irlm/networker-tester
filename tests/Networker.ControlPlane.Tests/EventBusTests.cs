using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Realtime;

namespace Networker.ControlPlane.Tests;

/// Unit tests for the browser EventBus — the replay + sequence-number contract
/// the React client relies on for gap recovery (?since=). SignalR provides no
/// replay, so this custom logic is the highest-risk part of M2 and is pinned
/// here. The bus is resolved from a real (clientless) SignalR DI graph, so
/// Publish broadcasts to nobody and only the seq/ring-buffer logic is exercised.
public sealed class EventBusTests
{
    private static EventBus NewBus()
    {
        var services = new ServiceCollection();
        services.AddLogging(b => b.SetMinimumLevel(LogLevel.Warning));
        services.AddSignalR();
        services.AddDashboardEventBus();
        return services.BuildServiceProvider().GetRequiredService<EventBus>();
    }

    private static JobUpdate Evt(string status = "running") =>
        new(Guid.NewGuid(), status, null, null, null);

    [Fact]
    public void Publish_assigns_monotonic_seq_starting_at_1()
    {
        var bus = NewBus();

        Assert.Equal(1, bus.Publish(Evt()));
        Assert.Equal(2, bus.Publish(Evt()));
        Assert.Equal(3, bus.Publish(Evt()));
        Assert.Equal(3, bus.CurrentSeq());
    }

    [Fact]
    public void Replay_returns_only_events_after_since_in_order()
    {
        var bus = NewBus();
        for (var i = 0; i < 5; i++) bus.Publish(Evt());

        var replay = bus.Replay(since: 2);

        Assert.Equal(new long[] { 3, 4, 5 }, replay.Select(e => e.Seq).ToArray());
    }

    [Fact]
    public void Replay_since_zero_returns_all_buffered()
    {
        var bus = NewBus();
        for (var i = 0; i < 4; i++) bus.Publish(Evt());

        Assert.Equal(4, bus.Replay(0).Count);
    }

    [Fact]
    public void Replay_at_or_beyond_head_returns_empty()
    {
        var bus = NewBus();
        bus.Publish(Evt());
        bus.Publish(Evt());

        Assert.Empty(bus.Replay(since: 2));
        Assert.Empty(bus.Replay(since: 99));
    }

    [Fact]
    public void Ring_buffer_evicts_oldest_beyond_capacity()
    {
        var bus = NewBus();
        var total = EventBus.EventLogCapacity + 100;
        for (var i = 0; i < total; i++) bus.Publish(Evt());

        // seq keeps climbing past capacity...
        Assert.Equal(total, bus.CurrentSeq());
        // ...but the buffer holds at most `capacity` events, and the oldest
        // (seq 1..100) have been evicted — a client asking to replay from 0
        // gets the newest window, not everything since the dawn of time.
        var all = bus.Replay(0);
        Assert.True(all.Count <= EventBus.EventLogCapacity);
        Assert.Equal(total, all[^1].Seq);
        Assert.True(all[0].Seq > 100);
    }
}
