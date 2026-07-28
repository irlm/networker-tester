using System.Reflection;

namespace Networker.ControlPlane.Reports.Documents;

/// <summary>
/// The brand surface for exported report documents: the product wordmark, the
/// print-theme colour palette, and the embedded SVG logo assets.
///
/// <para>Reports are LIGHT, printable documents (white body, dark ink) with a
/// dark brand header band — the Grafana/Datadog look called for in
/// <c>.impeccable.md</c>. Per <c>dashboard/src/index.css</c> the purple
/// (<see cref="Purple"/>) is the wordmark/logo colour ONLY and is never
/// re-purposed for a status or KPI (audit F12); status tone comes from
/// <see cref="ReportTones"/>.</para>
///
/// <para>The SVG assets live at <c>assets/brand/*.svg</c> (the editable source
/// of truth) and are embedded into this assembly at build time (see the csproj)
/// so a deploy can never lose them — the same pattern as the embedded
/// <c>shared/cloud-costs.json</c>.</para>
/// </summary>
public static class ReportBranding
{
    /// <summary>User-facing product name (brand — matches the frontend
    /// <c>PRODUCT_NAME</c>). Never derive infra identifiers from this.</summary>
    public const string ProductName = "LagHound";

    public const string Tagline = "Network & application latency diagnostics";

    // ── Print palette ────────────────────────────────────────────────────────
    /// <summary>Brand purple — wordmark/logo ONLY (never a status colour).</summary>
    public const string Purple = "#863bff";

    /// <summary>Terminal-accent cyan — the logo's pulse, header hairline.</summary>
    public const string Cyan = "#47bfff";

    /// <summary>Header band / logo-tile background (brand surface).</summary>
    public const string HeaderBg = "#0d0e14";

    /// <summary>Body ink (near-black, on white).</summary>
    public const string Ink = "#14151b";

    /// <summary>Muted secondary text.</summary>
    public const string Muted = "#5b6270";

    public const string PageBg = "#ffffff";
    public const string PanelBg = "#f6f7f9";
    public const string Border = "#e4e7ec";

    /// <summary>Dark table-header row (fg on <see cref="HeaderBg"/>).</summary>
    public const string TableHeadFg = "#eceefb";

    private static readonly Lazy<string> GlyphSvgLazy =
        new(() => LoadResource("Networker.ControlPlane.brand.laghound-glyph.svg"));

    private static readonly Lazy<string> WordmarkSvgLazy =
        new(() => LoadResource("Networker.ControlPlane.brand.laghound-wordmark.svg"));

    /// <summary>The square logo glyph (paw + latency pulse), as raw SVG markup.</summary>
    public static string GlyphSvg => GlyphSvgLazy.Value;

    /// <summary>The horizontal wordmark lockup (glyph + "LagHound"), as raw SVG.</summary>
    public static string WordmarkSvg => WordmarkSvgLazy.Value;

    private static string LoadResource(string logicalName)
    {
        var asm = Assembly.GetExecutingAssembly();
        using var stream = asm.GetManifestResourceStream(logicalName)
            ?? throw new InvalidOperationException(
                $"Embedded brand asset '{logicalName}' not found — check the "
                + "<EmbeddedResource> entries in Networker.ControlPlane.csproj.");
        using var reader = new StreamReader(stream);
        return reader.ReadToEnd();
    }
}
