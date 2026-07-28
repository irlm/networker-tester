namespace Networker.ControlPlane.Reports.Documents;

/// <summary>
/// A format-agnostic report — the shared intermediate representation every
/// exporter renders. Builders (<c>RunReportDocument</c>,
/// <c>AppNetworkReportDocument</c>, <c>PerfPerCostReportDocument</c>) map a
/// domain report into this shape; exporters (Markdown / HTML / DOCX / PDF) turn
/// it into bytes. Neither side knows about the other, so a new source report or
/// a new output format is a one-file change.
/// </summary>
/// <param name="Title">Main heading, e.g. "Perf-per-Cost Report".</param>
/// <param name="Subtitle">One-line context under the title (nullable).</param>
/// <param name="GeneratedAt">UTC generation timestamp (rendered in the header).</param>
/// <param name="Meta">Header key/value chips (project, target, run id, …).</param>
/// <param name="Sections">Ordered body sections.</param>
/// <param name="FooterNote">Small print under the last section (nullable).</param>
public sealed record ReportDocument(
    string Title,
    string? Subtitle,
    DateTime GeneratedAt,
    IReadOnlyList<ReportMeta> Meta,
    IReadOnlyList<ReportSection> Sections,
    string? FooterNote = null);

/// <summary>One header key/value chip.</summary>
public sealed record ReportMeta(string Label, string Value);

/// <summary>A titled body section holding an ordered list of blocks.</summary>
public sealed record ReportSection(string Heading, IReadOnlyList<ReportBlock> Blocks)
{
    public ReportSection(string heading, params ReportBlock[] blocks)
        : this(heading, (IReadOnlyList<ReportBlock>)blocks) { }
}

/// <summary>Semantic colour of a metric/callout/table cell. Exporters map each
/// tone to their palette; purple is deliberately absent (wordmark-only).</summary>
public enum ReportTone
{
    Neutral,
    Good,
    Warn,
    Bad,
    Info,
}

public enum ColumnAlign
{
    Left,
    Right,
}

/// <summary>Base type for the content blocks a section can hold.</summary>
public abstract record ReportBlock;

/// <summary>A paragraph of plain prose.</summary>
public sealed record ProseBlock(string Text) : ReportBlock;

/// <summary>A highlighted one-liner — a verdict or the headline finding.</summary>
public sealed record CalloutBlock(string Text, ReportTone Tone = ReportTone.Info) : ReportBlock;

/// <summary>A strip of KPI figures (label + big value + tone).</summary>
public sealed record MetricsBlock(IReadOnlyList<Metric> Metrics) : ReportBlock
{
    public MetricsBlock(params Metric[] metrics) : this((IReadOnlyList<Metric>)metrics) { }
}

/// <summary>One KPI cell.</summary>
public sealed record Metric(string Label, string Value, ReportTone Tone = ReportTone.Neutral);

/// <summary>
/// A data table. <paramref name="Aligns"/>, when supplied, must have one entry
/// per column; omit it for all-left. Cells are pre-formatted strings — the
/// builder owns rounding/units so exporters stay dumb.
/// </summary>
public sealed record TableBlock(
    IReadOnlyList<string> Headers,
    IReadOnlyList<IReadOnlyList<string>> Rows,
    IReadOnlyList<ColumnAlign>? Aligns = null) : ReportBlock
{
    /// <summary>Alignment for a column, defaulting to left when unspecified.</summary>
    public ColumnAlign AlignAt(int col) =>
        Aligns is not null && col < Aligns.Count ? Aligns[col] : ColumnAlign.Left;
}

/// <summary>
/// A horizontal bar chart. The builder supplies raw numeric values (plus a
/// pre-formatted display string per bar); each exporter draws it natively —
/// inline SVG in HTML, unicode block-bars in Markdown/DOCX — so charts need no
/// image pipeline or native dependency. Bars scale to the largest value.
/// </summary>
/// <param name="Caption">Optional line above the chart.</param>
/// <param name="Points">The bars, in display order.</param>
public sealed record ChartBlock(string? Caption, IReadOnlyList<ChartPoint> Points) : ReportBlock
{
    /// <summary>Largest value, floored at a tiny epsilon so an all-zero chart
    /// still renders empty bars rather than dividing by zero.</summary>
    public double Max => Math.Max(1e-9, Points.Count == 0 ? 0 : Points.Max(p => Math.Max(0, p.Value)));

    /// <summary>Bar fill fraction in [0,1] for a value.</summary>
    public double Fraction(double value) => Math.Clamp(value / Max, 0, 1);
}

/// <summary>One bar: a label, its numeric value (for scaling), the display
/// string (with units), and a tone for the fill colour.</summary>
public sealed record ChartPoint(string Label, double Value, string Display, ReportTone Tone = ReportTone.Info);

/// <summary>
/// A distribution ("candle" / box-plot) chart: per category a whisker from min
/// to max, a box from p25 to p75, and a median tick — the right way to show a
/// latency distribution, where the p95/max tail is what matters and a single
/// bar would hide the spread. Rendered as SVG candles in HTML and a monospace
/// box-plot in Markdown/DOCX. All bounds are optional so partial data still
/// draws what it has; every candle scales to the shared <see cref="AxisMax"/>.
/// </summary>
public sealed record CandleBlock(string? Caption, IReadOnlyList<CandlePoint> Points, string Unit = "ms") : ReportBlock
{
    /// <summary>Shared upper axis bound — the largest max/p95/median across all
    /// candles (floored above zero).</summary>
    public double AxisMax => Math.Max(1e-9, Points.Count == 0
        ? 0
        : Points.Max(p => p.High ?? p.P95 ?? p.Median ?? 0));

    /// <summary>Position in [0,1] along the axis for a value.</summary>
    public double Fraction(double value) => Math.Clamp(value / AxisMax, 0, 1);
}

/// <summary>One candle: the five-number summary of a distribution (plus p95,
/// the SLO tail marker). Any bound may be null when unavailable.</summary>
public sealed record CandlePoint(
    string Label,
    double? Min,
    double? P25,
    double? Median,
    double? P75,
    double? P95,
    double? High,
    ReportTone Tone = ReportTone.Info);
