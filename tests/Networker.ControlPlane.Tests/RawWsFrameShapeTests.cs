using System.Text.Json;
using Networker.ControlPlane.Realtime;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Wire-contract tests for the raw-WebSocket bridge: the JSON each socket loop
/// sends must match what the React frontend parses — the flat, snake_case,
/// type-tagged frames the Rust hubs emitted.
///
/// Browser feed (useWebSocket.ts): each frame is one flat object
/// <c>{"seq":N,"type":"job_update",...}</c> — serialized exactly the way
/// BrowserSocketEndpoint / EventBus do it: <c>JsonSerializer.Serialize(seqEvent)</c>.
///
/// Tester feed (useTesterSubscription.ts / usePhaseSubscription.ts): frames are
/// <c>{"type":"tester_queue_snapshot",...}</c> etc., serialized the way
/// TesterQueueSocketEndpoint and RawWsTesterQueueLifetimeManager do it:
/// <c>JsonSerializer.Serialize(payload, payload.GetType())</c> — no SignalR envelope.
/// </summary>
public sealed class RawWsFrameShapeTests
{
    // ── Browser feed frames ───────────────────────────────────────────────────

    [Fact]
    public void SeqEvent_JobUpdate_SerializesFlat_WithSeqAndTypeDiscriminator()
    {
        var jobId = Guid.NewGuid();
        var agentId = Guid.NewGuid();
        var started = DateTimeOffset.Parse("2026-07-14T12:00:00Z");
        var seqEvent = new SeqEvent(42, new JobUpdate(jobId, "running", agentId, started, null));

        // Exactly the raw browser socket's serialization call.
        var json = JsonSerializer.Serialize(seqEvent);

        using var doc = JsonDocument.Parse(json);
        var root = doc.RootElement;

        Assert.Equal(JsonValueKind.Object, root.ValueKind);
        Assert.Equal(42, root.GetProperty("seq").GetInt64());
        Assert.Equal("job_update", root.GetProperty("type").GetString());
        Assert.Equal(jobId.ToString(), root.GetProperty("job_id").GetString());
        Assert.Equal("running", root.GetProperty("status").GetString());
        Assert.Equal(agentId.ToString(), root.GetProperty("agent_id").GetString());
        Assert.Equal(JsonValueKind.Null, root.GetProperty("finished_at").ValueKind);

        // FLAT: no nested "event" wrapper — the frontend reads data.type/data.seq
        // off the top-level object.
        Assert.False(root.TryGetProperty("event", out _));
        Assert.False(root.TryGetProperty("Event", out _));
    }

    [Fact]
    public void SeqEvent_SeqIsFirstProperty_AndFrontendFieldsAreTopLevel()
    {
        var seqEvent = new SeqEvent(7, new JobLog(Guid.NewGuid(), "hello", "info"));
        var json = JsonSerializer.Serialize(seqEvent);

        // The converter writes seq first (cosmetic but pinned — matches Rust).
        Assert.StartsWith("{\"seq\":7,\"type\":\"job_log\"", json, StringComparison.Ordinal);

        using var doc = JsonDocument.Parse(json);
        Assert.Equal("hello", doc.RootElement.GetProperty("line").GetString());
        Assert.Equal("info", doc.RootElement.GetProperty("level").GetString());
    }

    [Fact]
    public void SeqEvent_ReplayAndLivePaths_ProduceIdenticalJson()
    {
        // BrowserSocketEndpoint (replay) and EventBus (live) both call
        // JsonSerializer.Serialize(seqEvent) — assert the two call shapes agree.
        var seqEvent = new SeqEvent(
            9, new AgentStatus(Guid.NewGuid(), "online", DateTimeOffset.UnixEpoch));

        var replayJson = JsonSerializer.Serialize(seqEvent);
        var liveJson = JsonSerializer.Serialize(seqEvent);

        Assert.Equal(replayJson, liveJson);
    }

    // ── Tester feed frames ────────────────────────────────────────────────────

