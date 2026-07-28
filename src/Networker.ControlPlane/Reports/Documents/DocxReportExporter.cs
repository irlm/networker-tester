using DocumentFormat.OpenXml;
using DocumentFormat.OpenXml.Packaging;
using DocumentFormat.OpenXml.Wordprocessing;

namespace Networker.ControlPlane.Reports.Documents;

/// <summary>
/// Renders a report as an editable Word <c>.docx</c> via the pure-managed
/// DocumentFormat.OpenXml SDK (no native dependency). The brand mark is a
/// colour-run text wordmark ("LagHound" in brand purple + a cyan cursor) —
/// Word's SVG image support needs a raster fallback, which would drag in the
/// SkiaSharp native stack; the graphical glyph is carried by the HTML/PDF
/// exports instead.
/// </summary>
public sealed class DocxReportExporter : IReportExporter
{
    public ReportFormat Format => ReportFormat.Docx;
    public string ContentType =>
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
    public string FileExtension => "docx";

    // OpenXml font sizes are half-points; hex colours carry no leading '#'.
    private const string Purple = "863bff";
    private const string Cyan = "47bfff";
    private const string HeaderBg = "0d0e14";
    private const string Ink = "14151b";
    private const string Muted = "5b6270";
    private const string Panel = "f6f7f9";

    public byte[] Render(ReportDocument doc)
    {
        using var ms = new MemoryStream();
        using (var word = WordprocessingDocument.Create(
                   ms, WordprocessingDocumentType.Document, autoSave: true))
        {
            var main = word.AddMainDocumentPart();
            main.Document = new Document();
            var body = main.Document.AppendChild(new Body());

            // ── Header: wordmark + tagline ──
            body.AppendChild(new Paragraph(
                new Run(RunProps(bold: true, sizeHalfPt: 40, color: Purple), Text(ReportBranding.ProductName)),
                new Run(RunProps(bold: true, sizeHalfPt: 40, color: Cyan), Text("_"))));
            body.AppendChild(Para(ReportBranding.Tagline, sizeHalfPt: 17, color: Muted, italic: true));

            // ── Title / subtitle ──
            body.AppendChild(Para(doc.Title, sizeHalfPt: 36, bold: true, color: Ink, spaceBeforePt: 10));
            if (!string.IsNullOrWhiteSpace(doc.Subtitle))
            {
                body.AppendChild(Para(doc.Subtitle!, sizeHalfPt: 20, color: Muted));
            }

            // ── Meta line ──
            var meta = "Generated " + doc.GeneratedAt.ToString("yyyy-MM-dd HH:mm 'UTC'");
            foreach (var m in doc.Meta)
            {
                meta += $"    ·    {m.Label}: {m.Value}";
            }
            body.AppendChild(Para(meta, sizeHalfPt: 16, color: Muted, spaceAfterPt: 6));

            foreach (var section in doc.Sections)
            {
                body.AppendChild(Para(section.Heading.ToUpperInvariant(),
                    sizeHalfPt: 20, bold: true, color: Muted, spaceBeforePt: 12, spaceAfterPt: 4));
                foreach (var block in section.Blocks)
                {
                    AppendBlock(body, block);
                }
            }

            if (!string.IsNullOrWhiteSpace(doc.FooterNote))
            {
                body.AppendChild(Para(doc.FooterNote!, sizeHalfPt: 15, color: Muted,
                    italic: true, spaceBeforePt: 14));
            }

            body.AppendChild(new SectionProperties(
                new PageSize { Width = 12240, Height = 15840 },          // US Letter, twips
                new PageMargin { Top = 1080, Bottom = 1080, Left = 1080, Right = 1080, Header = 720, Footer = 720 }));
        }

        return ms.ToArray();
    }

    private void AppendBlock(Body body, ReportBlock block)
    {
        switch (block)
        {
            case ProseBlock p:
                body.AppendChild(Para(p.Text, sizeHalfPt: 21, color: Ink, spaceAfterPt: 4));
                break;

            case CalloutBlock c:
                var para = Para(c.Text, sizeHalfPt: 21, bold: true, color: ToneHex(c.Tone), spaceAfterPt: 6);
                para.ParagraphProperties!.Append(new Shading
                {
                    Val = ShadingPatternValues.Clear,
                    Color = "auto",
                    Fill = Panel,
                });
                body.AppendChild(para);
                break;

            case MetricsBlock m:
                body.AppendChild(BuildMetricsTable(m));
                break;

            case TableBlock t:
                body.AppendChild(BuildTable(t));
                break;

            case ChartBlock c:
                if (c.Caption is { Length: > 0 } cap)
                {
                    body.AppendChild(Para(cap, sizeHalfPt: 16, color: Muted, italic: true));
                }
                if (c.Points.Count > 0)
                {
                    body.AppendChild(BuildChart(c));
                }
                break;

            case CandleBlock cd:
                if (cd.Caption is { Length: > 0 } ccap)
                {
                    body.AppendChild(Para(ccap, sizeHalfPt: 16, color: Muted, italic: true));
                }
                if (cd.Points.Count > 0)
                {
                    body.AppendChild(BuildCandles(cd));
                }
                break;
        }
    }

