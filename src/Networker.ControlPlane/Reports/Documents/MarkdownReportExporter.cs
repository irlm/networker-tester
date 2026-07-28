using System.Text;

namespace Networker.ControlPlane.Reports.Documents;

/// <summary>
/// Renders a report as GitHub-flavoured Markdown — diffable plain text that
/// pastes into tickets, wikis and PRs. The logo is a text wordmark (Markdown
/// image embedding is unreliable across renderers/sanitizers); the graphical
/// mark is carried by the HTML/PDF exports.
/// </summary>
public sealed class MarkdownReportExporter : IReportExporter
{
    public ReportFormat Format => ReportFormat.Markdown;
    public string ContentType => "text/markdown; charset=utf-8";
    public string FileExtension => "md";

    public byte[] Render(ReportDocument doc)
    {
        var sb = new StringBuilder();

        // Wordmark banner + title.
        sb.Append("**").Append(ReportBranding.ProductName).Append("**  ·  _")
          .Append(ReportBranding.Tagline).Append("_\n\n");
        sb.Append("# ").Append(doc.Title).Append('\n');
        if (!string.IsNullOrWhiteSpace(doc.Subtitle))
        {
            sb.Append('\n').Append(Escape(doc.Subtitle)).Append('\n');
        }

        // Meta line.
        sb.Append("\n*Generated ")
          .Append(doc.GeneratedAt.ToString("yyyy-MM-dd HH:mm 'UTC'"));
        foreach (var m in doc.Meta)
        {
            sb.Append("  ·  **").Append(Escape(m.Label)).Append(":** ").Append(Escape(m.Value));
        }
        sb.Append("*\n");

        foreach (var section in doc.Sections)
        {
            sb.Append("\n## ").Append(section.Heading).Append('\n');
            foreach (var block in section.Blocks)
            {
                sb.Append('\n');
                RenderBlock(sb, block);
            }
        }

        if (!string.IsNullOrWhiteSpace(doc.FooterNote))
        {
            sb.Append("\n---\n\n<sub>").Append(Escape(doc.FooterNote)).Append("</sub>\n");
        }

        return Encoding.UTF8.GetBytes(sb.ToString());
    }

    private static void RenderBlock(StringBuilder sb, ReportBlock block)
    {
        switch (block)
        {
            case ProseBlock p:
                sb.Append(Escape(p.Text)).Append('\n');
                break;

            case CalloutBlock c:
                sb.Append("> ").Append(ToneLabel(c.Tone)).Append(' ')
                  .Append(Escape(c.Text)).Append('\n');
                break;

            case MetricsBlock m:
                // Render as a compact two-row table: labels then values.
                sb.Append("| ").Append(string.Join(" | ", m.Metrics.Select(x => Escape(x.Label)))).Append(" |\n");
                sb.Append("|").Append(string.Concat(Enumerable.Repeat(" --- |", m.Metrics.Count))).Append('\n');
                sb.Append("| ").Append(string.Join(" | ", m.Metrics.Select(x => "**" + Escape(x.Value) + "**"))).Append(" |\n");
                break;

            case TableBlock t:
                RenderTable(sb, t);
                break;

            case ChartBlock ch:
                RenderChart(sb, ch);
                break;

            case CandleBlock cd:
                RenderCandles(sb, cd);
                break;
        }
    }

    /// <summary>Distribution chart as a fenced monospace box-plot (whisker/box/
    /// median) — the text twin of the HTML SVG candles.</summary>
    private static void RenderCandles(StringBuilder sb, CandleBlock c)
    {
        if (c.Caption is { Length: > 0 } cap)
        {
            sb.Append('_').Append(Escape(cap)).Append("_\n\n");
        }
        if (c.Points.Count == 0)
        {
            return;
        }

        var labelW = c.Points.Max(p => p.Label.Length);
        sb.Append("```\n");
        foreach (var p in c.Points)
        {
            sb.Append(p.Label.PadRight(labelW)).Append("  ")
              .Append(CandleAscii.Track(p, c.AxisMax)).Append("  ")
              .Append(CandleAscii.Summary(p, c.Unit)).Append('\n');
        }
        sb.Append("```\n");
    }

    /// <summary>Chart as a fenced code block of unicode block-bars — renders as
    /// aligned monospace everywhere, no image support required.</summary>
    private static void RenderChart(StringBuilder sb, ChartBlock c)
    {
        if (c.Caption is { Length: > 0 } cap)
        {
            sb.Append('_').Append(Escape(cap)).Append("_\n\n");
        }
        if (c.Points.Count == 0)
        {
            return;
        }

        const int cells = 24;
        var labelW = c.Points.Max(p => p.Label.Length);
        sb.Append("```\n");
        foreach (var p in c.Points)
        {
            var filled = (int)Math.Round(c.Fraction(p.Value) * cells);
            var bar = new string('█', filled) + new string('░', cells - filled);
            sb.Append(p.Label.PadRight(labelW)).Append("  ").Append(bar)
              .Append("  ").Append(p.Display).Append('\n');
        }
        sb.Append("```\n");
    }

    private static void RenderTable(StringBuilder sb, TableBlock t)
    {
        sb.Append("| ").Append(string.Join(" | ", t.Headers.Select(Escape))).Append(" |\n");
        sb.Append('|');
        for (var c = 0; c < t.Headers.Count; c++)
        {
            sb.Append(t.AlignAt(c) == ColumnAlign.Right ? " ---: |" : " --- |");
        }
        sb.Append('\n');
        foreach (var row in t.Rows)
        {
            sb.Append("| ").Append(string.Join(" | ", row.Select(Escape))).Append(" |\n");
        }
    }

    private static string ToneLabel(ReportTone tone) => tone switch
    {
        ReportTone.Good => "✅",
        ReportTone.Warn => "⚠️",
        ReportTone.Bad => "🔴",
        ReportTone.Info => "ℹ️",
        _ => "",
    };

    /// <summary>Escape the Markdown table/inline metacharacters that would break
    /// a cell or line: pipes and newlines.</summary>
    private static string Escape(string s) =>
        s.Replace("\\", "\\\\").Replace("|", "\\|").Replace("\r", "").Replace("\n", " ");
}
