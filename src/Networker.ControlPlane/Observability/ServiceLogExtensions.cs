using Microsoft.Extensions.Logging;
using Npgsql;

namespace Networker.ControlPlane.Observability;

/// <summary>
/// Wiring for the <c>service_log</c> sink.
///
/// <para>Registration is unconditional (the metrics object is always resolvable,
/// so <c>/api/logs/pipeline-status</c> has something to report), but the
/// <see cref="ILoggerProvider"/> is only attached when the sink is switched on.
/// A deployment that has not opted in pays nothing and behaves exactly as
/// before.</para>
/// </summary>
public static class ServiceLogExtensions
{
    public static IServiceCollection AddServiceLogSink(
        this IServiceCollection services, ServiceLogOptions options)
    {
        services.AddSingleton(options);
        services.AddSingleton<ServiceLogMetrics>();

        if (!options.Enabled)
        {
            return services;
        }

        services.AddSingleton(sp => new ServiceLogWriter(
            sp.GetRequiredService<NpgsqlDataSource>(),
            options,
            sp.GetRequiredService<ServiceLogMetrics>()));

        services.AddSingleton<ILoggerProvider>(sp => new ServiceLogProvider(
            sp.GetRequiredService<ServiceLogWriter>(), options));

        return services;
    }

    /// <summary>
    /// Create the table (if absent) and start the writer. Returns a
    /// human-readable outcome for the startup log.
    ///
    /// <para>Failure here is <b>not fatal</b>: log persistence is a diagnostic
    /// convenience, and a control plane that refuses to serve traffic because it
    /// could not create a logging table would be trading a real outage for a
    /// cosmetic one. The endpoint keeps answering
    /// <c>log_sink: "unconfigured"</c>, which is the truth.</para>
    /// </summary>
    public static async Task<string> StartServiceLogSinkAsync(
        this IServiceProvider services, CancellationToken ct = default)
    {
        var options = services.GetRequiredService<ServiceLogOptions>();
        if (!options.Enabled)
        {
            return $"disabled (set {ServiceLogOptions.EnabledEnvVar}=1 to persist control-plane logs to service_log)";
        }

        var dataSource = services.GetRequiredService<NpgsqlDataSource>();
        try
        {
            await ServiceLogSchema.EnsureTableAsync(dataSource, ct);
            var shape = await ServiceLogSchema.TryEnsureHypertableAsync(dataSource, options.RetentionDays, ct);
            services.GetRequiredService<ServiceLogWriter>().Start();
            return $"enabled as '{options.Service}' at {options.MinimumLevel} — {shape}";
        }
        catch (Exception ex) when (ex is not OperationCanceledException)
        {
            return $"FAILED to start ({ex.GetType().Name}: {ex.Message}) — logs stay on stdout/journald";
        }
    }
}
