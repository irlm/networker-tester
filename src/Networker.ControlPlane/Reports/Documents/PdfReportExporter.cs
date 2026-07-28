using System.Globalization;
using System.Text;
using QuestPDF.Fluent;
using QuestPDF.Helpers;
using QuestPDF.Infrastructure;

namespace Networker.ControlPlane.Reports.Documents;

/// <summary>
/// Renders a report as a PDF via QuestPDF (code-first, MIT; Community licence
/// set once below). Mirrors the HTML layout — dark brand header, KPI strip,
/// charts, tables. Charts are drawn with native QuestPDF primitives (bars via
/// relative weights, the distribution candle via a SkiaSharp canvas), so
/// nothing relies on SVG text/font resolution; only the text-free logo glyph is
/// embedded as SVG.
///
/// <para>Linux: the <c>SkiaSharp.NativeAssets.Linux.NoDependencies</c> package
/// supplies a fontconfig-free <c>libSkiaSharp</c>, and the document pins
/// QuestPDF's embedded Lato font and never enumerates system fonts — so the prod
/// VM needs no extra apt packages.</para>
/// </summary>
public sealed class PdfReportExporter : IReportExporter
{
    static PdfReportExporter()
    {
        // Free for the Community tier; must be set before the first GeneratePdf.
        QuestPDF.Settings.License = LicenseType.Community;
    }

    public ReportFormat Format => ReportFormat.Pdf;
    public string ContentType => "application/pdf";
    public string FileExtension => "pdf";

    private const string Font = "Lato"; // QuestPDF's bundled default (no system fonts)

    public byte[] Render(ReportDocument doc)
    {
        return Document.Create(container =>
        {
            container.Page(page =>
            {
                page.Size(PageSizes.Letter);
                page.Margin(0);
                page.DefaultTextStyle(t => t.FontFamily(Font).FontSize(10).FontColor(Hex(ReportBranding.Ink)));

                page.Content().Column(col =>
                {
                    Header(col, doc);
                    col.Item().PaddingHorizontal(28).PaddingBottom(28).Column(body =>
                    {
                        Meta(body, doc);
                        foreach (var section in doc.Sections)
                        {
                            Section(body, section);
                        }
                        if (!string.IsNullOrWhiteSpace(doc.FooterNote))
                        {
                            body.Item().PaddingTop(16).BorderTop(1).BorderColor(Hex(ReportBranding.Border))
                                .PaddingTop(8).Text(doc.FooterNote!).FontSize(8.5f).Italic()
                                .FontColor(Hex(ReportBranding.Muted));
                        }
                    });
                });

                page.Footer().PaddingHorizontal(28).PaddingBottom(12).Row(r =>
                {
                    r.RelativeItem().Text(ReportBranding.ProductName).FontSize(8).FontColor(Hex(ReportBranding.Muted));
                    r.RelativeItem().AlignRight().Text(t =>
                    {
                        t.CurrentPageNumber().FontSize(8).FontColor(Hex(ReportBranding.Muted));
                        t.Span(" / ").FontSize(8).FontColor(Hex(ReportBranding.Muted));
                        t.TotalPages().FontSize(8).FontColor(Hex(ReportBranding.Muted));
                    });
                });
            });
        }).GeneratePdf();
    }

    private static void Header(ColumnDescriptor col, ReportDocument doc)
    {
        col.Item().Background(Hex(ReportBranding.HeaderBg)).PaddingHorizontal(28).PaddingVertical(16)
            .Column(h =>
            {
                h.Item().Row(r =>
                {
                    r.ConstantItem(30).Height(30).Svg(ReportBranding.GlyphSvg);
                    r.ConstantItem(9);
                    r.AutoItem().AlignMiddle().Text(t =>
                    {
                        t.Span(ReportBranding.ProductName).FontSize(17).Bold().FontColor(Hex(ReportBranding.Purple));
                        t.Span("_").FontSize(17).Bold().FontColor(Hex(ReportBranding.Cyan));
                    });
                });
                h.Item().PaddingTop(10).Text(doc.Title).FontSize(19).Bold().FontColor(Hex("#ffffff"));
                if (!string.IsNullOrWhiteSpace(doc.Subtitle))
                {
                    h.Item().PaddingTop(2).Text(doc.Subtitle!).FontSize(10.5f).FontColor(Hex("#aab1c2"));
                }
            });
        col.Item().Height(2).Background(Hex(ReportBranding.Cyan));
    }

