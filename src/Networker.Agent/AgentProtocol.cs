using System.Text.Json;
using System.Text.Json.Serialization;

namespace Networker.Agent;

// ─────────────────────────────────────────────────────────────────────────────
// Agent ⇄ control-plane wire protocol — the agent-side mirror of the Rust
// `AgentMessage` (outbound) and `ControlMessage` (inbound) enums
// (crates/networker-common/src/messages.rs). This is a self-contained copy so
// the Agent project does not take a dependency on the ControlPlane project (they
// deploy independently); the shapes are byte-for-byte identical to
// Networker.ControlPlane.Realtime.AgentProtocol so the two sides interop.
//
// WIRE CONTRACT (must match the Rust serde output field-for-field):
//   * Both enums are externally tagged: `#[serde(tag = "type",
//     rename_all = "snake_case")]`. Rust writes `{"type":"<snake_case>", ...}`.
//     Reproduced here with System.Text.Json polymorphism
//     (`TypeDiscriminatorPropertyName = "type"`), which writes the discriminator
//     inline as a sibling of the payload fields (not nested).
//   * Every field is snake_case, pinned with explicit [JsonPropertyName].
//   * The tuple-newtype Rust variants — CommandLog / CommandResult (agent→cp)
//     and Command / Cancel (cp→agent) — serde-flatten the inner struct's fields
//     alongside `type`, so the C# records declare those fields directly.
//   * `RunStatus` / `CommandStatus` / `LogStream` serialize `rename_all =
//     "lowercase"` — carried as plain lowercase strings on the wire.
//   * Opaque nested payloads (`attempt`, artifact sections, run/config, command
//     `args`/`result`) are carried as JsonElement and forwarded verbatim.
// ─────────────────────────────────────────────────────────────────────────────

// ── Agent → control plane ────────────────────────────────────────────────────

/// <summary>Agent → control plane message (outbound). Serialised flat with a
/// leading <c>"type"</c> discriminator, matching Rust <c>AgentMessage</c>.</summary>
[JsonPolymorphic(TypeDiscriminatorPropertyName = "type")]
[JsonDerivedType(typeof(HeartbeatMessage), "heartbeat")]
[JsonDerivedType(typeof(RunStartedMessage), "run_started")]
[JsonDerivedType(typeof(RunProgressMessage), "run_progress")]
[JsonDerivedType(typeof(AttemptEventMessage), "attempt_event")]
[JsonDerivedType(typeof(RunFinishedMessage), "run_finished")]
[JsonDerivedType(typeof(ErrorMessage), "error")]
[JsonDerivedType(typeof(CommandLogMessage), "command_log")]
[JsonDerivedType(typeof(CommandResultMessage), "command_result")]
public abstract record AgentMessage;

/// <summary><c>{"type":"heartbeat","load":?,"version":?}</c></summary>
public sealed record HeartbeatMessage(
    [property: JsonPropertyName("load")] double? Load,
    [property: JsonPropertyName("version")] string? Version
) : AgentMessage;

/// <summary><c>{"type":"run_started","run_id":...,"started_at":...}</c></summary>
public sealed record RunStartedMessage(
    [property: JsonPropertyName("run_id")] Guid RunId,
    [property: JsonPropertyName("started_at")] DateTimeOffset StartedAt
) : AgentMessage;

/// <summary><c>{"type":"run_progress","run_id":...,"success":u32,"failure":u32}</c></summary>
public sealed record RunProgressMessage(
    [property: JsonPropertyName("run_id")] Guid RunId,
    [property: JsonPropertyName("success")] uint Success,
    [property: JsonPropertyName("failure")] uint Failure
) : AgentMessage;

/// <summary><c>{"type":"attempt_event","run_id":...,"attempt":{...}}</c> —
/// <c>attempt</c> is the tester's serialized RequestAttempt, forwarded verbatim.</summary>
public sealed record AttemptEventMessage(
    [property: JsonPropertyName("run_id")] Guid RunId,
    [property: JsonPropertyName("attempt")] JsonElement Attempt
) : AgentMessage;

/// <summary><c>{"type":"run_finished","run_id":...,"status":...,"artifact":{...}?}</c>.
/// <c>artifact</c> omitted when null (Rust <c>skip_serializing_if = Option::is_none</c>).</summary>
public sealed record RunFinishedMessage(
    [property: JsonPropertyName("run_id")] Guid RunId,
    [property: JsonPropertyName("status")] string Status,
    [property: JsonPropertyName("artifact")]
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    BenchmarkArtifactPayload? Artifact
) : AgentMessage;

/// <summary><c>{"type":"error","run_id":?,"message":...}</c> — <c>run_id</c> omitted
/// when null (Rust <c>skip_serializing_if = Option::is_none</c>).</summary>
public sealed record ErrorMessage(
    [property: JsonPropertyName("run_id")]
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    Guid? RunId,
    [property: JsonPropertyName("message")] string Message
) : AgentMessage;

