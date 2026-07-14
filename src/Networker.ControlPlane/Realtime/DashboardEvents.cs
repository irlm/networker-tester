using System.Text.Json;
using System.Text.Json.Serialization;

namespace Networker.ControlPlane.Realtime;

// ─────────────────────────────────────────────────────────────────────────────
// Browser event-bus wire types — C# mirror of the Rust `DashboardEvent` enum
// (crates/networker-common/src/messages.rs) and the `SeqEvent` wrapper
// (crates/networker-dashboard/src/services/event_bus.rs).
//
// WIRE CONTRACT (must match Rust byte-for-byte at the field level):
//   * Every event serialises FLAT as `{"type":"<snake_case>", ...fields}`.
//     Rust does this via `#[serde(tag = "type", rename_all = "snake_case")]`.
//     Here we use System.Text.Json polymorphism with
//     `TypeDiscriminatorPropertyName = "type"`, which writes the discriminator
//     as an inline sibling of the payload fields (NOT nested) — identical shape.
//   * The `SeqEvent` wrapper prepends `seq`, producing
//     `{"seq":123,"type":"job_update", ...fields}`. Rust achieves this with
//     `#[serde(flatten)]`; we use a dedicated converter (below) that writes
//     `seq` first and then inlines the event's own properties.
//   * All field names are snake_case. Records below use explicit
//     [JsonPropertyName] so the shape is pinned regardless of the ambient
//     JsonSerializerOptions the host happens to configure.
//
// Opaque nested payloads (`attempt`, `payload`, `regressions`) are carried as
// JsonElement — the control plane forwards whatever the agent/DB produced
// (e.g. a serialized RequestAttempt) without re-modelling it here. That keeps
// this bus decoupled from the probe-result schema, exactly like the Rust side
// which boxes `RequestAttempt` / uses `serde_json::Value`.
// ─────────────────────────────────────────────────────────────────────────────

/// <summary>
/// A dashboard live-update event. Polymorphic base: each concrete record
/// serialises flat with a leading <c>"type"</c> discriminator, matching the
/// Rust <c>DashboardEvent</c> enum's external-tag JSON.
/// </summary>
[JsonPolymorphic(TypeDiscriminatorPropertyName = "type")]
[JsonDerivedType(typeof(JobUpdate), "job_update")]
[JsonDerivedType(typeof(AttemptResult), "attempt_result")]
[JsonDerivedType(typeof(JobComplete), "job_complete")]
[JsonDerivedType(typeof(AgentStatus), "agent_status")]
[JsonDerivedType(typeof(JobLog), "job_log")]
[JsonDerivedType(typeof(DeployLog), "deploy_log")]
[JsonDerivedType(typeof(DeployComplete), "deploy_complete")]
[JsonDerivedType(typeof(BenchmarkUpdate), "benchmark_update")]
[JsonDerivedType(typeof(BenchmarkRegression), "benchmark_regression")]
public abstract record DashboardEvent;

/// <summary>
/// <c>{"type":"job_update", "job_id":..., "status":..., "agent_id":..., "started_at":..., "finished_at":...}</c>
/// Mirrors Rust <c>DashboardEvent::JobUpdate</c>.
/// </summary>
public sealed record JobUpdate(
    [property: JsonPropertyName("job_id")] Guid JobId,
    [property: JsonPropertyName("status")] string Status,
    [property: JsonPropertyName("agent_id")] Guid? AgentId,
    [property: JsonPropertyName("started_at")] DateTimeOffset? StartedAt,
    [property: JsonPropertyName("finished_at")] DateTimeOffset? FinishedAt
) : DashboardEvent;

/// <summary>
/// <c>{"type":"attempt_result", "job_id":..., "attempt":{...}}</c>
/// Mirrors Rust <c>DashboardEvent::AttemptResult</c>. <c>attempt</c> is the
/// opaque serialized RequestAttempt forwarded verbatim.
/// </summary>
public sealed record AttemptResult(
    [property: JsonPropertyName("job_id")] Guid JobId,
    [property: JsonPropertyName("attempt")] JsonElement Attempt
) : DashboardEvent;

/// <summary>
/// <c>{"type":"job_complete", "job_id":..., "run_id":..., "success_count":..., "failure_count":...}</c>
/// Mirrors Rust <c>DashboardEvent::JobComplete</c>.
/// </summary>
public sealed record JobComplete(
    [property: JsonPropertyName("job_id")] Guid JobId,
    [property: JsonPropertyName("run_id")] Guid RunId,
    [property: JsonPropertyName("success_count")] long SuccessCount,
    [property: JsonPropertyName("failure_count")] long FailureCount
) : DashboardEvent;

/// <summary>
/// <c>{"type":"agent_status", "agent_id":..., "status":..., "last_heartbeat":...}</c>
/// Mirrors Rust <c>DashboardEvent::AgentStatus</c>.
/// </summary>
public sealed record AgentStatus(
    [property: JsonPropertyName("agent_id")] Guid AgentId,
    [property: JsonPropertyName("status")] string Status,
    [property: JsonPropertyName("last_heartbeat")] DateTimeOffset? LastHeartbeat
) : DashboardEvent;

