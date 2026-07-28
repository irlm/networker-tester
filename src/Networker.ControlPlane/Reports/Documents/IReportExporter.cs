using System.Diagnostics.CodeAnalysis;

namespace Networker.ControlPlane.Reports.Documents;

/// <summary>The document formats a <see cref="ReportDocument"/> can be exported to.</summary>
public enum ReportFormat
{
    /// <summary>The JSON wire report (the pre-existing default; not a document).</summary>
    Json,
    Markdown,
    Html,
    Docx,
    Pdf,
}

/// <summary>
/// Renders a <see cref="ReportDocument"/> into a downloadable document. One
/// implementation per document format; each is stateless and registered as a
/// singleton. <see cref="ReportExporterResolver"/> picks by
/// <see cref="Format"/>.
/// </summary>
public interface IReportExporter
{
    ReportFormat Format { get; }

    /// <summary>MIME type for the HTTP <c>Content-Type</c>.</summary>
    string ContentType { get; }

    /// <summary>File extension WITHOUT the dot (e.g. <c>md</c>, <c>docx</c>).</summary>
    string FileExtension { get; }

    /// <summary>Render the document to its serialized bytes.</summary>
    byte[] Render(ReportDocument document);
}

/// <summary>Parsing and DI resolution for report formats.</summary>
public static class ReportFormats
{
    /// <summary>
    /// Parse a <c>?format=</c> value (case-insensitive; accepts common aliases).
    /// Returns <c>false</c> for unknown values so the endpoint can 400 with the
    /// supported list rather than guessing.
    /// </summary>
    public static bool TryParse(string? value, out ReportFormat format)
    {
        format = ReportFormat.Json;
        if (string.IsNullOrWhiteSpace(value))
        {
            return true; // absent → JSON (unchanged default)
        }

        switch (value.Trim().ToLowerInvariant())
        {
            case "json": format = ReportFormat.Json; return true;
            case "md":
            case "markdown": format = ReportFormat.Markdown; return true;
            case "html":
            case "htm": format = ReportFormat.Html; return true;
            case "docx":
            case "word": format = ReportFormat.Docx; return true;
            case "pdf": format = ReportFormat.Pdf; return true;
            default: return false;
        }
    }
}

/// <summary>
/// Resolves the registered <see cref="IReportExporter"/> for a format. Formats
/// with no registered exporter (e.g. <see cref="ReportFormat.Pdf"/> before the
/// PDF package lands) resolve to <c>null</c> so the caller can 400/415 with a
/// clear "not available" message instead of throwing.
/// </summary>
public sealed class ReportExporterResolver
{
    private readonly Dictionary<ReportFormat, IReportExporter> _byFormat;

    public ReportExporterResolver(IEnumerable<IReportExporter> exporters)
    {
        _byFormat = exporters.ToDictionary(e => e.Format);
    }

    public bool TryResolve(ReportFormat format, [NotNullWhen(true)] out IReportExporter? exporter) =>
        _byFormat.TryGetValue(format, out exporter);

    /// <summary>Document formats that actually have a registered exporter,
    /// lower-cased for a user-facing "supported: …" message.</summary>
    public IEnumerable<string> AvailableDocumentFormats =>
        _byFormat.Keys
            .Where(f => f != ReportFormat.Json)
            .Select(f => f.ToString().ToLowerInvariant())
            .OrderBy(s => s, StringComparer.Ordinal);
}
