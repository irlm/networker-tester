using System.Text.Json.Serialization;

namespace Networker.ControlPlane.Realtime;

/// <summary>
/// C# port of the Rust <c>networker_common::tester_messages::TesterMessage</c>
/// wire types (crates/networker-common/src/tester_messages.rs), used by the
/// project-scoped tester-queue hub mapped at <c>/ws/testers</c>.
///
/// The Rust hub is a raw axum WebSocket that exchanges JSON text frames of the
/// form <c>{ "type": "&lt;snake_case_variant&gt;", ... }</c> (serde
/// <c>#[serde(tag = "type", rename_all = "snake_case")]</c>). The React
/// frontend switches on that <c>type</c> discriminator. To keep the on-the-wire
/// body byte-compatible, every record here:
///   * carries an explicit <c>type</c> property whose default value is the
///     snake_case variant tag, and
///   * spells every field in snake_case (matching the serde field names).
///
/// Optional fields (running, position, eta_seconds, outcome, message) carry an
/// explicit <c>[JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]</c>
/// so <c>null</c> is omitted from the JSON exactly like the Rust
/// <c>skip_serializing_if = "Option::is_none"</c> attributes — no global
/// serializer options are required.
///
/// Under SignalR these objects are shipped as the single argument of a client
/// method (see <see cref="TesterQueueHub"/>): the frame envelope differs from
/// the raw-WS Rust build, but the payload object is identical, so the frontend's
/// <c>type</c>-switch is unchanged.
/// </summary>
public static class TesterQueueMessageTypes
{
    public const string SubscribeTesterQueue = "subscribe_tester_queue";
    public const string UnsubscribeTesterQueue = "unsubscribe_tester_queue";
    public const string TesterQueueSnapshot = "tester_queue_snapshot";
    public const string TesterQueueUpdate = "tester_queue_update";
    public const string PhaseUpdate = "phase_update";
}

/// <summary>
/// One benchmark currently running or waiting on a tester. Mirrors the Rust
/// <c>QueueEntry</c> struct: <c>position</c> and <c>eta_seconds</c> are optional
/// and omitted when null.
/// </summary>
public sealed record TesterQueueEntry(
    [property: JsonPropertyName("config_id")] string ConfigId,
    [property: JsonPropertyName("name")] string Name,
    [property: JsonPropertyName("position")]
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    uint? Position = null,
    [property: JsonPropertyName("eta_seconds")]
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    uint? EtaSeconds = null);

// ── Inbound (client → server) ────────────────────────────────────────────────

/// <summary>
/// Inbound <c>subscribe_tester_queue</c>. The client asks to receive queue
/// snapshots/updates for a set of testers within one project.
/// </summary>
public sealed record SubscribeTesterQueueMessage(
    [property: JsonPropertyName("project_id")] string ProjectId,
    [property: JsonPropertyName("tester_ids")] IReadOnlyList<string> TesterIds)
{
    [JsonPropertyName("type")]
    public string Type { get; init; } = TesterQueueMessageTypes.SubscribeTesterQueue;
}

/// <summary>Inbound <c>unsubscribe_tester_queue</c>.</summary>
public sealed record UnsubscribeTesterQueueMessage(
    [property: JsonPropertyName("tester_ids")] IReadOnlyList<string> TesterIds)
{
    [JsonPropertyName("type")]
    public string Type { get; init; } = TesterQueueMessageTypes.UnsubscribeTesterQueue;
}

// ── Outbound (server → client) ───────────────────────────────────────────────

/// <summary>
/// Outbound <c>tester_queue_snapshot</c> — the full current state sent right
/// after a successful subscribe. <c>running</c> is omitted when idle.
/// </summary>
public sealed record TesterQueueSnapshotMessage(
    [property: JsonPropertyName("project_id")] string ProjectId,
    [property: JsonPropertyName("tester_id")] string TesterId,
    [property: JsonPropertyName("seq")] ulong Seq,
    [property: JsonPropertyName("queued")] IReadOnlyList<TesterQueueEntry> Queued,
    [property: JsonPropertyName("running")]
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    TesterQueueEntry? Running = null)
{
    [JsonPropertyName("type")]
    public string Type { get; init; } = TesterQueueMessageTypes.TesterQueueSnapshot;
}

/// <summary>
/// Outbound <c>tester_queue_update</c> — a delta pushed when the tester's queue
/// changes. <c>trigger</c> names the cause (e.g. "benchmark_queued",
/// "benchmark_completed"). <c>running</c> omitted when idle.
/// </summary>
public sealed record TesterQueueUpdateMessage(
    [property: JsonPropertyName("project_id")] string ProjectId,
    [property: JsonPropertyName("tester_id")] string TesterId,
    [property: JsonPropertyName("seq")] ulong Seq,
    [property: JsonPropertyName("trigger")] string Trigger,
    [property: JsonPropertyName("queued")] IReadOnlyList<TesterQueueEntry> Queued,
    [property: JsonPropertyName("running")]
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    TesterQueueEntry? Running = null)
{
    [JsonPropertyName("type")]
    public string Type { get; init; } = TesterQueueMessageTypes.TesterQueueUpdate;
}

/// <summary>
/// Outbound <c>phase_update</c> — a lifecycle transition for an entity
/// (benchmark/tester). <c>outcome</c> and <c>message</c> are omitted when null.
/// Phase/Outcome serialize as the Rust lowercase strings.
/// </summary>
public sealed record PhaseUpdateMessage(
    [property: JsonPropertyName("project_id")] string ProjectId,
    [property: JsonPropertyName("entity_type")] string EntityType,
    [property: JsonPropertyName("entity_id")] string EntityId,
    [property: JsonPropertyName("seq")] ulong Seq,
    [property: JsonPropertyName("phase")] string Phase,
    [property: JsonPropertyName("applied_stages")] IReadOnlyList<string> AppliedStages,
    [property: JsonPropertyName("outcome")]
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    string? Outcome = null,
    [property: JsonPropertyName("message")]
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    string? Message = null)
{
    [JsonPropertyName("type")]
    public string Type { get; init; } = TesterQueueMessageTypes.PhaseUpdate;
}

/// <summary>
/// Lifecycle phase, mirroring the Rust <c>networker_common::phase::Phase</c>
/// enum. Serialized as the lowercase string form the wire protocol and DB use.
/// </summary>
public static class TesterPhase
{
    public const string Queued = "queued";
    public const string Starting = "starting";
    public const string Deploy = "deploy";
    public const string Running = "running";
    public const string Collect = "collect";
    public const string Done = "done";
}

/// <summary>
/// Terminal outcome, mirroring the Rust <c>networker_common::phase::Outcome</c>
/// enum (note <c>partial_success</c> is the serde-renamed form).
/// </summary>
public static class TesterOutcome
{
    public const string Success = "success";
    public const string PartialSuccess = "partial_success";
    public const string Failure = "failure";
    public const string Cancelled = "cancelled";
}