/// <summary>
/// <c>{"type":"job_log", "job_id":..., "line":..., "level":...}</c>
/// Mirrors Rust <c>DashboardEvent::JobLog</c>.
/// </summary>
public sealed record JobLog(
    [property: JsonPropertyName("job_id")] Guid JobId,
    [property: JsonPropertyName("line")] string Line,
    [property: JsonPropertyName("level")] string Level
) : DashboardEvent;

/// <summary>
/// <c>{"type":"deploy_log", "deployment_id":..., "line":..., "stream":...}</c>
/// Mirrors Rust <c>DashboardEvent::DeployLog</c>.
/// </summary>
public sealed record DeployLog(
    [property: JsonPropertyName("deployment_id")] Guid DeploymentId,
    [property: JsonPropertyName("line")] string Line,
    [property: JsonPropertyName("stream")] string Stream
) : DashboardEvent;

/// <summary>
/// <c>{"type":"deploy_complete", "deployment_id":..., "status":..., "endpoint_ips":[...]}</c>
/// Mirrors Rust <c>DashboardEvent::DeployComplete</c>.
/// </summary>
public sealed record DeployComplete(
    [property: JsonPropertyName("deployment_id")] Guid DeploymentId,
    [property: JsonPropertyName("status")] string Status,
    [property: JsonPropertyName("endpoint_ips")] IReadOnlyList<string> EndpointIps
) : DashboardEvent;

/// <summary>
/// <c>{"type":"benchmark_update", "config_id":..., "event_type":..., "payload":{...}}</c>
/// Mirrors Rust <c>DashboardEvent::BenchmarkUpdate</c>. <c>payload</c> is
/// free-form JSON forwarded verbatim.
/// </summary>
public sealed record BenchmarkUpdate(
    [property: JsonPropertyName("config_id")] Guid ConfigId,
    [property: JsonPropertyName("event_type")] string EventType,
    [property: JsonPropertyName("payload")] JsonElement Payload
) : DashboardEvent;

/// <summary>
/// <c>{"type":"benchmark_regression", "config_id":..., "config_name":..., "regression_count":..., "regressions":{...}}</c>
/// Mirrors Rust <c>DashboardEvent::BenchmarkRegression</c>. <c>regressions</c>
/// is free-form JSON forwarded verbatim.
/// </summary>
public sealed record BenchmarkRegression(
    [property: JsonPropertyName("config_id")] Guid ConfigId,
    [property: JsonPropertyName("config_name")] string ConfigName,
    [property: JsonPropertyName("regression_count")] long RegressionCount,
    [property: JsonPropertyName("regressions")] JsonElement Regressions
) : DashboardEvent;

/// <summary>
/// A <see cref="DashboardEvent"/> tagged with a monotonic sequence number.
/// Serialises FLAT as <c>{"seq":N, "type":"...", ...event-fields}</c> — the
/// exact shape the Rust <c>SeqEvent</c> (with <c>#[serde(flatten)]</c>) emits.
/// Browsers key off <c>seq</c> to request replay on reconnect (<c>?since=</c>).
/// </summary>
[JsonConverter(typeof(SeqEventJsonConverter))]
public sealed record SeqEvent(long Seq, DashboardEvent Event);

/// <summary>
/// Writes <see cref="SeqEvent"/> as a single flat object: emits <c>seq</c>
/// first, then inlines every property of the wrapped event (including its
/// polymorphic <c>type</c> discriminator). This reproduces Rust's
/// <c>#[serde(flatten)]</c> over the <c>SeqEvent { seq, event }</c> struct.
///
/// Deserialization is not required for the outbound browser feed and is left
/// unimplemented (the browser is a pure consumer of this shape).
/// </summary>
public sealed class SeqEventJsonConverter : JsonConverter<SeqEvent>
{
    public override SeqEvent Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
        => throw new NotSupportedException(
            "SeqEvent is an outbound-only wire type; deserialization is not supported.");

    public override void Write(Utf8JsonWriter writer, SeqEvent value, JsonSerializerOptions options)
    {
        writer.WriteStartObject();
        writer.WriteNumber("seq", value.Seq);

        // Serialize the polymorphic event to a temporary document, then splice
        // its properties in alongside `seq`. STJ writes the event as
        // {"type":"...", ...fields}; we lift those into this same object so the
        // final shape is flat: {"seq":N,"type":"...", ...fields}.
        // Serialize against the runtime (base) type so the [JsonPolymorphic]
        // discriminator is emitted.
        var eventJson = JsonSerializer.SerializeToUtf8Bytes<DashboardEvent>(value.Event, options);
        using var doc = JsonDocument.Parse(eventJson);
        foreach (var prop in doc.RootElement.EnumerateObject())
        {
            prop.WriteTo(writer);
        }

        writer.WriteEndObject();
    }
}
