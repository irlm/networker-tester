using System.Text.Json;
using System.Text.Json.Serialization;

namespace Networker.ControlPlane.Realtime;

// ─────────────────────────────────────────────────────────────────────────────
// Agent ⇄ control-plane wire protocol — the C# mirror of the Rust
// `AgentMessage` (inbound) and `ControlMessage` (outbound) enums
// (crates/networker-common/src/messages.rs). Only the WS-v2 variants that the
// live dashboard actually speaks are modelled; the legacy v1 variants the Rust
// enum still carries for the parallel-agent transition (JobAssign / JobAck /
// JobComplete / JobError / JobLog / TlsProfileComplete / AttemptResult /
// JobCancel) are intentionally omitted — the Rust handler ignores every one of
// them (`_ => trace!("Ignored legacy v1 agent message")`).
//
// WIRE CONTRACT (must match the Rust serde output field-for-field):
//   * Both enums are externally tagged: `#[serde(tag = "type",
//     rename_all = "snake_case")]`. Rust writes `{"type":"<snake_case>", ...}`.
//     Here that is reproduced with System.Text.Json polymorphism using
//     `TypeDiscriminatorPropertyName = "type"`, which writes the discriminator
//     inline as a sibling of the payload fields (not nested).
//   * Every field is snake_case, pinned with explicit [JsonPropertyName] so the
//     shape does not drift with ambient JsonSerializerOptions.
//   * The tuple-newtype Rust variants — `CommandLog(AgentCommandLog)`,
//     `CommandResult(AgentCommandResult)`, `Command(AgentCommand)`,
//     `Cancel(AgentCommandCancel)` — serialize with the inner struct's fields
//     lifted flat alongside `type` (serde flattens newtype variants). So the
//     C# records for those variants declare the inner struct's fields directly.
//   * `RunStatus` serializes `rename_all = "lowercase"`
//     (queued/provisioning/running/completed/failed/cancelled). Carried as a
//     plain string here; no enum modelling needed on the wire.
//   * Opaque nested payloads (`attempt`, artifact sections, command `args` /
//     `result`) are carried as JsonElement and forwarded verbatim — the hub
//     never re-models the probe-result / artifact schema, matching the Rust
//     side which boxes `RequestAttempt` / uses `serde_json::Value`.
// ─────────────────────────────────────────────────────────────────────────────

/// <summary>
/// Agent → control plane message. Polymorphic base: each concrete record
/// serialises flat with a leading <c>"type"</c> discriminator, matching the
/// Rust <c>AgentMessage</c> enum's external-tag JSON. Deserialization is the
/// hot path here (the hub decodes frames the agent sends).
/// </summary>
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

/// <summary>
/// <c>{"type":"heartbeat", "load":..., "version":...}</c> — periodic liveness
/// ping with optional load + reported agent version. Mirrors Rust
/// <c>AgentMessage::Heartbeat { load, version }</c> (both fields
/// <c>Option</c>, <c>load</c> defaults).
/// </summary>
public sealed record HeartbeatMessage(
    [property: JsonPropertyName("load")] double? Load,
    [property: JsonPropertyName("version")] string? Version
) : AgentMessage;

/// <summary>
/// <c>{"type":"run_started", "run_id":..., "started_at":...}</c> — agent picked
/// up an assigned run and began executing it. Mirrors Rust
/// <c>AgentMessage::RunStarted { run_id, started_at }</c>.
/// </summary>
public sealed record RunStartedMessage(
    [property: JsonPropertyName("run_id")] Guid RunId,
    [property: JsonPropertyName("started_at")] DateTimeOffset StartedAt
) : AgentMessage;

/// <summary>
/// <c>{"type":"run_progress", "run_id":..., "success":..., "failure":...}</c> —
/// periodic in-flight attempt counters. Mirrors Rust
/// <c>AgentMessage::RunProgress { run_id, success, failure }</c> (u32).
/// </summary>
public sealed record RunProgressMessage(
    [property: JsonPropertyName("run_id")] Guid RunId,
    [property: JsonPropertyName("success")] int Success,
    [property: JsonPropertyName("failure")] int Failure
) : AgentMessage;