    private static void Meta(ColumnDescriptor body, ReportDocument doc)
    {
        var parts = new List<string> { "Generated " + doc.GeneratedAt.ToString("yyyy-MM-dd HH:mm 'UTC'") };
        parts.AddRange(doc.Meta.Select(m => $"{m.Label}: {m.Value}"));
        body.Item().PaddingTop(10).Text(string.Join("    ·    ", parts))
            .FontSize(9).FontColor(Hex(ReportBranding.Muted));
    }

    private static void Section(ColumnDescriptor body, ReportSection section)
    {
        body.Item().PaddingTop(16).BorderBottom(1).BorderColor(Hex(ReportBranding.Border)).PaddingBottom(4)
            .Text(section.Heading.ToUpperInvariant()).FontSize(9).Bold()
            .FontColor(Hex(ReportBranding.Muted)).LetterSpacing(0.03f);

        foreach (var block in section.Blocks)
        {
            Block(body, block);
        }
    }

    private static void Block(ColumnDescriptor body, ReportBlock block)
    {
        switch (block)
        {
            case ProseBlock p:
                body.Item().PaddingTop(6).Text(p.Text).FontSize(10).LineHeight(1.4f);
                break;

            case CalloutBlock c:
                body.Item().PaddingTop(8).Background(Hex(ReportBranding.PanelBg))
                    .BorderLeft(3).BorderColor(Hex(ToneHex(c.Tone))).Padding(9)
                    .Text(c.Text).FontSize(10.5f).SemiBold().FontColor(Hex(ToneHex(c.Tone)));
                break;

            case MetricsBlock m:
                body.Item().PaddingTop(8).Row(row =>
                {
                    row.Spacing(10);
                    foreach (var k in m.Metrics)
                    {
                        row.RelativeItem().Border(1).BorderColor(Hex(ReportBranding.Border)).Padding(9).Column(cell =>
                        {
                            cell.Item().Text(k.Value).FontSize(16).Bold().FontColor(Hex(ToneHex(k.Tone)));
                            cell.Item().PaddingTop(1).Text(k.Label.ToUpperInvariant()).FontSize(7.5f)
                                .FontColor(Hex(ReportBranding.Muted)).LetterSpacing(0.04f);
                        });
                    }
                });
                break;

            case TableBlock t:
                Table(body, t);
                break;

            case ChartBlock ch:
                Bars(body, ch);
                break;

            case CandleBlock cd:
                Candles(body, cd);
                break;
        }
    }

    private static void Table(ColumnDescriptor body, TableBlock t)
    {
        body.Item().PaddingTop(8).Table(table =>
        {
            table.ColumnsDefinition(cols =>
            {
                for (var c = 0; c < t.Headers.Count; c++)
                {
                    cols.RelativeColumn(c == 0 ? 2 : 1);
                }
            });

            for (var c = 0; c < t.Headers.Count; c++)
            {
                var align = t.AlignAt(c);
                table.Cell().Background(Hex(ReportBranding.HeaderBg)).PaddingVertical(5).PaddingHorizontal(7)
                    .Element(e => Aligned(e, align))
                    .Text(t.Headers[c]).FontSize(8.5f).Bold().FontColor(Hex(ReportBranding.TableHeadFg));
            }

            var row = 0;
            foreach (var r in t.Rows)
            {
                var bg = row++ % 2 == 1 ? Hex(ReportBranding.PanelBg) : Hex("#ffffff");
                for (var c = 0; c < r.Count; c++)
                {
                    var align = t.AlignAt(c);
                    table.Cell().Background(bg).BorderBottom(1).BorderColor(Hex(ReportBranding.Border))
                        .PaddingVertical(4).PaddingHorizontal(7)
                        .Element(e => Aligned(e, align))
                        .Text(r[c]).FontSize(8.5f);
                }
            }
        });
    }

    private static void Bars(ColumnDescriptor body, ChartBlock c)
    {
        if (c.Caption is { Length: > 0 } cap)
        {
            body.Item().PaddingTop(8).Text(cap).FontSize(8.5f).FontColor(Hex(ReportBranding.Muted));
        }
        body.Item().PaddingTop(4).Column(chart =>
        {
            foreach (var p in c.Points)
            {
                chart.Item().PaddingVertical(2).Row(row =>
                {
                    row.ConstantItem(90).AlignRight().PaddingRight(8).AlignMiddle()
                        .Text(p.Label).FontSize(8.5f);
                    row.RelativeItem().Height(13).Row(bar =>
                    {
                        var f = (float)Math.Max(0.012, c.Fraction(p.Value));
                        bar.RelativeItem(f).Background(Hex(BarHex(p.Tone)));
                        if (f < 1f)
                        {
                            bar.RelativeItem(1f - f).Background(Hex(ReportBranding.PanelBg));
                        }
                    });
                    row.ConstantItem(78).PaddingLeft(8).AlignMiddle()
                        .Text(p.Display).FontSize(8).FontColor(Hex(ReportBranding.Muted));
                });
            }
        });
    }

