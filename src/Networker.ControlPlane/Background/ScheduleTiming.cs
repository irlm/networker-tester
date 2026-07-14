using Cronos;

namespace Networker.ControlPlane.Background;

/// <summary>
/// Pure cron timing helper — the C# port of the Rust scheduler's
/// <c>compute_next_run</c> (<c>crates/networker-dashboard/src/scheduler.rs</c>),
/// upgraded to be time-zone aware (the Rust helper computed the next occurrence
/// in UTC only; here we honour each schedule's IANA/Windows time zone).
///
/// <para>Kept static and side-effect free so it is trivially unit-testable and
/// so a malformed cron expression can never crash the scheduler loop: an
/// unparseable expression (or an unknown time zone) yields <c>null</c> rather
/// than throwing, mirroring the Rust <c>.ok()?</c> semantics.</para>
/// </summary>
public static class ScheduleTiming
{
    /// <summary>
    /// Compute the next fire time, in UTC, strictly after <paramref name="fromUtc"/>.
    /// </summary>
    /// <param name="cronExpr">
    /// A 5-field (minute-precision) or 6-field (with-seconds) cron expression.
    /// </param>
    /// <param name="timezone">
    /// An IANA (e.g. <c>"America/New_York"</c>) or Windows time zone id used to
    /// resolve the cron occurrence. Empty/whitespace falls back to UTC.
    /// </param>
    /// <param name="fromUtc">The instant (UTC) to compute the next occurrence after.</param>
    /// <returns>
    /// The next occurrence as a UTC <see cref="DateTime"/> (<see cref="DateTimeKind.Utc"/>),
    /// or <c>null</c> when the cron is unparseable, the time zone is unknown, or there
    /// is no future occurrence.
    /// </returns>
    public static DateTime? NextFireUtc(string cronExpr, string timezone, DateTime fromUtc)
    {
        if (string.IsNullOrWhiteSpace(cronExpr))
        {
            return null;
        }

        CronExpression cron;
        try
        {
            // Cronos infers the field count: 5 fields => Standard, 6 => WithSeconds.
            var fields = cronExpr.Trim().Split(
                ' ', StringSplitOptions.RemoveEmptyEntries).Length;
            var format = fields >= 6 ? CronFormat.IncludeSeconds : CronFormat.Standard;
            cron = CronExpression.Parse(cronExpr.Trim(), format);
        }
        catch (CronFormatException)
        {
            return null;
        }

        var tz = ResolveTimeZone(timezone);
        if (tz is null)
        {
            return null;
        }

        // Ensure the anchor is a UTC instant regardless of the incoming Kind.
        var anchor = fromUtc.Kind == DateTimeKind.Utc
            ? fromUtc
            : DateTime.SpecifyKind(fromUtc, DateTimeKind.Utc);

        // inclusive:false => strictly after the anchor, matching Rust's
        // schedule.upcoming(...).next() (never returns the anchor itself).
        var next = cron.GetNextOccurrence(anchor, tz, inclusive: false);
        return next;
    }

    private static TimeZoneInfo? ResolveTimeZone(string? timezone)
    {
        if (string.IsNullOrWhiteSpace(timezone))
        {
            return TimeZoneInfo.Utc;
        }

        try
        {
            return TimeZoneInfo.FindSystemTimeZoneById(timezone);
        }
        catch (TimeZoneNotFoundException)
        {
            return null;
        }
        catch (InvalidTimeZoneException)
        {
            return null;
        }
    }
}
