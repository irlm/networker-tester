using System.Text;

namespace Networker.ControlPlane.Reports.Documents;

/// <summary>
/// Renders a <see cref="CandlePoint"/> as a fixed-width monospace box-plot
/// "track" (whisker <c>─</c>, box <c>█</c>, median <c>┃</c>, caps <c>├ ┤</c>) so
/// the Markdown and DOCX exporters can show the same latency distribution the
/// HTML/PDF draw as SVG candles. Shared so both text exporters agree.
/// </summary>
public static class CandleAscii
{
    /// <summary>Track width in characters.</summary>
    public const int Cells = 30;

    public static string Track(CandlePoint p, double axisMax)
    {
        var buf = new char[Cells];
        Array.Fill(buf, ' ');

        int Cell(double v) => Math.Clamp((int)Math.Round(v / axisMax * (Cells - 1)), 0, Cells - 1);

        var lo = p.Min ?? p.P25 ?? p.Median;
        var hi = p.High ?? p.P95 ?? p.Median;

        if (lo is { } l && hi is { } h)
        {
            for (var i = Cell(l); i <= Cell(h); i++)
            {
                buf[i] = '─';
            }
        }
        if (p.P25 is { } q1 && p.P75 is { } q3)
        {
            for (var i = Cell(q1); i <= Cell(q3); i++)
            {
                buf[i] = '█';
            }
        }
        if (lo is { } l2)
        {
            buf[Cell(l2)] = '├';
        }
        if (hi is { } h2)
        {
            buf[Cell(h2)] = '┤';
        }
        if (p.Median is { } med)
        {
            var c = Cell(med);
            buf[c] = buf[c] == '█' ? '┃' : '╋';
        }

        return new string(buf);
    }

    /// <summary>A compact numeric summary, e.g. "p50 98 · p95 201 ms".</summary>
    public static string Summary(CandlePoint p, string unit)
    {
        var sb = new StringBuilder();
        void Add(string k, double? v)
        {
            if (v is { } d)
            {
                if (sb.Length > 0)
                {
                    sb.Append(" · ");
                }
                sb.Append(k).Append(' ').Append(Fmt(d));
            }
        }
        Add("min", p.Min);
        Add("p50", p.Median);
        Add("p95", p.P95);
        Add("max", p.High);
        if (sb.Length > 0)
        {
            sb.Append(' ').Append(unit);
        }
        return sb.ToString();
    }

    private static string Fmt(double d) =>
        d.ToString(d >= 100 ? "0" : "0.0", System.Globalization.CultureInfo.InvariantCulture);
}
