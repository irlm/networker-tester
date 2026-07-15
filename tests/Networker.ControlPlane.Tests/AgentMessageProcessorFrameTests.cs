using System.Text.Json;
using Networker.ControlPlane.Realtime;
using Networker.ControlPlane.Realtime.RawWs;

namespace Networker.ControlPlane.Tests;

/// Frame-codec tests for the transport-agnostic AgentMessageProcessor — the
/// parse/dispatch-selection seam shared by the SignalR hub and the raw
/// /ws/agent endpoint. The samples are the exact `{"type":"...", ...}` text
/// frames the fielded Rust agent (ws_client.rs / networker-common messages.rs)
/// writes, so these tests pin the wire contract: tag-first external tagging,
/// snake_case fields, flattened newtype variants, silent drop of
/// unknown/legacy/undecodable frames.
public sealed class AgentMessageProcessorFrameTests
{
    // ── Dispatch selection: each type tag decodes to its handler's message ──

    [Fact]
    public void Heartbeat_frame_decodes_with_load_and_version()
    {
        var msg = AgentMessageProcessor.Decode(
            """{"type":"heartbeat","load":0.42,"version":"0.28.13"}""");

        var hb = Assert.IsType<HeartbeatMessage>(msg);
        Assert.Equal(0.42, hb.Load);
        Assert.Equal("0.28.13", hb.Version);
    }

    [Fact]
    public void Heartbeat_frame_with_null_optionals_decodes()
    {
        var msg = AgentMessageProcessor.Decode(
            """{"type":"heartbeat","load":null,"version":null}""");

        var hb = Assert.IsType<HeartbeatMessage>(msg);
        Assert.Null(hb.Load);
        Assert.Null(hb.Version);
    }

    [Fact]
    public void Run_started_frame_decodes_run_id_and_timestamp()
    {
        var runId = Guid.NewGuid();
        var msg = AgentMessageProcessor.Decode(
            $$"""{"type":"run_started","run_id":"{{runId}}","started_at":"2026-07-14T12:34:56Z"}""");

        var rs = Assert.IsType<RunStartedMessage>(msg);
        Assert.Equal(runId, rs.RunId);
        Assert.Equal(new DateTimeOffset(2026, 7, 14, 12, 34, 56, TimeSpan.Zero), rs.StartedAt);
    }

    [Fact]
    public void Run_finished_frame_without_artifact_decodes()
    {
        var runId = Guid.NewGuid();
        var msg = AgentMessageProcessor.Decode(
            $$"""{"type":"run_finished","run_id":"{{runId}}","status":"completed","artifact":null}""");

        var rf = Assert.IsType<RunFinishedMessage>(msg);
        Assert.Equal(runId, rf.RunId);
        Assert.Equal("completed", rf.Status);
        Assert.Null(rf.Artifact);
    }

    [Fact]
    public void Run_finished_frame_with_artifact_carries_all_sections()
    {
        var runId = Guid.NewGuid();
        var msg = AgentMessageProcessor.Decode(
            $$"""
            {"type":"run_finished","run_id":"{{runId}}","status":"failed","artifact":{
                "environment":{"os":"linux"},
                "methodology":{"warmup":3},
                "launches":[{"n":1}],
                "cases":[{"case":"http1"}],
                "samples":null,
                "summaries":{"p50":12.5},
                "data_quality":{"outliers":0}
            } }
            """);

        var rf = Assert.IsType<RunFinishedMessage>(msg);
        Assert.Equal("failed", rf.Status);
        Assert.NotNull(rf.Artifact);
        Assert.Equal("linux", rf.Artifact!.Environment.GetProperty("os").GetString());
        Assert.Equal(3, rf.Artifact.Methodology.GetProperty("warmup").GetInt32());
        Assert.Equal(12.5, rf.Artifact.Summaries.GetProperty("p50").GetDouble());
        Assert.Equal(0, rf.Artifact.DataQuality.GetProperty("outliers").GetInt32());
    }