    /// <summary>Box-plot as a borderless table: label · monospace track ·
    /// numeric summary — the DOCX twin of the HTML SVG candles.</summary>
    private Table BuildCandles(CandleBlock c)
    {
        var table = new Table(BorderlessTableProps());
        foreach (var p in c.Points)
        {
            var track = new Paragraph(
                new ParagraphProperties(new SpacingBetweenLines { Before = "0", After = "0" }),
                new Run(
                    new RunProperties(
                        new RunFonts { Ascii = "Consolas", HighAnsi = "Consolas" },
                        new Color { Val = BarHex(p.Tone) },
                        new FontSize { Val = "18" }),
                    Text(CandleAscii.Track(p, c.AxisMax))));

            table.Append(new TableRow(
                new TableCell(Para(p.Label, sizeHalfPt: 18, color: Ink)),
                new TableCell(track),
                new TableCell(Para(CandleAscii.Summary(p, c.Unit), sizeHalfPt: 18, color: Muted))));
        }
        return table;
    }

    /// <summary>Bar chart as a borderless 3-column table: label · monospace
    /// block-bar (brand-tinted) · value.</summary>
    private Table BuildChart(ChartBlock c)
    {
        const int cells = 24;
        var table = new Table(BorderlessTableProps());
        foreach (var p in c.Points)
        {
            var filled = (int)Math.Round(c.Fraction(p.Value) * cells);
            var bar = new string('█', filled) + new string('░', cells - filled);
            var barPara = new Paragraph(new ParagraphProperties(new SpacingBetweenLines { Before = "0", After = "0" }),
                new Run(
                    new RunProperties(
                        new RunFonts { Ascii = "Consolas", HighAnsi = "Consolas" },
                        new Color { Val = BarHex(p.Tone) },
                        new FontSize { Val = "18" }),
                    Text(bar)));

            table.Append(new TableRow(
                new TableCell(Para(p.Label, sizeHalfPt: 18, color: Ink)),
                new TableCell(barPara),
                new TableCell(Para(p.Display, sizeHalfPt: 18, color: Muted))));
        }
        return table;
    }

    private static string BarHex(ReportTone tone) => tone switch
    {
        ReportTone.Good => "1f8a4c",
        ReportTone.Warn => "b26a00",
        ReportTone.Bad => "c62828",
        _ => Cyan,
    };

    private Table BuildMetricsTable(MetricsBlock m)
    {
        var row = new TableRow();
        foreach (var k in m.Metrics)
        {
            row.Append(new TableCell(
                new TableCellProperties(new TableCellWidth { Type = TableWidthUnitValues.Auto }),
                Para(k.Value, sizeHalfPt: 30, bold: true, color: ToneHex(k.Tone)),
                Para(k.Label.ToUpperInvariant(), sizeHalfPt: 14, color: Muted)));
        }
        return new Table(BorderlessTableProps(), row);
    }

    private Table BuildTable(TableBlock t)
    {
        var table = new Table(GridTableProps());

        var head = new TableRow();
        for (var c = 0; c < t.Headers.Count; c++)
        {
            head.Append(new TableCell(
                new TableCellProperties(new Shading
                {
                    Val = ShadingPatternValues.Clear,
                    Color = "auto",
                    Fill = HeaderBg,
                }),
                Para(t.Headers[c], sizeHalfPt: 18, bold: true, color: "eceefb", align: t.AlignAt(c))));
        }
        table.Append(head);

        foreach (var r in t.Rows)
        {
            var tr = new TableRow();
            for (var c = 0; c < r.Count; c++)
            {
                tr.Append(new TableCell(Para(r[c], sizeHalfPt: 18, color: Ink, align: t.AlignAt(c))));
            }
            table.Append(tr);
        }
        return table;
    }

    // ── OpenXml helpers ────────────────────────────────────────────────────────

    private static TableProperties GridTableProps() => new(
        new TableWidth { Width = "5000", Type = TableWidthUnitValues.Pct },
        new TableBorders(
            Border<TopBorder>(), Border<BottomBorder>(), Border<LeftBorder>(),
            Border<RightBorder>(), Border<InsideHorizontalBorder>(), Border<InsideVerticalBorder>()));

    private static TableProperties BorderlessTableProps() => new(
        new TableWidth { Width = "5000", Type = TableWidthUnitValues.Pct });

    private static T Border<T>() where T : BorderType, new() =>
        new() { Val = BorderValues.Single, Size = 4, Color = "e4e7ec" };

    private static Paragraph Para(
        string text, int sizeHalfPt, string color, bool bold = false, bool italic = false,
        int spaceBeforePt = 0, int spaceAfterPt = 0, ColumnAlign align = ColumnAlign.Left)
    {
        var props = new ParagraphProperties(new SpacingBetweenLines
        {
            Before = (spaceBeforePt * 20).ToString(),
            After = (spaceAfterPt * 20).ToString(),
        });
        if (align == ColumnAlign.Right)
        {
            props.Append(new Justification { Val = JustificationValues.Right });
        }

        return new Paragraph(props, new Run(RunProps(bold, sizeHalfPt, color, italic), Text(text)));
    }

    private static RunProperties RunProps(bool bold, int sizeHalfPt, string color, bool italic = false)
    {
        var rp = new RunProperties(new RunFonts { Ascii = "Segoe UI", HighAnsi = "Segoe UI" });
        if (bold)
        {
            rp.Append(new Bold());
        }
        if (italic)
        {
            rp.Append(new Italic());
        }
        rp.Append(new Color { Val = color });
        rp.Append(new FontSize { Val = sizeHalfPt.ToString() });
        return rp;
    }

    private static Text Text(string s) => new(s) { Space = SpaceProcessingModeValues.Preserve };

    private static string ToneHex(ReportTone tone) => tone switch
    {
        ReportTone.Good => "1f8a4c",
        ReportTone.Warn => "b26a00",
        ReportTone.Bad => "c62828",
        ReportTone.Info => "1266a8",
        _ => Ink,
    };
}