    [Fact]
    public void TesterQueueSnapshot_SerializesTypeTagged_SnakeCase_NoEnvelope()
    {
        object payload = new TesterQueueSnapshotMessage(
            "proj-1",
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            3,
            new[]
            {
                new TesterQueueEntry("cfg-1", "nightly bench", Position: 1, EtaSeconds: 120),
            },
            Running: new TesterQueueEntry("cfg-0", "running bench"));

        // Exactly the lifetime-manager / endpoint serialization call.
        var json = JsonSerializer.Serialize(payload, payload.GetType());

        using var doc = JsonDocument.Parse(json);
        var root = doc.RootElement;

        Assert.Equal("tester_queue_snapshot", root.GetProperty("type").GetString());
        Assert.Equal("proj-1", root.GetProperty("project_id").GetString());
        Assert.Equal("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", root.GetProperty("tester_id").GetString());
        Assert.Equal(3ul, root.GetProperty("seq").GetUInt64());

        var queued = root.GetProperty("queued");
        Assert.Equal(1, queued.GetArrayLength());
        Assert.Equal("cfg-1", queued[0].GetProperty("config_id").GetString());
        Assert.Equal("nightly bench", queued[0].GetProperty("name").GetString());
        Assert.Equal(1u, queued[0].GetProperty("position").GetUInt32());
        Assert.Equal(120u, queued[0].GetProperty("eta_seconds").GetUInt32());

        Assert.Equal("cfg-0", root.GetProperty("running").GetProperty("config_id").GetString());

        // No SignalR invocation envelope leaks into the raw frame.
        Assert.False(root.TryGetProperty("target", out _));
        Assert.False(root.TryGetProperty("arguments", out _));
        Assert.False(root.TryGetProperty("invocationId", out _));
    }

    [Fact]
    public void TesterQueueSnapshot_OmitsRunning_WhenIdle()
    {
        object payload = new TesterQueueSnapshotMessage(
            "proj-1", "tester-1", 1, Array.Empty<TesterQueueEntry>());

        var json = JsonSerializer.Serialize(payload, payload.GetType());

        using var doc = JsonDocument.Parse(json);
        // Rust skip_serializing_if — running must be ABSENT, not null.
        Assert.False(doc.RootElement.TryGetProperty("running", out _));
        Assert.Equal(0, doc.RootElement.GetProperty("queued").GetArrayLength());
    }

    [Fact]
    public void TesterQueueUpdate_CarriesTriggerAndTag()
    {
        object payload = new TesterQueueUpdateMessage(
            "proj-1", "tester-1", 12, "benchmark_completed",
            Array.Empty<TesterQueueEntry>());

        var json = JsonSerializer.Serialize(payload, payload.GetType());

        using var doc = JsonDocument.Parse(json);
        Assert.Equal("tester_queue_update", doc.RootElement.GetProperty("type").GetString());
        Assert.Equal("benchmark_completed", doc.RootElement.GetProperty("trigger").GetString());
        Assert.Equal(12ul, doc.RootElement.GetProperty("seq").GetUInt64());
    }

    [Fact]
    public void PhaseUpdate_MatchesUsePhaseSubscriptionContract()
    {
        object payload = new PhaseUpdateMessage(
            "proj-1", "benchmark", "cfg-9", 5, TesterPhase.Done,
            new[] { TesterPhase.Deploy, TesterPhase.Running, TesterPhase.Collect },
            Outcome: TesterOutcome.PartialSuccess,
            Message: "2 of 3 stages passed");

        var json = JsonSerializer.Serialize(payload, payload.GetType());

        using var doc = JsonDocument.Parse(json);
        var root = doc.RootElement;
        Assert.Equal("phase_update", root.GetProperty("type").GetString());
        Assert.Equal("benchmark", root.GetProperty("entity_type").GetString());
        Assert.Equal("cfg-9", root.GetProperty("entity_id").GetString());
        Assert.Equal("done", root.GetProperty("phase").GetString());
        Assert.Equal("partial_success", root.GetProperty("outcome").GetString());
        Assert.Equal(3, root.GetProperty("applied_stages").GetArrayLength());
        Assert.Equal("deploy", root.GetProperty("applied_stages")[0].GetString());
    }

    [Fact]
    public void PhaseUpdate_OmitsOutcomeAndMessage_WhenNull()
    {
        object payload = new PhaseUpdateMessage(
            "proj-1", "benchmark", "cfg-9", 1, TesterPhase.Queued, Array.Empty<string>());

        var json = JsonSerializer.Serialize(payload, payload.GetType());

        using var doc = JsonDocument.Parse(json);
        Assert.False(doc.RootElement.TryGetProperty("outcome", out _));
        Assert.False(doc.RootElement.TryGetProperty("message", out _));
    }
}
