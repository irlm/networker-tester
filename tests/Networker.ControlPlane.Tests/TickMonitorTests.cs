using Networker.ControlPlane.Background;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// TickMonitor registry semantics: started/tick/error reporting, snapshot
/// consistency, and thread safety. Time is a fake <see cref="TimeProvider"/> so
/// no test sleeps.
/// </summary>
public class TickMonitorTests
{
    private sealed class FakeTime : TimeProvider
    {
        public DateTimeOffset Now { get; set; } = new(2026, 7, 14, 12, 0, 0, TimeSpan.Zero);
        public override DateTimeOffset GetUtcNow() => Now;
    }

    [Fact]
    public void ReportStarted_registers_service_with_zero_ticks()
    {
        var time = new FakeTime();
        var monitor = new TickMonitor(time);

        monitor.ReportStarted("scheduler");

        var s = Assert.Single(monitor.Snapshot());
        Assert.Equal("scheduler", s.Service);
        Assert.Equal(time.Now, s.StartedAt);
        Assert.Null(s.LastTickAt);
        Assert.Equal(0, s.TicksTotal);
        Assert.Null(s.LastError);
    }

    [Fact]
    public void ReportStarted_is_idempotent_and_keeps_first_start_time()
    {
        var time = new FakeTime();
        var monitor = new TickMonitor(time);
        var first = time.Now;

        monitor.ReportStarted("scheduler");
        time.Now = first.AddMinutes(5);
        monitor.ReportStarted("scheduler");

        var s = Assert.Single(monitor.Snapshot());
        Assert.Equal(first, s.StartedAt);
    }

    [Fact]
    public void ReportTick_updates_all_tick_fields()
    {
        var time = new FakeTime();
        var monitor = new TickMonitor(time);
        monitor.ReportStarted("watchdog");

        time.Now = time.Now.AddSeconds(60);
        monitor.ReportTick("watchdog", 3, "reaped_running=1 reaped_queued=2");
        time.Now = time.Now.AddSeconds(60);
        monitor.ReportTick("watchdog", 0);

        var s = Assert.Single(monitor.Snapshot());
        Assert.Equal(time.Now, s.LastTickAt);
        Assert.Equal(2, s.TicksTotal);
        Assert.Equal(0, s.LastItems);
        Assert.Null(s.LastNote); // note is per-tick, not sticky
        Assert.Null(s.LastError);
    }

    [Fact]
    public void ReportTick_without_ReportStarted_still_registers()
    {
        var monitor = new TickMonitor(new FakeTime());

        monitor.ReportTick("orphan-reaper", 1);

        var s = Assert.Single(monitor.Snapshot());
        Assert.Equal(1, s.TicksTotal);
        Assert.NotNull(s.LastTickAt);
    }

    [Fact]
    public void ReportError_records_error_without_clobbering_tick_history()
    {
        var time = new FakeTime();
        var monitor = new TickMonitor(time);
        monitor.ReportTick("scheduler", 5, "launched=5");

        var tickAt = time.Now;
        time.Now = time.Now.AddSeconds(30);
        monitor.ReportError("scheduler", new InvalidOperationException("db unreachable"));

        var s = Assert.Single(monitor.Snapshot());
        Assert.Equal("InvalidOperationException: db unreachable", s.LastError);
        Assert.Equal(time.Now, s.LastErrorAt);
        // Tick history survives the error report.
        Assert.Equal(tickAt, s.LastTickAt);
        Assert.Equal(1, s.TicksTotal);
        Assert.Equal(5, s.LastItems);
    }

    [Fact]
    public void Successful_tick_after_error_keeps_last_error_visible()
    {
        // last_error is deliberately sticky (until the next error) so a soak
        // review can see that a loop failed at some point even if it recovered.
        var monitor = new TickMonitor(new FakeTime());
        monitor.ReportError("scheduler", new TimeoutException("boom"));
        monitor.ReportTick("scheduler", 1);

        var s = Assert.Single(monitor.Snapshot());
        Assert.Equal(1, s.TicksTotal);
        Assert.Contains("boom", s.LastError);
    }

    [Fact]
    public void Snapshot_is_sorted_by_service_name()
    {
        var monitor = new TickMonitor(new FakeTime());
        monitor.ReportStarted("watchdog");
        monitor.ReportStarted("agent-reaper");
        monitor.ReportStarted("scheduler");

        var names = monitor.Snapshot().Select(s => s.Service).ToList();
        Assert.Equal(["agent-reaper", "scheduler", "watchdog"], names);
    }

    [Fact]
    public async Task Concurrent_ticks_never_lose_a_count()
    {
        var monitor = new TickMonitor(new FakeTime());
        const int perWorker = 500;

        await Task.WhenAll(Enumerable.Range(0, 8).Select(_ => Task.Run(() =>
        {
            for (var i = 0; i < perWorker; i++)
            {
                monitor.ReportTick("scheduler", i);
            }
        })));

        var s = Assert.Single(monitor.Snapshot());
        Assert.Equal(8L * perWorker, s.TicksTotal);
    }
}
