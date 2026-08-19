namespace Networker.Monitoring.Endpoints;

public sealed record CreateMonitorRequest(
    string? Name,
    string? TargetUrl,
    string? Method,
    MonitorAssertions? Assertions,
    int? IntervalSeconds,
    int? TimeoutMs,
    string? Criticality,
    Guid? LocationId);

public sealed record UpdateMonitorRequest(
    string? Name,
    string? TargetUrl,
    string? Method,
    MonitorAssertions? Assertions,
    int? IntervalSeconds,
    int? TimeoutMs,
    bool? Enabled,
    string? Criticality,
    Guid? LocationId);

public sealed record MonitorView(
    Guid MonitorId,
    string ProjectId,
    string Name,
    string TargetUrl,
    string Method,
    MonitorAssertions Assertions,
    int IntervalSeconds,
    int TimeoutMs,
    bool Enabled,
    string Criticality,
    Guid LocationId,
    string? LatestOutcome,
    DateTimeOffset? LatestCheckAt,
    DateTimeOffset CreatedAt,
    DateTimeOffset UpdatedAt);

public sealed record MonitorCheckView(
    Guid CheckId,
    Guid MonitorId,
    Guid LocationId,
    DateTimeOffset ScheduledAt,
    DateTimeOffset StartedAt,
    DateTimeOffset FinishedAt,
    string Outcome,
    string? FailureKind,
    int? StatusCode,
    int? TtfbMs,
    int? TotalMs,
    object AssertionResults,
    string? ErrorSummary);

public sealed record MonitorLocationView(
    Guid LocationId,
    string Name,
    string? Provider,
    string? Region,
    DateTimeOffset? LastHeartbeatAt,
    bool Enabled);

public sealed record ListResponse<T>(IReadOnlyList<T> Items, string? NextCursor);