    [Fact]
    public void Error_frame_with_run_id_decodes()
    {
        var runId = Guid.NewGuid();
        var msg = AgentMessageProcessor.Decode(
            $$"""{"type":"error","run_id":"{{runId}}","message":"probe engine crashed"}""");

        var err = Assert.IsType<ErrorMessage>(msg);
        Assert.Equal(runId, err.RunId);
        Assert.Equal("probe engine crashed", err.Message);
    }

    [Fact]
    public void Error_frame_without_run_id_decodes_as_global_error()
    {
        var msg = AgentMessageProcessor.Decode(
            """{"type":"error","run_id":null,"message":"config reload failed"}""");

        var err = Assert.IsType<ErrorMessage>(msg);
        Assert.Null(err.RunId);
        Assert.Equal("config reload failed", err.Message);
    }

    [Fact]
    public void Command_result_frame_decodes_flattened_newtype_fields()
    {
        var commandId = Guid.NewGuid();
        var msg = AgentMessageProcessor.Decode(
            $$"""{"type":"command_result","command_id":"{{commandId}}","status":"ok","result":{"exit_code":0},"error":null,"duration_ms":1234}""");

        var cr = Assert.IsType<CommandResultMessage>(msg);
        Assert.Equal(commandId, cr.CommandId);
        Assert.Equal("ok", cr.Status);
        Assert.Equal(1234, cr.DurationMs);
        Assert.Null(cr.Error);
        Assert.NotNull(cr.Result);
        Assert.Equal(0, cr.Result!.Value.GetProperty("exit_code").GetInt32());
    }

    [Fact]
    public void Run_progress_and_attempt_event_frames_decode()
    {
        var runId = Guid.NewGuid();

        var progress = AgentMessageProcessor.Decode(
            $$"""{"type":"run_progress","run_id":"{{runId}}","success":17,"failure":3}""");
        var rp = Assert.IsType<RunProgressMessage>(progress);
        Assert.Equal(17, rp.Success);
        Assert.Equal(3, rp.Failure);

        var attempt = AgentMessageProcessor.Decode(
            $$"""{"type":"attempt_event","run_id":"{{runId}}","attempt":{"phase_ms":{"dns":2} } }""");
        var ae = Assert.IsType<AttemptEventMessage>(attempt);
        Assert.Equal(runId, ae.RunId);
        Assert.Equal(2, ae.Attempt.GetProperty("phase_ms").GetProperty("dns").GetInt32());
    }

    // ── Drop semantics: unknown / legacy / undecodable frames → null ────────

    [Theory]
    [InlineData("""{"type":"job_ack","job_id":"00000000-0000-0000-0000-000000000001"}""")] // legacy v1
    [InlineData("""{"type":"totally_unknown"}""")]
    [InlineData("""{"no_type_at_all":true}""")]
    [InlineData("not json at all")]
    [InlineData("")]
    [InlineData("null")]
    public void Unknown_or_undecodable_frames_are_dropped(string frame)
    {
        Assert.Null(AgentMessageProcessor.Decode(frame));
    }

    // ── Outbound codec: control frames match the Rust wire shape ────────────

    [Fact]
    public void Welcome_frame_matches_rust_wire_shape()
    {
        var agentId = Guid.NewGuid();
        var frame = AgentMessageProcessor.WelcomeFrame(agentId, "edge-probe-01");

        using var doc = JsonDocument.Parse(frame);
        Assert.Equal("welcome", doc.RootElement.GetProperty("type").GetString());
        Assert.Equal(agentId, doc.RootElement.GetProperty("agent_id").GetGuid());
        Assert.Equal("edge-probe-01", doc.RootElement.GetProperty("agent_name").GetString());
    }

    [Fact]
    public void Encoded_control_messages_carry_the_type_discriminator()
    {
        var runId = Guid.NewGuid();
        var frame = AgentMessageProcessor.EncodeControl(new CancelRunMessage(runId));

        using var doc = JsonDocument.Parse(frame);
        Assert.Equal("cancel_run", doc.RootElement.GetProperty("type").GetString());
        Assert.Equal(runId, doc.RootElement.GetProperty("run_id").GetGuid());
    }
}
