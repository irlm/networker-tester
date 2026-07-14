using Networker.ControlPlane.Background;

namespace Networker.ControlPlane.Tests;

/// Unit tests for the scheduler's cron→next-fire computation. Pure/static, so
/// no host needed — this is the logic that decides when a schedule fires.
public sealed class ScheduleTimingTests
{
    [Fact]
    public void Daily_cron_returns_next_midnight_utc()
    {
        var from = new DateTime(2026, 1, 1, 12, 0, 0, DateTimeKind.Utc);

        var next = ScheduleTiming.NextFireUtc("0 0 * * *", "UTC", from);

        Assert.Equal(new DateTime(2026, 1, 2, 0, 0, 0, DateTimeKind.Utc), next);
    }

    [Fact]
    public void Blank_timezone_is_treated_as_utc()
    {
        var from = new DateTime(2026, 1, 1, 12, 0, 0, DateTimeKind.Utc);

        var next = ScheduleTiming.NextFireUtc("0 0 * * *", "", from);

        Assert.Equal(new DateTime(2026, 1, 2, 0, 0, 0, DateTimeKind.Utc), next);
    }

    [Fact]
    public void Result_is_strictly_after_the_anchor()
    {
        // Anchored exactly on a fire time — the next occurrence must be the
        // FOLLOWING one, never the anchor itself (no double-fire).
        var onFire = new DateTime(2026, 1, 1, 0, 0, 0, DateTimeKind.Utc);

        var next = ScheduleTiming.NextFireUtc("0 0 * * *", "UTC", onFire);

        Assert.Equal(new DateTime(2026, 1, 2, 0, 0, 0, DateTimeKind.Utc), next);
    }

    [Fact]
    public void Unparseable_cron_returns_null_not_throws()
    {
        var next = ScheduleTiming.NextFireUtc("not a cron", "UTC", DateTime.UtcNow);
        Assert.Null(next);
    }

    [Fact]
    public void Unknown_timezone_returns_null()
    {
        var next = ScheduleTiming.NextFireUtc("0 0 * * *", "Mars/Olympus_Mons", DateTime.UtcNow);
        Assert.Null(next);
    }
}
