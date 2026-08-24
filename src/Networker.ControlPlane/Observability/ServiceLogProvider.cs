using System.Text.Json;
using Microsoft.Extensions.Logging;

namespace Networker.ControlPlane.Observability;

/// <summary>
/// The <see cref="ILoggerProvider"/> that feeds <c>service_log</c>.
///
/// <para>It captures the structured state the control plane already logs, so
/// the Logs tab's <c>config_id</c> / <c>project_id</c> / <c>trace_id</c>
/// filters actually select something: those columns are lifted out of the log
/// state and scopes by name rather than being left null, which is what makes
/// the filters more than decoration.</para>
/// </summary>
[ProviderAlias("ServiceLog")]
public sealed class ServiceLogProvider(
    ServiceLogWriter writer, ServiceLogOptions options) : ILoggerProvider, ISupportExternalScope
{
    private IExternalScopeProvider? _scopes;

    public ILogger CreateLogger(string categoryName) =>
        new ServiceLogLogger(categoryName, writer, options, () => _scopes);

    public void SetScopeProvider(IExternalScopeProvider scopeProvider) => _scopes = scopeProvider;

    public void Dispose() { }
}

internal sealed class ServiceLogLogger(
    string category,
    ServiceLogWriter writer,
    ServiceLogOptions options,
    Func<IExternalScopeProvider?> scopes) : ILogger
{
    // The sink's own diagnostics must not become rows in the sink.
    private static readonly string SelfCategory = typeof(ServiceLogWriter).Namespace!;

    public IDisposable? BeginScope<TState>(TState state) where TState : notnull =>
        scopes()?.Push(state);

    public bool IsEnabled(LogLevel logLevel) =>
        logLevel != LogLevel.None
        && logLevel >= options.MinimumLevel
        && !category.StartsWith(SelfCategory, StringComparison.Ordinal);

    public void Log<TState>(
        LogLevel logLevel,
        EventId eventId,
        TState state,
        Exception? exception,
        Func<TState, Exception?, string> formatter)
    {
        if (!IsEnabled(logLevel))
        {
            return;
        }

        try
        {
            var message = formatter(state, exception);
            if (exception is not null)
            {
                message = $"{message} | {exception.GetType().Name}: {exception.Message}";
            }

            var fields = new Dictionary<string, object?>(StringComparer.Ordinal)
            {
                ["category"] = category,
            };
            if (eventId.Id != 0 || !string.IsNullOrEmpty(eventId.Name))
            {
                fields["event"] = eventId.Name ?? eventId.Id.ToString();
            }
            if (exception is not null)
            {
                fields["exception"] = exception.ToString();
            }

            Guid? configId = null;
            string? projectId = null;
            Guid? traceId = null;

            Harvest(state, fields, ref configId, ref projectId, ref traceId);
            scopes()?.ForEachScope(
                (scope, _) => Harvest(scope, fields, ref configId, ref projectId, ref traceId),
                0);

            // System.Diagnostics correlation, when the request is traced and
            // nothing in the log state named a trace explicitly.
            traceId ??= ParseGuid(System.Diagnostics.Activity.Current?.TraceId.ToString());

            writer.TryEnqueue(new ServiceLogEntry(
                Timestamp: DateTime.UtcNow,
                Service: options.Service,
                Level: ServiceLogOptions.ToDbLevel(logLevel),
                Message: Truncate(message),
                ConfigId: configId,
                ProjectId: projectId,
                TraceId: traceId,
                Fields: Serialize(fields)));
        }
        catch
        {
            // A logging call may never throw into its caller. Losing one row is
            // strictly better than failing the request that produced it.
        }
    }

    /// <summary>Lift the correlation columns out of a log state or scope.
    /// Accepts both the C# ("ProjectId") and wire ("project_id") spellings,
    /// because both appear across this codebase's log calls.</summary>
    private static void Harvest(
        object? state,
        Dictionary<string, object?> fields,
        ref Guid? configId,
        ref string? projectId,
        ref Guid? traceId)
    {
        if (state is not IReadOnlyList<KeyValuePair<string, object?>> pairs)
        {
            return;
        }

        foreach (var (key, value) in pairs)
        {
            if (key == "{OriginalFormat}")
            {
                continue;
            }

            switch (key.ToLowerInvariant())
            {
                case "configid" or "config_id":
                    configId ??= ParseGuid(value?.ToString());
                    break;
                case "projectid" or "project_id":
                    projectId ??= NormalizeProjectId(value?.ToString());
                    break;
                case "traceid" or "trace_id":
                    traceId ??= ParseGuid(value?.ToString());
                    break;
            }

            fields.TryAdd(key, value?.ToString());
        }
    }

    private static Guid? ParseGuid(string? raw) =>
        Guid.TryParse(raw, out var g) ? g : null;

    /// <summary>`project_id` is CHAR(14); anything else would either be padded
    /// into a value that never matches a filter or blow up the insert.</summary>
    private static string? NormalizeProjectId(string? raw) =>
        string.IsNullOrWhiteSpace(raw) || raw.Trim().Length != 14 ? null : raw.Trim();

    /// <summary>Postgres would take a megabyte message; the Logs tab would not
    /// survive rendering it, and neither would the operator reading it.</summary>
    private static string Truncate(string message) =>
        string.IsNullOrEmpty(message) ? string.Empty
        : message.Length <= 8_000 ? message
        : message[..8_000] + "…[truncated]";

    private static string? Serialize(Dictionary<string, object?> fields)
    {
        try
        {
            return fields.Count == 0 ? null : JsonSerializer.Serialize(fields);
        }
        catch
        {
            return null;
        }
    }
}