/// <summary>
/// <c>{"type":"attempt_event", "run_id":..., "attempt":{...}}</c> — one probe
/// attempt completed (live stream). Mirrors Rust
/// <c>AgentMessage::AttemptEvent { run_id, attempt }</c>. <c>attempt</c> is the
/// opaque serialized RequestAttempt, forwarded verbatim to the browser bus.
/// </summary>
public sealed record AttemptEventMessage(
    [property: JsonPropertyName("run_id")] Guid RunId,
    [property: JsonPropertyName("attempt")] JsonElement Attempt
) : AgentMessage;

/// <summary>
/// <c>{"type":"run_finished", "run_id":..., "status":..., "artifact":{...}?}</c>
/// — run terminated. <c>artifact</c> is present iff the config carried a
/// methodology block (benchmark mode). Mirrors Rust
/// <c>AgentMessage::RunFinished { run_id, status, artifact }</c>.
/// </summary>
public sealed record RunFinishedMessage(
    [property: JsonPropertyName("run_id")] Guid RunId,
    [property: JsonPropertyName("status")] string Status,
    [property: JsonPropertyName("artifact")] BenchmarkArtifactPayload? Artifact
) : AgentMessage;

/// <summary>
/// <c>{"type":"error", "run_id":...?, "message":...}</c> — generic v2 error
/// envelope. <c>run_id</c> optional (a global agent error carries none).
/// Mirrors Rust <c>AgentMessage::Error { run_id, message }</c>.
/// </summary>
public sealed record ErrorMessage(
    [property: JsonPropertyName("run_id")] Guid? RunId,
    [property: JsonPropertyName("message")] string Message
) : AgentMessage;

/// <summary>
/// <c>{"type":"command_log", "command_id":..., "stream":"stdout|stderr", "line":...}</c>
/// — a log line streamed while a dispatched command runs. Mirrors Rust
/// <c>AgentMessage::CommandLog(AgentCommandLog)</c>; because that is a
/// newtype variant, serde flattens the inner struct's fields alongside
/// <c>type</c>, so they are declared directly here. <c>stream</c> is
/// <c>rename_all = "lowercase"</c> (<c>stdout</c> / <c>stderr</c>).
/// </summary>
public sealed record CommandLogMessage(
    [property: JsonPropertyName("command_id")] Guid CommandId,
    [property: JsonPropertyName("stream")] string Stream,
    [property: JsonPropertyName("line")] string Line
) : AgentMessage;

/// <summary>
/// <c>{"type":"command_result", "command_id":..., "status":"ok|error|timeout|cancelled", "result":{...}?, "error":...?, "duration_ms":...}</c>
/// — terminal result of a dispatched command. Mirrors Rust
/// <c>AgentMessage::CommandResult(AgentCommandResult)</c> (flattened newtype).
/// <c>status</c> is <c>rename_all = "lowercase"</c>.
/// </summary>
public sealed record CommandResultMessage(
    [property: JsonPropertyName("command_id")] Guid CommandId,
    [property: JsonPropertyName("status")] string Status,
    [property: JsonPropertyName("result")] JsonElement? Result,
    [property: JsonPropertyName("error")] string? Error,
    [property: JsonPropertyName("duration_ms")] long DurationMs
) : AgentMessage;

/// <summary>
/// The benchmark artifact envelope carried by <see cref="RunFinishedMessage"/>.
/// Mirrors Rust <c>BenchmarkArtifact</c> — each section is free-form JSON
/// (<c>serde_json::Value</c>); <c>samples</c> is optional and skipped when
/// absent. These map 1:1 onto the <c>benchmark_artifact</c> EF entity's jsonb
/// text columns.
/// </summary>
public sealed record BenchmarkArtifactPayload(
    [property: JsonPropertyName("environment")] JsonElement Environment,
    [property: JsonPropertyName("methodology")] JsonElement Methodology,
    [property: JsonPropertyName("launches")] JsonElement Launches,
    [property: JsonPropertyName("cases")] JsonElement Cases,
    [property: JsonPropertyName("samples")] JsonElement? Samples,
    [property: JsonPropertyName("summaries")] JsonElement Summaries,
    [property: JsonPropertyName("data_quality")] JsonElement DataQuality
);

// ─────────────────────────────────────────────────────────────────────────────
// Control plane → Agent messages
// ─────────────────────────────────────────────────────────────────────────────

