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

/// <summary><c>{"type":"heartbeat","load":?,"version":?,"capabilities":{...}?}</c>.
/// <c>capabilities</c> (additive, v0.28.208+) is the runner's self-detected
/// tool inventory (<see cref="AgentCapabilities"/>) — omitted when null so an
/// older control plane sees the pre-0.28.208 shape byte-for-byte; a newer
/// control plane persists it on the agent row so pickers can gate the
/// <c>browser*</c> modes (need Chrome) and packet capture (needs tshark) on
/// the pinned runner.</summary>
public sealed record HeartbeatMessage(
    [property: JsonPropertyName("load")] double? Load,
    [property: JsonPropertyName("version")] string? Version,
    [property: JsonPropertyName("capabilities")]
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    AgentCapabilities? Capabilities = null,
    // Additive (v0.28.211+): the runner's host OS / CPU architecture
    // ("windows"|"linux"|"macos", "x86_64"|"aarch64"|…) — the same words the
    // `health` command verb reports (RunnerCapabilities.HostOs/HostArch).
    // Omitted when null so older control planes see the previous shape; a
    // newer control plane persists them on agent.os / agent.arch so a mixed
    // Linux + Windows runner pool is visible per runner (agents list,
    // lab.sh status).
    [property: JsonPropertyName("os")]
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    string? Os = null,
    [property: JsonPropertyName("arch")]
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    string? Arch = null
) : AgentMessage;

/// <summary>Runner tool inventory carried on the heartbeat:
/// <c>{"chrome":bool,"tshark":bool}</c>. <c>chrome</c> = a Chrome/Chromium
/// binary the tester's <c>find_chrome()</c> would locate (the <c>browser*</c>
/// modes need it); <c>tshark</c> = a tshark binary on PATH (packet capture
/// needs it). Detected once at startup by <see cref="RunnerCapabilities"/>.</summary>
public sealed record AgentCapabilities(
    [property: JsonPropertyName("chrome")] bool Chrome,
    [property: JsonPropertyName("tshark")] bool Tshark
);

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

/// <summary><c>{"type":"run_finished","run_id":...,"status":...,"artifact":{...}?,"envelope":{...}?}</c>.
/// <c>artifact</c> omitted when null (Rust <c>skip_serializing_if = Option::is_none</c>).
/// <c>envelope</c> (additive, v0.28.80+) carries the run-envelope fields
/// extracted from the tester's final TestRun JSON
/// (<see cref="RunExecutor.ExtractRunEnvelope"/>); omitted when the tester
/// emitted none. Version skew is safe in both directions: an old control
/// plane ignores the unknown member (System.Text.Json drops unmapped
/// properties), and an old agent simply never sends it (the server-side
/// column stays null).</summary>
public sealed record RunFinishedMessage(
    [property: JsonPropertyName("run_id")] Guid RunId,
    [property: JsonPropertyName("status")] string Status,
    [property: JsonPropertyName("artifact")]
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    BenchmarkArtifactPayload? Artifact,
    [property: JsonPropertyName("envelope")]
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    JsonElement? Envelope = null,
    // Authoritative end-of-run totals (additive, v0.28.214+). run_progress -
    // which the control plane used as its ONLY source for the counters - rides
    // the LOSSY fast path: when the outbound channel saturates every progress
    // frame is dropped and a finished run reports ok=0/fail=0 even though the
    // tester measured 22 successes (native Windows lab, 2026-08-17). The
    // terminal frame is delivered on the critical path, so carrying the totals
    // here makes the counters self-sufficient. Null from older agents.
    [property: JsonPropertyName("attempts_ok")]
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    uint? AttemptsOk = null,
    [property: JsonPropertyName("attempts_failed")]
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    uint? AttemptsFailed = null
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
