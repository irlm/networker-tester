namespace Networker.ControlPlane.Reports.Documents;

/// <summary>
/// Shared plumbing for the report-export endpoints: turn a built
/// <see cref="ReportDocument"/> into a file download, and a consistent 400 for
/// an unknown/unavailable format. Keeps the endpoint handlers to a couple of
/// lines each.
/// </summary>
public static class ReportExport
{
    /// <summary>400 for a format the caller asked for that we don't produce —
    /// names the document formats that ARE available and reminds that omitting
    /// <c>?format=</c> yields JSON.</summary>
    public static IResult BadFormat(string? requested, ReportExporterResolver resolver) =>
        Results.BadRequest(new
        {
            error = $"unsupported report format '{requested}'",
            supported_document_formats = resolver.AvailableDocumentFormats,
            note = "omit ?format= (or use ?format=json) for the JSON report",
        });

    /// <summary>Render <paramref name="document"/> in <paramref name="format"/>
    /// and return it as an attachment named
    /// <c>{fileBase}.{ext}</c>; 400 if that format has no exporter.</summary>
    public static IResult Deliver(
        ReportExporterResolver resolver, ReportFormat format, ReportDocument document,
        string fileBase, string? requested = null)
    {
        if (!resolver.TryResolve(format, out var exporter))
        {
            return BadFormat(requested ?? format.ToString().ToLowerInvariant(), resolver);
        }

        var bytes = exporter.Render(document);
        return Results.File(bytes, exporter.ContentType, $"{fileBase}.{exporter.FileExtension}");
    }

    /// <summary>Sanitise a value for use in a download filename.</summary>
    public static string SafeFileBase(string s)
    {
        var chars = s.Select(c => char.IsLetterOrDigit(c) || c is '-' or '_' ? c : '-').ToArray();
        var cleaned = new string(chars).Trim('-');
        return cleaned.Length == 0 ? "report" : cleaned;
    }
}
