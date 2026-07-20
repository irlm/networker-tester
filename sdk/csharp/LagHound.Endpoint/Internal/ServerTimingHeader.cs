using System.Globalization;
using System.Text;

namespace LagHound.Endpoint.Internal;

/// <summary>
/// Builds the <c>Server-Timing</c> header value (contract §4.1): ≤ 8 metrics,
/// total value ≤ 512 bytes, durations in ms with ≤ 3 decimal places.
/// </summary>
internal static class ServerTimingHeader
{
    internal const int MaxMetrics = 8;
    internal const int MaxHeaderBytes = 512;

    internal static string Build(
        IReadOnlyList<(string Name, double Ms)> metrics,
        IReadOnlyList<KeyValuePair<string, double>>? marks = null)
    {
        var sb = new StringBuilder();
        int count = 0;

        void Append(string name, double ms)
        {
            if (count >= MaxMetrics)
            {
                return;
            }

            string piece = name + ";dur=" + Math.Max(0.0, ms).ToString("0.###", CultureInfo.InvariantCulture);
            int separator = sb.Length > 0 ? 2 : 0;
            if (sb.Length + separator + piece.Length > MaxHeaderBytes)
            {
                return;
            }

            if (separator > 0)
            {
                sb.Append(", ");
            }

            sb.Append(piece);
            count++;
        }

        foreach (var (name, ms) in metrics)
        {
            Append(name, ms);
        }

        if (marks is not null)
        {
            foreach (var mark in marks)
            {
                Append("mark-" + mark.Key, mark.Value);
            }
        }

        return sb.ToString();
    }
}
