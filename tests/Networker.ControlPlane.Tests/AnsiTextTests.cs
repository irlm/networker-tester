namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pins <see cref="AnsiText.Strip"/> — the server-side half of design-audit F8
/// (ANSI escapes stored in <c>error_message</c>). Mirrors the frontend's
/// <c>dashboard/src/lib/ansi.test.ts</c> case-for-case (same pattern, same
/// audit log line) so the two strips can never diverge silently.
/// </summary>
public sealed class AnsiTextTests
{
    private const string Esc = "";
    private const string Bel = "";
    private const string Csi8 = "";

    [Fact]
    public void Plain_text_passes_through_unchanged()
    {
        Assert.Equal(
            "connection refused (os error 111)",
            AnsiText.Strip("connection refused (os error 111)"));
    }

    [Fact]
    public void Bracketed_text_without_escape_bytes_is_left_alone()
    {
        Assert.Equal("[tester] probe failed", AnsiText.Strip("[tester] probe failed"));
    }

    [Fact]
    public void Strips_sgr_color_and_style_codes()
    {
        Assert.Equal(" INFO  ready", AnsiText.Strip($"{Esc}[32m INFO {Esc}[0m ready"));
        Assert.Equal(
            "2026-07-14T01:22:24.974248Z",
            AnsiText.Strip($"{Esc}[2m2026-07-14T01:22:24.974248Z{Esc}[0m"));
    }

    [Fact]
    public void Strips_the_exact_tester_log_line_from_the_audit()
    {
        // The literal line the design audit captured rendered raw in the UI
        // (F8) — the tester's tracing subscriber colorizing stderr, relayed
        // verbatim by the agent into error_message.
        var raw = $"[tester] {Esc}[2m2026-07-14T01:22:24.974248Z{Esc}[0m " +
                  $"{Esc}[32m INFO{Esc}[0m {Esc}[2mnetworker_tester{Esc}[0m probe failed";

        Assert.Equal(
            "[tester] 2026-07-14T01:22:24.974248Z  INFO networker_tester probe failed",
            AnsiText.Strip(raw));
    }

    [Fact]
    public void Strips_multi_parameter_and_cursor_erase_sequences()
    {
        Assert.Equal(
            "FAIL done",
            AnsiText.Strip($"{Esc}[1;31mFAIL{Esc}[0m {Esc}[2K{Esc}[1Adone"));
    }

    [Fact]
    public void Strips_osc_sequences()
    {
        Assert.Equal("run complete", AnsiText.Strip($"{Esc}]0;tester{Bel}run complete"));
    }

    [Fact]
    public void Strips_eight_bit_csi()
    {
        Assert.Equal("plain", AnsiText.Strip($"{Csi8}31mplain"));
    }

    [Fact]
    public void Null_and_empty_pass_through()
    {
        Assert.Null(AnsiText.Strip(null));
        Assert.Equal(string.Empty, AnsiText.Strip(string.Empty));
    }
}
