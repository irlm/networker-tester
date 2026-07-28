using System.Net;
using System.Text;

namespace Networker.ControlPlane.Reports.Documents;

/// <summary>
/// Renders a report as a single self-contained HTML file: a dark brand header
/// band carrying the inline SVG wordmark, a light data-dense body, all CSS
/// inlined (no external requests). This is the graphical, shareable web export
/// and the reference layout the PDF mirrors.
/// </summary>
public sealed class HtmlReportExporter : IReportExporter
{
    public ReportFormat Format => ReportFormat.Html;
    public string ContentType => "text/html; charset=utf-8";
    public string FileExtension => "html";

    public byte[] Render(ReportDocument doc)
    {
        var sb = new StringBuilder(4096);
        sb.Append("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
        sb.Append("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
        sb.Append("<meta name=\"generator\" content=\"").Append(ReportBranding.ProductName).Append("\">\n");
        sb.Append("<title>").Append(E(doc.Title)).Append(" — ").Append(ReportBranding.ProductName).Append("</title>\n");
        sb.Append("<style>").Append(Css).Append("</style>\n</head>\n<body>\n");

        // ── Header band ──
        sb.Append("<header class=\"hdr\">\n<div class=\"brand\">").Append(ReportBranding.WordmarkSvg).Append("</div>\n");
        sb.Append("<div class=\"htxt\"><h1>").Append(E(doc.Title)).Append("</h1>");
        if (!string.IsNullOrWhiteSpace(doc.Subtitle))
        {
            sb.Append("<p class=\"sub\">").Append(E(doc.Subtitle)).Append("</p>");
        }
        sb.Append("</div>\n</header>\n");

        // ── Meta strip ──
        sb.Append("<div class=\"meta\">");
        sb.Append("<span><b>Generated</b> ").Append(E(doc.GeneratedAt.ToString("yyyy-MM-dd HH:mm 'UTC'"))).Append("</span>");
        foreach (var m in doc.Meta)
        {
            sb.Append("<span><b>").Append(E(m.Label)).Append("</b> ").Append(E(m.Value)).Append("</span>");
        }
        sb.Append("</div>\n<main>\n");

        foreach (var section in doc.Sections)
        {
            sb.Append("<section>\n<h2>").Append(E(section.Heading)).Append("</h2>\n");
            foreach (var block in section.Blocks)
            {
                RenderBlock(sb, block);
            }
            sb.Append("</section>\n");
        }
        sb.Append("</main>\n");

        if (!string.IsNullOrWhiteSpace(doc.FooterNote))
        {
            sb.Append("<footer>").Append(E(doc.FooterNote)).Append("</footer>\n");
        }

        sb.Append("</body>\n</html>\n");
        return Encoding.UTF8.GetBytes(sb.ToString());
    }

    private static void RenderBlock(StringBuilder sb, ReportBlock block)
    {
        switch (block)
        {
            case ProseBlock p:
                sb.Append("<p>").Append(E(p.Text)).Append("</p>\n");
                break;

            case CalloutBlock c:
                sb.Append("<div class=\"callout ").Append(ToneClass(c.Tone)).Append("\">")
                  .Append(E(c.Text)).Append("</div>\n");
                break;

            case MetricsBlock m:
                sb.Append("<div class=\"kpis\">");
                foreach (var k in m.Metrics)
                {
                    sb.Append("<div class=\"kpi\"><span class=\"kv ").Append(ToneClass(k.Tone)).Append("\">")
                      .Append(E(k.Value)).Append("</span><span class=\"kl\">").Append(E(k.Label)).Append("</span></div>");
                }
                sb.Append("</div>\n");
                break;

            case TableBlock t:
                RenderTable(sb, t);
                break;

            case ChartBlock c:
                RenderChart(sb, c);
                break;

            case CandleBlock cd:
                RenderCandles(sb, cd);
                break;
        }
    }

    private static void RenderCandles(StringBuilder sb, CandleBlock c)
    {
        if (c.Caption is { Length: > 0 } cap)
        {
            sb.Append("<p class=\"cap\">").Append(E(cap)).Append("</p>\n");
        }
        if (c.Points.Count == 0)
        {
            return;
        }

        const int rowH = 34, top = 8, labelW = 150, plotW = 340, valGap = 8;
        var height = top * 2 + c.Points.Count * rowH;
        double X(double v) => labelW + c.Fraction(v) * plotW;

        sb.Append("<svg class=\"chart candle\" viewBox=\"0 0 720 ").Append(height)
          .Append("\" role=\"img\" preserveAspectRatio=\"xMinYMin meet\">\n");

        for (var i = 0; i < c.Points.Count; i++)
        {
            var p = c.Points[i];
            var y = top + i * rowH;
            var midY = y + rowH / 2;
            var col = BarColor(p.Tone);

            sb.Append("<text x=\"").Append(labelW - 8).Append("\" y=\"").Append(midY + 4)
              .Append("\" class=\"cl\" text-anchor=\"end\">").Append(E(p.Label)).Append("</text>");

            var lo = p.Min ?? p.P25 ?? p.Median;
            var hi = p.High ?? p.P95 ?? p.Median;
            // whisker min→max
            if (lo is { } l && hi is { } h)
            {
                sb.Append("<line x1=\"").Append(F(X(l))).Append("\" y1=\"").Append(midY)
                  .Append("\" x2=\"").Append(F(X(h))).Append("\" y2=\"").Append(midY)
                  .Append("\" class=\"whisk\"/>");
                sb.Append(Cap(X(l), midY)).Append(Cap(X(h), midY));
            }
            // box p25→p75
            if (p.P25 is { } q1 && p.P75 is { } q3)
            {
                var x1 = X(q1);
                var w = Math.Max(2, X(q3) - x1);
                sb.Append("<rect x=\"").Append(F(x1)).Append("\" y=\"").Append(y + 8)
                  .Append("\" width=\"").Append(F(w)).Append("\" height=\"").Append(rowH - 16)
                  .Append("\" rx=\"2\" fill=\"").Append(col).Append("\" fill-opacity=\"0.28\" stroke=\"")
                  .Append(col).Append("\" stroke-width=\"1.5\"/>");
            }
            // median tick
            if (p.Median is { } med)
            {
                sb.Append("<line x1=\"").Append(F(X(med))).Append("\" y1=\"").Append(y + 6)
                  .Append("\" x2=\"").Append(F(X(med))).Append("\" y2=\"").Append(y + rowH - 6)
                  .Append("\" stroke=\"").Append(col).Append("\" stroke-width=\"2.5\"/>");
            }
            // p95 tail marker
            if (p.P95 is { } p95)
            {
                sb.Append("<line x1=\"").Append(F(X(p95))).Append("\" y1=\"").Append(y + 5)
                  .Append("\" x2=\"").Append(F(X(p95))).Append("\" y2=\"").Append(y + rowH - 5)
                  .Append("\" stroke=\"#b26a00\" stroke-width=\"1.5\" stroke-dasharray=\"2 2\"/>");
            }
            // value summary
            sb.Append("<text x=\"").Append(labelW + plotW + valGap).Append("\" y=\"").Append(midY + 4)
              .Append("\" class=\"cv\">").Append(E(CandleAscii.Summary(p, c.Unit))).Append("</text>");
        }
        sb.Append("\n</svg>\n");
    }

    private static string Cap(double x, double midY) =>
        $"<line x1=\"{F(x)}\" y1=\"{midY - 5}\" x2=\"{F(x)}\" y2=\"{midY + 5}\" class=\"whisk\"/>";

    private static string F(double v) => v.ToString("0.#", System.Globalization.CultureInfo.InvariantCulture);

    private static void RenderChart(StringBuilder sb, ChartBlock c)
    {
        if (c.Caption is { Length: > 0 } cap)
        {
            sb.Append("<p class=\"cap\">").Append(E(cap)).Append("</p>\n");
        }
        if (c.Points.Count == 0)
        {
            return;
        }

        const int rowH = 30, top = 6, labelW = 150, barMax = 320, valGap = 8;
        var height = top * 2 + c.Points.Count * rowH;
        sb.Append("<svg class=\"chart\" viewBox=\"0 0 640 ").Append(height)
          .Append("\" role=\"img\" preserveAspectRatio=\"xMinYMin meet\">\n");

        for (var i = 0; i < c.Points.Count; i++)
        {
            var p = c.Points[i];
            var y = top + i * rowH;
            var barW = (int)Math.Round(c.Fraction(p.Value) * barMax);
            var midY = y + rowH / 2;
            // label (right-aligned into the gutter)
            sb.Append("<text x=\"").Append(labelW - 8).Append("\" y=\"").Append(midY + 4)
              .Append("\" class=\"cl\" text-anchor=\"end\">").Append(E(p.Label)).Append("</text>");
            // track + bar
            sb.Append("<rect x=\"").Append(labelW).Append("\" y=\"").Append(y + 6)
              .Append("\" width=\"").Append(barMax).Append("\" height=\"").Append(rowH - 14)
              .Append("\" rx=\"3\" class=\"track\"/>");
            sb.Append("<rect x=\"").Append(labelW).Append("\" y=\"").Append(y + 6)
              .Append("\" width=\"").Append(Math.Max(2, barW)).Append("\" height=\"").Append(rowH - 14)
              .Append("\" rx=\"3\" fill=\"").Append(BarColor(p.Tone)).Append("\"/>");
            // value
            sb.Append("<text x=\"").Append(labelW + barMax + valGap).Append("\" y=\"").Append(midY + 4)
              .Append("\" class=\"cv\">").Append(E(p.Display)).Append("</text>");
        }
        sb.Append("\n</svg>\n");
    }

    private static string BarColor(ReportTone tone) => tone switch
    {
        ReportTone.Good => "#1f8a4c",
        ReportTone.Warn => "#b26a00",
        ReportTone.Bad => "#c62828",
        _ => ReportBranding.Cyan,
    };

    private static void RenderTable(StringBuilder sb, TableBlock t)
    {
        sb.Append("<table>\n<thead><tr>");
        for (var c = 0; c < t.Headers.Count; c++)
        {
            sb.Append("<th").Append(Align(t.AlignAt(c))).Append('>').Append(E(t.Headers[c])).Append("</th>");
        }
        sb.Append("</tr></thead>\n<tbody>\n");
        foreach (var row in t.Rows)
        {
            sb.Append("<tr>");
            for (var c = 0; c < row.Count; c++)
            {
                sb.Append("<td").Append(Align(t.AlignAt(c))).Append('>').Append(E(row[c])).Append("</td>");
            }
            sb.Append("</tr>\n");
        }
        sb.Append("</tbody>\n</table>\n");
    }

    private static string Align(ColumnAlign a) => a == ColumnAlign.Right ? " class=\"r\"" : "";

    private static string ToneClass(ReportTone tone) => tone switch
    {
        ReportTone.Good => "t-good",
        ReportTone.Warn => "t-warn",
        ReportTone.Bad => "t-bad",
        ReportTone.Info => "t-info",
        _ => "t-neutral",
    };

    private static string E(string s) => WebUtility.HtmlEncode(s);

    // Plain (non-interpolated) raw string — CSS is full of braces, so the
    // brand hex values are written as literals here; they mirror the
    // ReportBranding.* constants (kept in sync by HtmlReportExporterTests).
    private static readonly string Css = """
        :root{--purple:#863bff;--cyan:#47bfff;--ink:#14151b;
        --muted:#5b6270;--border:#e4e7ec;--panel:#f6f7f9;
        --headbg:#0d0e14;--headfg:#eceefb}
        *{box-sizing:border-box}html{-webkit-text-size-adjust:100%}
        body{margin:0;font:15px/1.55 ui-sans-serif,system-ui,-apple-system,"Segoe UI",Roboto,sans-serif;
        color:var(--ink);background:#ffffff}
        .mono,td.r,th.r,.kv{font-family:ui-monospace,"JetBrains Mono","Cascadia Code",SFMono-Regular,Menlo,monospace}
        .hdr{display:flex;align-items:center;gap:22px;background:var(--headbg);
        border-bottom:2px solid var(--cyan);padding:20px 32px}
        .brand svg{height:44px;width:auto;display:block}
        .htxt h1{margin:0;font-size:22px;font-weight:700;letter-spacing:-.02em;color:#fff}
        .htxt .sub{margin:2px 0 0;color:#aab1c2;font-size:13.5px}
        .meta{display:flex;flex-wrap:wrap;gap:6px 20px;padding:12px 32px;background:var(--panel);
        border-bottom:1px solid var(--border);color:var(--muted);font-size:12.5px}
        .meta b{color:var(--ink);font-weight:600}
        main{max-width:960px;margin:0 auto;padding:8px 32px 40px}
        section{margin-top:26px}
        h2{font-size:15px;text-transform:uppercase;letter-spacing:.06em;color:var(--muted);
        border-bottom:1px solid var(--border);padding-bottom:6px;margin:0 0 12px}
        p{margin:0 0 10px}
        .callout{padding:10px 14px;border-left:3px solid var(--muted);background:var(--panel);
        border-radius:0 6px 6px 0;margin:0 0 12px;font-weight:500}
        table{width:100%;border-collapse:collapse;margin:0 0 14px;font-size:13.5px}
        thead th{background:var(--headbg);color:var(--headfg);text-align:left;font-weight:600;
        padding:8px 10px;white-space:nowrap}
        tbody td{padding:7px 10px;border-bottom:1px solid var(--border)}
        tbody tr:nth-child(even){background:var(--panel)}
        td.r,th.r{text-align:right}
        .kpis{display:flex;flex-wrap:wrap;gap:12px;margin:0 0 14px}
        .kpi{flex:1 1 130px;border:1px solid var(--border);border-radius:8px;padding:12px 14px;background:#fff}
        .kpi .kv{display:block;font-size:22px;font-weight:700;letter-spacing:-.02em}
        .kpi .kl{display:block;margin-top:2px;color:var(--muted);font-size:11.5px;
        text-transform:uppercase;letter-spacing:.05em}
        .cap{margin:0 0 6px;color:var(--muted);font-size:12.5px}
        svg.chart{width:100%;max-width:640px;height:auto;margin:2px 0 16px;overflow:visible}
        svg.chart .track{fill:var(--panel)}
        svg.chart .cl{fill:var(--ink);font-size:12.5px;font-weight:500}
        svg.chart .cv{fill:var(--muted);font-size:12px;
        font-family:ui-monospace,"JetBrains Mono",SFMono-Regular,Menlo,monospace}
        svg.candle .whisk{stroke:#8a93a5;stroke-width:1.4}
        .t-good{color:#1f8a4c}.t-warn{color:#b26a00}.t-bad{color:#c62828}.t-info{color:#1266a8}
        .callout.t-good{border-left-color:#1f8a4c}.callout.t-warn{border-left-color:#b26a00}
        .callout.t-bad{border-left-color:#c62828}.callout.t-info{border-left-color:#1266a8}
        footer{max-width:960px;margin:0 auto;padding:16px 32px 40px;color:var(--muted);
        font-size:12px;border-top:1px solid var(--border)}
        @media print{.kpi{break-inside:avoid}section{break-inside:avoid}}
        """;
}
