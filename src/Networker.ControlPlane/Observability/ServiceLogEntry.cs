namespace Networker.ControlPlane.Observability;

/// <summary>One row bound for <c>service_log</c>.</summary>
/// <param name="Timestamp">When the log call happened — captured at enqueue, not
/// at flush, so a slow or backed-up sink cannot rewrite history.</param>
/// <param name="Service">Value the UI's service filter shows.</param>
/// <param name="Level">SMALLINT 1..5, Error=1 … Trace=5.</param>
/// <param name="Message">Rendered message; never null.</param>
/// <param name="ConfigId">Correlation, when the log scope carried one.</param>
/// <param name="ProjectId">CHAR(14) project id, when the scope carried one.</param>
/// <param name="TraceId">Correlation, when the scope carried one.</param>
/// <param name="Fields">Structured state as JSON, or null.</param>
public readonly record struct ServiceLogEntry(
    DateTime Timestamp,
    string Service,
    short Level,
    string Message,
    Guid? ConfigId,
    string? ProjectId,
    Guid? TraceId,
    string? Fields);