    private static void Candles(ColumnDescriptor body, CandleBlock c)
    {
        if (c.Caption is { Length: > 0 } cap)
        {
            body.Item().PaddingTop(8).Text(cap).FontSize(8.5f).FontColor(Hex(ReportBranding.Muted));
        }
        body.Item().PaddingTop(4).Column(chart =>
        {
            foreach (var p in c.Points)
            {
                chart.Item().PaddingVertical(2).Row(row =>
                {
                    row.ConstantItem(90).AlignRight().PaddingRight(8).AlignMiddle()
                        .Text(p.Label).FontSize(8.5f);
                    row.RelativeItem().Height(16).Svg(CandleSvg(p, c.AxisMax));
                    row.ConstantItem(210).PaddingLeft(8).AlignMiddle()
                        .Text(CandleAscii.Summary(p, c.Unit)).FontSize(7.5f).FontColor(Hex(ReportBranding.Muted));
                });
            }
        });
    }

    /// <summary>A shapes-only (no text) SVG box-plot for one candle, fed to
    /// QuestPDF's <c>.Svg()</c> — the same font-free rendering path the logo
    /// glyph uses, so it needs no font resolution on the server.</summary>
    private static string CandleSvg(CandlePoint p, double axisMax)
    {
        const double w = 200, h = 16;
        double X(double v) => Math.Clamp(v / axisMax, 0, 1) * (w - 2) + 1;
        const double mid = h / 2;
        var tone = BarHex(p.Tone);

        var sb = new StringBuilder();
        sb.Append("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 200 16\" preserveAspectRatio=\"none\">");

        var lo = p.Min ?? p.P25 ?? p.Median;
        var hi = p.High ?? p.P95 ?? p.Median;
        if (lo is { } l && hi is { } hg)
        {
            sb.Append(Line(X(l), mid, X(hg), mid, "#8a93a5", 1.2));
            sb.Append(Line(X(l), mid - 4, X(l), mid + 4, "#8a93a5", 1.2));
            sb.Append(Line(X(hg), mid - 4, X(hg), mid + 4, "#8a93a5", 1.2));
        }
        if (p.P25 is { } q1 && p.P75 is { } q3)
        {
            var x = X(q1);
            var bw = Math.Max(1.5, X(q3) - x);
            sb.Append($"<rect x=\"{F(x)}\" y=\"3\" width=\"{F(bw)}\" height=\"10\" rx=\"1\" fill=\"{tone}\" fill-opacity=\"0.28\" stroke=\"{tone}\" stroke-width=\"1.3\"/>");
        }
        if (p.Median is { } med)
        {
            sb.Append(Line(X(med), 2, X(med), h - 2, tone, 2.2));
        }
        if (p.P95 is { } p95)
        {
            sb.Append($"<line x1=\"{F(X(p95))}\" y1=\"2\" x2=\"{F(X(p95))}\" y2=\"14\" stroke=\"#b26a00\" stroke-width=\"1.3\" stroke-dasharray=\"2 2\"/>");
        }

        sb.Append("</svg>");
        return sb.ToString();
    }

    private static string Line(double x1, double y1, double x2, double y2, string color, double width) =>
        $"<line x1=\"{F(x1)}\" y1=\"{F(y1)}\" x2=\"{F(x2)}\" y2=\"{F(y2)}\" stroke=\"{color}\" stroke-width=\"{F(width)}\" stroke-linecap=\"round\"/>";

    private static string F(double v) => v.ToString("0.##", CultureInfo.InvariantCulture);

    private static IContainer Aligned(IContainer e, ColumnAlign align) =>
        align == ColumnAlign.Right ? e.AlignRight() : e.AlignLeft();

    private static Color Hex(string hex) => Color.FromHex(hex);

    private static string ToneHex(ReportTone tone) => tone switch
    {
        ReportTone.Good => "#1f8a4c",
        ReportTone.Warn => "#b26a00",
        ReportTone.Bad => "#c62828",
        ReportTone.Info => "#1266a8",
        _ => ReportBranding.Ink,
    };

    private static string BarHex(ReportTone tone) => tone switch
    {
        ReportTone.Good => "#1f8a4c",
        ReportTone.Warn => "#b26a00",
        ReportTone.Bad => "#c62828",
        _ => ReportBranding.Cyan,
    };
}