/// <summary>
/// Control plane → agent message. Polymorphic base: each concrete record
/// serialises flat with a leading <c>"type"</c> discriminator, matching the
/// Rust <c>ControlMessage</c> enum's external-tag JSON. Serialization is the
/// hot path here (the hub pushes these frames to the agent). Only the WS-v2 +
/// command variants the live control plane emits are modelled; the legacy v1
/// <c>JobAssign</c> / <c>JobCancel</c> variants are omitted (never sent by the
/// C# control plane).
/// </summary>
[JsonPolymorphic(TypeDiscriminatorPropertyName = "type")]
[JsonDerivedType(typeof(WelcomeMessage), "welcome")]
[JsonDerivedType(typeof(AssignRunMessage), "assign_run")]
[JsonDerivedType(typeof(CancelRunMessage), "cancel_run")]
[JsonDerivedType(typeof(CommandMessage), "command")]
[JsonDerivedType(typeof(CancelMessage), "cancel")]
[JsonDerivedType(typeof(HeartbeatPingMessage), "heartbeat_ping")]
[JsonDerivedType(typeof(ShutdownMessage), "shutdown")]
public abstract record ControlMessage;

/// <summary>
/// <c>{"type":"welcome", "agent_id":..., "agent_name":...}</c> — sent on
/// connect to acknowledge registration/reconnection. Mirrors Rust
/// <c>ControlMessage::Welcome { agent_id, agent_name }</c>.
/// </summary>
public sealed record WelcomeMessage(
    [property: JsonPropertyName("agent_id")] Guid AgentId,
    [property: JsonPropertyName("agent_name")] string AgentName
) : ControlMessage;

/// <summary>
/// <c>{"type":"assign_run", "run":{...}, "config":{...}}</c> — assign an
/// execution to the agent. Mirrors Rust
/// <c>ControlMessage::AssignRun { run, config }</c>. Both payloads are carried
/// as opaque JsonElement so the M3 dispatcher can hand this hub a pre-serialized
/// TestRun/TestConfig (the canonical Rust shapes) without this file re-modelling
/// them; the agent decodes them into its own <c>TestRun</c> / <c>TestConfig</c>.
/// </summary>
public sealed record AssignRunMessage(
    [property: JsonPropertyName("run")] JsonElement Run,
    [property: JsonPropertyName("config")] JsonElement Config
) : ControlMessage;

/// <summary>
/// <c>{"type":"cancel_run", "run_id":...}</c> — cooperatively cancel an
/// in-flight run. Mirrors Rust <c>ControlMessage::CancelRun { run_id }</c>.
/// </summary>
public sealed record CancelRunMessage(
    [property: JsonPropertyName("run_id")] Guid RunId
) : ControlMessage;

/// <summary>
/// <c>{"type":"command", "command_id":..., "config_id":...?, "token":..., "verb":..., "args":{...}, "timeout_secs":...}</c>
/// — dispatch a typed command envelope. Mirrors Rust
/// <c>ControlMessage::Command(AgentCommand)</c> (flattened newtype). <c>token</c>
/// is an opaque short-lived JWT the agent validates; <c>args</c> is free-form
/// JSON.
/// </summary>
public sealed record CommandMessage(
    [property: JsonPropertyName("command_id")] Guid CommandId,
    [property: JsonPropertyName("config_id")] Guid? ConfigId,
    [property: JsonPropertyName("token")] string Token,
    [property: JsonPropertyName("verb")] string Verb,
    [property: JsonPropertyName("args")] JsonElement Args,
    [property: JsonPropertyName("timeout_secs")] long TimeoutSecs
) : ControlMessage;

/// <summary>
/// <c>{"type":"cancel", "command_id":...}</c> — cancel an in-flight command.
/// Mirrors Rust <c>ControlMessage::Cancel(AgentCommandCancel)</c> (flattened
/// newtype).
/// </summary>
public sealed record CancelMessage(
    [property: JsonPropertyName("command_id")] Guid CommandId
) : ControlMessage;

/// <summary>
/// <c>{"type":"heartbeat_ping", "now":...}</c> — dashboard-side liveness ping
/// carrying the server clock. Mirrors Rust
/// <c>ControlMessage::HeartbeatPing { now }</c>.
/// </summary>
public sealed record HeartbeatPingMessage(
    [property: JsonPropertyName("now")] DateTimeOffset Now
) : ControlMessage;

/// <summary>
/// <c>{"type":"shutdown"}</c> — server-initiated graceful drain. Mirrors Rust
/// unit variant <c>ControlMessage::Shutdown</c> (serialises to just the tag).
/// </summary>
public sealed record ShutdownMessage : ControlMessage;
