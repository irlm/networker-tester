using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Tests for the SSE wire-format helpers and the agent-command stream's
/// status-presentation logic (pending/running inference from started_at).
/// </summary>
public class SseFormattingTests
{
    // ── ServerSentEvents framing ─────────────────────────────────────────────

    [Fact]
    public void Named_event_frames_correctly()
    {
        var frame = ServerSentEvents.FormatEvent("done", "{\"ok\":true}");
        Assert.Equal("event: done\ndata: {\"ok\":true}\n\n", frame);
    }

    [Fact]
    public void Unnamed_event_omits_event_line()
    {
        var frame = ServerSentEvents.FormatEvent(null, "{}");
        Assert.Equal("data: {}\n\n", frame);
    }

    [Fact]
    public void Multiline_data_splits_into_multiple_data_lines()
    {
        // Per the SSE spec a literal newline in the payload must become
        // separate data: lines, or the frame terminator is corrupted.
        var frame = ServerSentEvents.FormatEvent("log", "line1\nline2");
        Assert.Equal("event: log\ndata: line1\ndata: line2\n\n", frame);
    }

    [Fact]
    public void Comment_frame_is_ignored_by_eventsource_clients()
    {
        Assert.Equal(": keep-alive\n\n", ServerSentEvents.FormatComment("keep-alive"));
    }

    [Fact]
    public void Every_frame_ends_with_blank_line_terminator()
    {
        Assert.EndsWith("\n\n", ServerSentEvents.FormatEvent("x", "y"));
        Assert.EndsWith("\n\n", ServerSentEvents.FormatComment("tick"));
    }

    // ── EffectiveStatus (stream presentation) ────────────────────────────────

    [Fact]
    public void Pending_with_no_started_at_stays_pending()
    {
        Assert.Equal("pending",
            AgentCommandsEndpoints.EffectiveStatus("pending", startedAt: null, finishedAt: null));
    }

    [Fact]
    public void Pending_with_started_at_presents_as_running()
    {
        Assert.Equal("running",
            AgentCommandsEndpoints.EffectiveStatus("pending", DateTime.UtcNow, finishedAt: null));
    }

    [Theory]
    [InlineData("ok")]
    [InlineData("error")]
    [InlineData("timeout")]
    [InlineData("cancelled")]
    public void Terminal_statuses_are_reported_verbatim(string status)
    {
        Assert.Equal(status,
            AgentCommandsEndpoints.EffectiveStatus(status, DateTime.UtcNow, DateTime.UtcNow));
    }

    [Fact]
    public void Dispatch_error_before_start_is_not_masked_as_running()
    {
        // mark_dispatch_error leaves started_at NULL and sets status='error';
        // a finished row must never be rewritten to "running".
        Assert.Equal("error",
            AgentCommandsEndpoints.EffectiveStatus("error", startedAt: null, finishedAt: DateTime.UtcNow));
    }
}
