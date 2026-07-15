using System.Text.Json;
using Networker.Agent;

namespace Networker.Agent.Tests;

/// <summary>
/// Wire-protocol tests: inbound <see cref="ControlMessage"/> discriminator
/// dispatch (which C# record each <c>"type"</c> resolves to) and outbound
/// <see cref="AgentMessage"/> JSON shape (must match the Rust serde output the
/// control plane decodes).
/// </summary>
public class ProtocolWireTests
{
    private static ControlMessage? Decode(string json) =>
        JsonSerializer.Deserialize<ControlMessage>(json, AgentProtocolJson.Options);

    private static string Encode(AgentMessage msg) =>
        JsonSerializer.Serialize(msg, AgentProtocolJson.Options);

    // ── Inbound ControlMessage dispatch selection ────────────────────────────────

    [Fact]
    public void Welcome_frame_dispatches_to_WelcomeMessage()
    {
        var msg = Decode("""{"type":"welcome","agent_id":"11111111-1111-1111-1111-111111111111","agent_name":"agent-a"}""");
        var w = Assert.IsType<WelcomeMessage>(msg);
        Assert.Equal("agent-a", w.AgentName);
        Assert.Equal(Guid.Parse("11111111-1111-1111-1111-111111111111"), w.AgentId);
    }

    [Fact]
    public void AssignRun_frame_dispatches_and_carries_opaque_payloads()
    {
        var msg = Decode("""
            {"type":"assign_run",
             "run":{"id":"22222222-2222-2222-2222-222222222222","status":"running"},
             "config":{"id":"33333333-3333-3333-3333-333333333333","endpoint":{"kind":"network","host":"h"}}}
            """);
        var a = Assert.IsType<AssignRunMessage>(msg);
        Assert.Equal("22222222-2222-2222-2222-222222222222", a.Run.GetProperty("id").GetString());
        Assert.Equal("network", a.Config.GetProperty("endpoint").GetProperty("kind").GetString());
    }

    [Fact]
    public void CancelRun_frame_dispatches_to_CancelRunMessage()
    {
        var msg = Decode("""{"type":"cancel_run","run_id":"44444444-4444-4444-4444-444444444444"}""");
        var c = Assert.IsType<CancelRunMessage>(msg);
        Assert.Equal(Guid.Parse("44444444-4444-4444-4444-444444444444"), c.RunId);
    }

    [Fact]
    public void Command_frame_dispatches_with_flattened_fields()
    {
        var msg = Decode("""
            {"type":"command","command_id":"55555555-5555-5555-5555-555555555555",
             "config_id":null,"token":"opaque.jwt","verb":"health","args":{},"timeout_secs":30}
            """);
        var cmd = Assert.IsType<CommandMessage>(msg);
        Assert.Equal("health", cmd.Verb);
        Assert.Equal("opaque.jwt", cmd.Token);
        Assert.Equal(30ul, cmd.TimeoutSecs);
        Assert.Null(cmd.ConfigId);
    }

    [Fact]
    public void Cancel_frame_dispatches_to_CancelMessage()
    {
        var msg = Decode("""{"type":"cancel","command_id":"66666666-6666-6666-6666-666666666666"}""");
        Assert.IsType<CancelMessage>(msg);
    }

    [Fact]
    public void HeartbeatPing_frame_dispatches_to_HeartbeatPingMessage()
    {
        var msg = Decode("""{"type":"heartbeat_ping","now":"2026-07-15T00:00:00Z"}""");
        Assert.IsType<HeartbeatPingMessage>(msg);
    }

    [Fact]
    public void Shutdown_frame_dispatches_to_ShutdownMessage()
    {
        var msg = Decode("""{"type":"shutdown"}""");
        Assert.IsType<ShutdownMessage>(msg);
    }