/// <summary><c>{"type":"command_log","command_id":...,"stream":"stdout|stderr","line":...}</c>
/// (flattened newtype).</summary>
public sealed record CommandLogMessage(
    [property: JsonPropertyName("command_id")] Guid CommandId,
    [property: JsonPropertyName("stream")] string Stream,
    [property: JsonPropertyName("line")] string Line
) : AgentMessage;

/// <summary><c>{"type":"command_result","command_id":...,"status":...,"result":{...}?,"error":...?,"duration_ms":...}</c>
/// (flattened newtype). <c>result</c>/<c>error</c> carry <c>#[serde(default)]</c>
/// on the Rust side (present-with-null is valid); we write them always for the
/// non-null case and null otherwise.</summary>
public sealed record CommandResultMessage(
    [property: JsonPropertyName("command_id")] Guid CommandId,
    [property: JsonPropertyName("status")] string Status,
    [property: JsonPropertyName("result")] JsonElement? Result,
    [property: JsonPropertyName("error")] string? Error,
    [property: JsonPropertyName("duration_ms")] ulong DurationMs
) : AgentMessage;

/// <summary>Benchmark artifact envelope carried by <see cref="RunFinishedMessage"/>.
/// Each section is free-form JSON; <c>samples</c> omitted when null.</summary>
public sealed record BenchmarkArtifactPayload(
    [property: JsonPropertyName("environment")] JsonElement Environment,
    [property: JsonPropertyName("methodology")] JsonElement Methodology,
    [property: JsonPropertyName("launches")] JsonElement Launches,
    [property: JsonPropertyName("cases")] JsonElement Cases,
    [property: JsonPropertyName("samples")]
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    JsonElement? Samples,
    [property: JsonPropertyName("summaries")] JsonElement Summaries,
    [property: JsonPropertyName("data_quality")] JsonElement DataQuality
);

// ── Control plane → Agent ────────────────────────────────────────────────────

/// <summary>Control plane → agent message (inbound). Deserialised via the
/// leading <c>"type"</c> discriminator, matching Rust <c>ControlMessage</c>.</summary>
[JsonPolymorphic(
    TypeDiscriminatorPropertyName = "type",
    UnknownDerivedTypeHandling = JsonUnknownDerivedTypeHandling.FailSerialization)]
[JsonDerivedType(typeof(WelcomeMessage), "welcome")]
[JsonDerivedType(typeof(AssignRunMessage), "assign_run")]
[JsonDerivedType(typeof(CancelRunMessage), "cancel_run")]
[JsonDerivedType(typeof(CommandMessage), "command")]
[JsonDerivedType(typeof(CancelMessage), "cancel")]
[JsonDerivedType(typeof(HeartbeatPingMessage), "heartbeat_ping")]
[JsonDerivedType(typeof(ShutdownMessage), "shutdown")]
public abstract record ControlMessage;

/// <summary><c>{"type":"welcome","agent_id":...,"agent_name":...}</c></summary>
public sealed record WelcomeMessage(
    [property: JsonPropertyName("agent_id")] Guid AgentId,
    [property: JsonPropertyName("agent_name")] string AgentName
) : ControlMessage;

/// <summary><c>{"type":"assign_run","run":{...},"config":{...}}</c> — both payloads
/// opaque JsonElement (the canonical Rust TestRun/TestConfig shapes).</summary>
public sealed record AssignRunMessage(
    [property: JsonPropertyName("run")] JsonElement Run,
    [property: JsonPropertyName("config")] JsonElement Config
) : ControlMessage;

/// <summary><c>{"type":"cancel_run","run_id":...}</c></summary>
public sealed record CancelRunMessage(
    [property: JsonPropertyName("run_id")] Guid RunId
) : ControlMessage;

/// <summary><c>{"type":"command","command_id":...,"config_id":?,"token":...,"verb":...,"args":{...},"timeout_secs":...}</c>
/// (flattened newtype).</summary>
public sealed record CommandMessage(
    [property: JsonPropertyName("command_id")] Guid CommandId,
    [property: JsonPropertyName("config_id")] Guid? ConfigId,
    [property: JsonPropertyName("token")] string Token,
    [property: JsonPropertyName("verb")] string Verb,
    [property: JsonPropertyName("args")] JsonElement Args,
    [property: JsonPropertyName("timeout_secs")] ulong TimeoutSecs
) : ControlMessage;

/// <summary><c>{"type":"cancel","command_id":...}</c> (flattened newtype).</summary>
public sealed record CancelMessage(
    [property: JsonPropertyName("command_id")] Guid CommandId
) : ControlMessage;

/// <summary><c>{"type":"heartbeat_ping","now":...}</c></summary>
public sealed record HeartbeatPingMessage(
    [property: JsonPropertyName("now")] DateTimeOffset Now
) : ControlMessage;

/// <summary><c>{"type":"shutdown"}</c> — unit variant (serialises to just the tag).</summary>
public sealed record ShutdownMessage : ControlMessage;

/// <summary>Shared JSON options for agent protocol (de)serialization — matches
/// the Rust serde defaults (no indentation, nulls handled per-property).</summary>
public static class AgentProtocolJson
{
    public static readonly JsonSerializerOptions Options = new()
    {
        DefaultIgnoreCondition = JsonIgnoreCondition.Never,
    };
}
