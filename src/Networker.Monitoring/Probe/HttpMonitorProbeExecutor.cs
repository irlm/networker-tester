using System.Diagnostics;
using System.Text.Json;
using Networker.Monitoring.Data.Entities;
using MonitorEntity = Networker.Monitoring.Data.Entities.Monitor;

namespace Networker.Monitoring.Probe;

public sealed class HttpMonitorProbeExecutor(
    IHttpClientFactory httpClientFactory,
    TimeProvider timeProvider) : IMonitorProbeExecutor
{
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);

    public async Task<MonitorCheck> ExecuteAsync(
        MonitorEntity monitor,
        Guid locationId,
        Guid checkId,
        DateTimeOffset scheduledAt,
        CancellationToken cancellationToken)
    {
        var startedAt = timeProvider.GetUtcNow();
        var stopwatch = Stopwatch.StartNew();
        var assertions = JsonSerializer.Deserialize<MonitorAssertions>(monitor.AssertionConfig, JsonOptions)
            ?? new MonitorAssertions();

        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeout.CancelAfter(TimeSpan.FromMilliseconds(monitor.TimeoutMs));

        try
        {
            using var request = new HttpRequestMessage(new HttpMethod(monitor.Method), monitor.TargetUrl);
            request.Headers.TryAddWithoutValidation("X-LagHound-Check-Id", checkId.ToString());
            using var response = await httpClientFactory.CreateClient("monitor-probe")
                .SendAsync(request, HttpCompletionOption.ResponseHeadersRead, timeout.Token);
            stopwatch.Stop();

            var totalMs = ToBoundedMilliseconds(stopwatch.Elapsed);
            var statusCode = (int)response.StatusCode;
            var statusOk = statusCode >= assertions.StatusMin && statusCode <= assertions.StatusMax;
            var outcome = "healthy";
            string? failureKind = null;

            if (!statusOk)
            {
                outcome = "critical";
                failureKind = "status";
            }
            else if (assertions.CriticalLatencyMs is int criticalMs && totalMs > criticalMs)
            {
                outcome = "critical";
                failureKind = "latency";
            }
            else if (assertions.WarningLatencyMs is int warningMs && totalMs > warningMs)
            {
                outcome = "warning";
                failureKind = "latency";
            }

            return BuildCheck(
                monitor, locationId, checkId, scheduledAt, startedAt, outcome,
                failureKind, statusCode, totalMs,
                JsonSerializer.Serialize(new
                {
                    status = new
                    {
                        passed = statusOk,
                        expected_min = assertions.StatusMin,
                        expected_max = assertions.StatusMax,
                        actual = statusCode,
                    },
                    latency = new
                    {
                        warning_ms = assertions.WarningLatencyMs,
                        critical_ms = assertions.CriticalLatencyMs,
                        actual_ms = totalMs,
                    },
                }, JsonOptions),
                errorSummary: null);
        }
        catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested)
        {
            stopwatch.Stop();
            return BuildCheck(
                monitor, locationId, checkId, scheduledAt, startedAt,
                "critical", "timeout", null, ToBoundedMilliseconds(stopwatch.Elapsed),
                "{\"request\":{\"passed\":false,\"reason\":\"timeout\"}}",
                $"Request exceeded the {monitor.TimeoutMs} ms timeout.");
        }
        catch (HttpRequestException exception)
        {
            stopwatch.Stop();
            return BuildCheck(
                monitor, locationId, checkId, scheduledAt, startedAt,
                "critical", "connect", null, ToBoundedMilliseconds(stopwatch.Elapsed),
                "{\"request\":{\"passed\":false,\"reason\":\"connect\"}}",
                BoundError(exception.Message));
        }
    }

    private MonitorCheck BuildCheck(
        MonitorEntity monitor,
        Guid locationId,
        Guid checkId,
        DateTimeOffset scheduledAt,
        DateTimeOffset startedAt,
        string outcome,
        string? failureKind,
        int? statusCode,
        int totalMs,
        string assertionResults,
        string? errorSummary)
    {
        var finishedAt = timeProvider.GetUtcNow();
        return new MonitorCheck
        {
            CheckId = checkId,
            MonitorId = monitor.MonitorId,
            LocationId = locationId,
            ScheduledAt = scheduledAt,
            StartedAt = startedAt,
            FinishedAt = finishedAt,
            Outcome = outcome,
            FailureKind = failureKind,
            StatusCode = statusCode,
            TotalMs = totalMs,
            AssertionResults = assertionResults,
            ErrorSummary = errorSummary,
            CreatedAt = finishedAt,
        };
    }

    private static int ToBoundedMilliseconds(TimeSpan elapsed) =>
        (int)Math.Min(int.MaxValue, Math.Max(0, Math.Round(elapsed.TotalMilliseconds)));

    private static string BoundError(string value) =>
        value.Length <= 1000 ? value : value[..1000];
}
