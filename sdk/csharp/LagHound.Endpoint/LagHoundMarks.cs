using System.Text.RegularExpressions;
using Microsoft.AspNetCore.Http;

namespace LagHound.Endpoint;

/// <summary>
/// Host-app custom <c>Server-Timing</c> marks (contract v1 §4.2). Handlers call
/// <see cref="Mark"/> to attach a named duration (e.g. <c>db</c>, <c>cache</c>)
/// to the current LagHound response; it surfaces as <c>mark-db;dur=…</c>. Marks
/// are added through this typed API only — never by string-concatenating the
/// header. Names must match <c>[a-z0-9]{1,24}</c>; invalid names are ignored.
/// </summary>
public static partial class LagHoundMarks
{
    internal const string ItemsKey = "__laghound_marks";

    [GeneratedRegex("^[a-z0-9]{1,24}$")]
    private static partial Regex NamePattern();

    /// <summary>
    /// Record a custom server-side mark on the current request. No-op outside a
    /// LagHound route, for an invalid name, or a negative duration.
    /// </summary>
    public static void Mark(HttpContext context, string name, TimeSpan duration)
    {
        ArgumentNullException.ThrowIfNull(context);
        if (string.IsNullOrEmpty(name) || !NamePattern().IsMatch(name) || duration < TimeSpan.Zero)
        {
            return;
        }

        if (context.Items[ItemsKey] is not List<KeyValuePair<string, double>> marks)
        {
            marks = new List<KeyValuePair<string, double>>(4);
            context.Items[ItemsKey] = marks;
        }

        // ≤ 8 metrics per response total (contract §4.1); cap marks defensively.
        if (marks.Count < 6)
        {
            marks.Add(new KeyValuePair<string, double>(name, duration.TotalMilliseconds));
        }
    }

    internal static IReadOnlyList<KeyValuePair<string, double>>? Get(HttpContext context)
        => context.Items[ItemsKey] as List<KeyValuePair<string, double>>;
}