    [Fact]
    public void Unknown_type_throws_so_receive_loop_can_ignore_it()
    {
        // The receive loop catches JsonException and ignores the frame (Rust:
        // `if let Ok(ctrl) = decode(...)`). Assert the decode does fail loudly.
        Assert.ThrowsAny<JsonException>(() => Decode("""{"type":"job_assign","job_id":"x"}"""));
    }

    // ── Outbound AgentMessage wire shape ─────────────────────────────────────────

    [Fact]
    public void Heartbeat_serializes_with_type_and_null_load()
    {
        var json = Encode(new HeartbeatMessage(Load: null, Version: "0.28.13"));
        using var doc = JsonDocument.Parse(json);
        Assert.Equal("heartbeat", doc.RootElement.GetProperty("type").GetString());
        Assert.Equal(JsonValueKind.Null, doc.RootElement.GetProperty("load").ValueKind);
        Assert.Equal("0.28.13", doc.RootElement.GetProperty("version").GetString());
    }

    [Fact]
    public void RunStarted_serializes_type_and_fields()
    {
        var runId = Guid.NewGuid();
        var json = Encode(new RunStartedMessage(runId, DateTimeOffset.UnixEpoch));
        using var doc = JsonDocument.Parse(json);
        Assert.Equal("run_started", doc.RootElement.GetProperty("type").GetString());
        Assert.Equal(runId.ToString(), doc.RootElement.GetProperty("run_id").GetString());
        Assert.True(doc.RootElement.TryGetProperty("started_at", out _));
    }

    [Fact]
    public void RunProgress_serializes_success_and_failure()
    {
        var json = Encode(new RunProgressMessage(Guid.Empty, 7, 3));
        using var doc = JsonDocument.Parse(json);
        Assert.Equal("run_progress", doc.RootElement.GetProperty("type").GetString());
        Assert.Equal(7, doc.RootElement.GetProperty("success").GetInt32());
        Assert.Equal(3, doc.RootElement.GetProperty("failure").GetInt32());
    }

    [Fact]
    public void RunFinished_omits_null_artifact()
    {
        var json = Encode(new RunFinishedMessage(Guid.Empty, "completed", null));
        using var doc = JsonDocument.Parse(json);
        Assert.Equal("run_finished", doc.RootElement.GetProperty("type").GetString());
        Assert.Equal("completed", doc.RootElement.GetProperty("status").GetString());
        Assert.False(doc.RootElement.TryGetProperty("artifact", out _)); // skip when null
    }

    [Fact]
    public void Error_with_null_run_id_omits_run_id_but_keeps_message()
    {
        var json = Encode(new ErrorMessage(RunId: null, Message: "boom"));
        using var doc = JsonDocument.Parse(json);
        Assert.Equal("error", doc.RootElement.GetProperty("type").GetString());
        Assert.False(doc.RootElement.TryGetProperty("run_id", out _)); // skip when null
        Assert.Equal("boom", doc.RootElement.GetProperty("message").GetString());
    }

    [Fact]
    public void CommandResult_serializes_flattened_lowercase_status()
    {
        var json = Encode(new CommandResultMessage(Guid.Empty, "ok", null, null, 42));
        using var doc = JsonDocument.Parse(json);
        Assert.Equal("command_result", doc.RootElement.GetProperty("type").GetString());
        Assert.Equal("ok", doc.RootElement.GetProperty("status").GetString());
        Assert.Equal(42, doc.RootElement.GetProperty("duration_ms").GetInt64());
    }

    [Fact]
    public void CommandLog_serializes_flattened_stream_field()
    {
        var json = Encode(new CommandLogMessage(Guid.Empty, "stdout", "hello"));
        using var doc = JsonDocument.Parse(json);
        Assert.Equal("command_log", doc.RootElement.GetProperty("type").GetString());
        Assert.Equal("stdout", doc.RootElement.GetProperty("stream").GetString());
        Assert.Equal("hello", doc.RootElement.GetProperty("line").GetString());
    }
}
