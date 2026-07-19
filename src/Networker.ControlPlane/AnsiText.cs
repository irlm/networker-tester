using System.Text.RegularExpressions;

namespace Networker.ControlPlane;

/// <summary>
/// Strips ANSI/VT100 escape sequences from text the control plane persists.
///
/// <para>The agent streams the tester's stderr back verbatim as
/// <c>error</c> frames, and the tester's tracing subscriber colorizes stderr —
/// so raw SGR codes (<c>\x1b[2m</c>, <c>\x1b[32m</c>, …) used to land in
/// <c>test_run.error_message</c> / <c>agent_command.error_message</c> and leak
/// to every consumer (API, exports, UI — design audit F8). Scrubbing at the
/// ingest write path keeps the STORED data clean; the frontend's
/// <c>stripAnsi()</c> (<c>dashboard/src/lib/ansi.ts</c>) stays as the
/// belt-and-braces display guard and this pattern mirrors it exactly.</para>
///
/// <para>Covers, in order: OSC sequences (ESC ] … BEL/ST — terminal title
/// etc.), CSI sequences (ESC [ params intermediates final-byte — SGR colors,
/// cursor movement, erase), two-byte ESC codes, and 8-bit CSI (0x9B).</para>
/// </summary>
public static partial class AnsiText
{
    [GeneratedRegex(
        "\\u001B\\][^\\u0007\\u001B]*(?:\\u0007|\\u001B\\\\)" +
        "|\\u001B\\[[0-9;:?]*[ -/]*[@-~]" +
        "|\\u001B[@-Z\\\\-_]" +
        "|\\u009B[0-9;:?]*[ -/]*[@-~]")]
    private static partial Regex AnsiPattern();

    /// <summary>
    /// Remove all ANSI escape sequences. Null/empty pass through unchanged;
    /// plain text is returned as-is (no allocation beyond the regex scan).
    /// </summary>
    public static string? Strip(string? input)
        => string.IsNullOrEmpty(input) ? input : AnsiPattern().Replace(input, string.Empty);
}
