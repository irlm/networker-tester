using Microsoft.Extensions.Options;

namespace Networker.Agent;

/// <summary>
/// Hosted background service. On startup it runs one probe against the
/// configured target, logs the parsed result, and reports it via the (stubbed)
/// dashboard client. In Phase 2 this loop becomes job-driven off SignalR.
/// </summary>
public sealed class AgentWorker(
    ILogger<AgentWorker> logger,
    ProbeRunner probeRunner,
    IDashboardClient dashboard,
    IOptions<AgentOptions> options,
    IHostApplicationLifetime lifetime) : BackgroundService
{
    private readonly AgentOptions _options = options.Value;

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        await dashboard.HeartbeatAsync(stoppingToken).ConfigureAwait(false);

        try
        {
            var result = await probeRunner.RunAsync(_options.Target, stoppingToken)
                .ConfigureAwait(false);

            var attempt = result.Attempts.Count > 0 ? result.Attempts[0] : null;
            logger.LogInformation(
                "Probe complete: schema_version={SchemaVersion} target={Target} " +
                "protocol={Protocol} dns_ms={DnsMs:F1} tls_ms={TlsMs:F1} ttfb_ms={TtfbMs:F1} total_ms={TotalMs:F1}",
                result.SchemaVersion,
                result.TargetUrl,
                attempt?.Protocol ?? "n/a",
                attempt?.Dns?.DurationMs ?? 0,
                attempt?.Tls?.HandshakeDurationMs ?? 0,
                attempt?.Http?.TtfbMs ?? 0,
                attempt?.Http?.TotalDurationMs ?? 0);

            await dashboard.ReportResultAsync(result, stoppingToken).ConfigureAwait(false);
        }
        catch (ProbeRunnerException ex)
        {
            logger.LogError(ex,
                "Probe failed. Is the '{Tester}' binary built and on PATH (or AGENT_TESTER_PATH set)?",
                _options.TesterPath);
        }
        catch (OperationCanceledException)
        {
            // shutting down
        }
        finally
        {
            // Skeleton: one-shot run, then stop the host.
            lifetime.StopApplication();
        }
    }
}
