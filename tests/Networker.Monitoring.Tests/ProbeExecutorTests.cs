using System.Net;
using System.Text.Json;
using Networker.Monitoring.Data.Entities;
using Networker.Monitoring.Probe;
using MonitorEntity = Networker.Monitoring.Data.Entities.Monitor;

namespace Networker.Monitoring.Tests;

public sealed class ProbeExecutorTests
{
    [Fact]
    public async Task Successful_status_and_latency_are_healthy()
    {
        var executor = NewExecutor(_ => new HttpResponseMessage(HttpStatusCode.NoContent));
        var monitor = NewMonitor(new MonitorAssertions { StatusMin = 200, StatusMax = 299 });

        var check = await executor.ExecuteAsync(
            monitor, Guid.NewGuid(), Guid.NewGuid(), DateTimeOffset.UtcNow, CancellationToken.None);

        Assert.Equal("healthy", check.Outcome);
        Assert.Null(check.FailureKind);
        Assert.Equal(204, check.StatusCode);
        Assert.NotNull(check.TotalMs);
    }

    [Fact]
    public async Task Unexpected_status_is_critical_with_evidence()
    {
        var executor = NewExecutor(_ => new HttpResponseMessage(HttpStatusCode.ServiceUnavailable));
        var monitor = NewMonitor(new MonitorAssertions { StatusMin = 200, StatusMax = 399 });

        var check = await executor.ExecuteAsync(
            monitor, Guid.NewGuid(), Guid.NewGuid(), DateTimeOffset.UtcNow, CancellationToken.None);

        Assert.Equal("critical", check.Outcome);
        Assert.Equal("status", check.FailureKind);
        Assert.Equal(503, check.StatusCode);
        Assert.Contains("\"passed\":false", check.AssertionResults);
    }

    [Fact]
    public async Task Timeout_is_target_critical_not_runner_unknown()
    {
        var executor = NewExecutor(async (_, cancellationToken) =>
        {
            await Task.Delay(Timeout.InfiniteTimeSpan, cancellationToken);
            return new HttpResponseMessage(HttpStatusCode.OK);
        });
        var monitor = NewMonitor(new MonitorAssertions());
        monitor.TimeoutMs = 100;

        var check = await executor.ExecuteAsync(
            monitor, Guid.NewGuid(), Guid.NewGuid(), DateTimeOffset.UtcNow, CancellationToken.None);

        Assert.Equal("critical", check.Outcome);
        Assert.Equal("timeout", check.FailureKind);
    }

    private static HttpMonitorProbeExecutor NewExecutor(
        Func<HttpRequestMessage, HttpResponseMessage> response) =>
        NewExecutor((request, _) => Task.FromResult(response(request)));

    private static HttpMonitorProbeExecutor NewExecutor(
        Func<HttpRequestMessage, CancellationToken, Task<HttpResponseMessage>> response) =>
        new(new SingleClientFactory(new HttpClient(new StubHandler(response))), TimeProvider.System);

    private static MonitorEntity NewMonitor(MonitorAssertions assertions) => new()
    {
        MonitorId = Guid.NewGuid(),
        ProjectId = "probe-tests",
        Name = "probe",
        TargetUrl = "https://example.test/health",
        Method = "GET",
        AssertionConfig = JsonSerializer.Serialize(assertions),
        TimeoutMs = 1000,
    };

    private sealed class SingleClientFactory(HttpClient client) : IHttpClientFactory
    {
        public HttpClient CreateClient(string name) => client;
    }

    private sealed class StubHandler(
        Func<HttpRequestMessage, CancellationToken, Task<HttpResponseMessage>> response) : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken) => response(request, cancellationToken);
    }
}
