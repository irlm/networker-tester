namespace Networker.ControlPlane.Observability;

/// <summary>
/// Configuration for the <c>service_log</c> sink — the half of log persistence
/// the Rust→C# migration never ported.
///
/// <para>The Logs tab reads <c>service_log</c>, a table the Rust
/// <c>networker-log</c> crate creates and writes. Only the tester and the
/// endpoint still depend on that crate, so on a C#-only control plane nothing
/// ever wrote the table: <c>/api/logs</c> answered
/// <c>log_sink: "unconfigured"</c> and the UI honestly said log persistence was
/// not configured. This is the writer.</para>
///
/// <para><b>Opt-in on purpose.</b> Turning it on makes every qualifying log
/// line a database write; that is a real cost and a real disk-growth decision
/// for an existing deployment, so it is never enabled implicitly. Set
/// <c>DASHBOARD_LOG_SINK=1</c> and the control plane creates the table (same
/// DDL as the Rust crate) and starts writing.</para>
/// </summary>
public sealed record ServiceLogOptions
{
    public const string EnabledEnvVar = "DASHBOARD_LOG_SINK";
    public const string LevelEnvVar = "DASHBOARD_LOG_SINK_LEVEL";
    public const string ServiceEnvVar = "DASHBOARD_LOG_SINK_SERVICE";
    public const string QueueEnvVar = "DASHBOARD_LOG_SINK_QUEUE";
    public const string BatchEnvVar = "DASHBOARD_LOG_SINK_BATCH";
    public const string IntervalEnvVar = "DASHBOARD_LOG_SINK_INTERVAL_MS";
    public const string RetentionEnvVar = "DASHBOARD_LOG_SINK_RETENTION_DAYS";

    /// <summary>The value the UI's service filter shows for these rows.
    /// `dashboard` is what the Rust control plane used and what
    /// SystemDashboardPage still lists first, so rows keep landing under the
    /// name operators already filter by.</summary>
    public const string DefaultService = "dashboard";

    public bool Enabled { get; init; }

    /// <summary>Floor for what reaches the database. Debug/Trace on a busy
    /// control plane is a firehose nobody reads and everybody pays for, so the
    /// default floor is Information.</summary>
    public LogLevel MinimumLevel { get; init; } = LogLevel.Information;

    public string Service { get; init; } = DefaultService;

    /// <summary>Bounded: logging must never block a request or grow without
    /// limit when the database is slow or gone. Overflow is dropped and
    /// counted, never awaited.</summary>
    public int QueueCapacity { get; init; } = 10_000;

    public int BatchSize { get; init; } = 200;

    public TimeSpan FlushInterval { get; init; } = TimeSpan.FromSeconds(2);

    /// <summary>Days of history to keep when TimescaleDB is present (a
    /// retention policy on the hypertable). 0 disables the policy. Mirrors the
    /// Rust crate's 7-day default.</summary>
    public int RetentionDays { get; init; } = 7;

    /// <summary>Read the options from the environment, falling back to the
    /// defaults above. Never throws: a malformed value logs nothing and keeps
    /// the default, because a typo in an env var must not stop the process.</summary>
    public static ServiceLogOptions FromEnvironment(IConfiguration config)
    {
        string? Get(string key) =>
            config[key] ?? Environment.GetEnvironmentVariable(key);

        var enabled = ParseBool(Get(EnabledEnvVar));
        return new ServiceLogOptions
        {
            Enabled = enabled,
            MinimumLevel = ParseLevel(Get(LevelEnvVar)) ?? LogLevel.Information,
            Service = string.IsNullOrWhiteSpace(Get(ServiceEnvVar)) ? DefaultService : Get(ServiceEnvVar)!.Trim(),
            QueueCapacity = ParseInt(Get(QueueEnvVar), 10_000, min: 100, max: 1_000_000),
            BatchSize = ParseInt(Get(BatchEnvVar), 200, min: 1, max: 10_000),
            FlushInterval = TimeSpan.FromMilliseconds(ParseInt(Get(IntervalEnvVar), 2_000, min: 100, max: 60_000)),
            RetentionDays = ParseInt(Get(RetentionEnvVar), 7, min: 0, max: 3_650),
        };
    }

    internal static bool ParseBool(string? raw) =>
        raw?.Trim().ToLowerInvariant() is "1" or "true" or "yes" or "on";

    internal static LogLevel? ParseLevel(string? raw) =>
        raw?.Trim().ToUpperInvariant() switch
        {
            null or "" => null,
            "TRACE" or "TRC" => LogLevel.Trace,
            "DEBUG" or "DBG" => LogLevel.Debug,
            "INFO" or "INFORMATION" or "INF" => LogLevel.Information,
            "WARN" or "WARNING" or "WRN" => LogLevel.Warning,
            "ERROR" or "ERR" => LogLevel.Error,
            "CRITICAL" or "FATAL" => LogLevel.Critical,
            _ => null,
        };

    internal static int ParseInt(string? raw, int fallback, int min, int max) =>
        int.TryParse(raw?.Trim(), out var v) ? Math.Clamp(v, min, max) : fallback;

    /// <summary>
    /// The <c>service_log.level</c> SMALLINT for a .NET level, matching
    /// <c>networker_log::Level::as_db</c> exactly: Error=1 … Trace=5.
    /// Critical folds into Error (1) — the table has no sixth severity, and the
    /// read side's level filter only understands 1..5.
    /// </summary>
    public static short ToDbLevel(LogLevel level) => level switch
    {
        LogLevel.Critical or LogLevel.Error => 1,
        LogLevel.Warning => 2,
        LogLevel.Information => 3,
        LogLevel.Debug => 4,
        _ => 5,
    };
}
