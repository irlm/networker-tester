using System.Text.Json;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Provisioning;
using Networker.ControlPlane.Realtime;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Tests for the regression data shapes + event emission ported from Rust
/// <c>regression.rs</c>. (The Rust module is a v0.28.0 stub with no comparison
/// logic; only the shapes + the <see cref="BenchmarkRegression"/> emission seam
/// are exercised.)
/// </summary>
public sealed class RegressionAnalyzerTests
{
    private static EventBus NewBus()
    {
        var services = new ServiceCollection();
        services.AddLogging(b => b.SetMinimumLevel(LogLevel.Warning));
        services.AddSignalR();
        services.AddDashboardEventBus();
        return services.BuildServiceProvider().GetRequiredService<EventBus>();
    }

    [Fact]
    public void EmitRegressionEvent_publishes_benchmark_regression()
    {
        var bus = NewBus();
        var configId = Guid.NewGuid();
        var regressions = new List<Regression>
        {
            new("dns", "p95_ms", 10.0, 15.0, 50.0),
            new("tls", "p95_ms", 20.0, 25.0, 25.0),
        };

        var seq = RegressionAnalyzer.EmitRegressionEvent(bus, configId, "my-config", regressions);

        Assert.Equal(1, seq);
        var replayed = bus.Replay(0);
        var evt = Assert.IsType<BenchmarkRegression>(Assert.Single(replayed).Event);
        Assert.Equal(configId, evt.ConfigId);
        Assert.Equal("my-config", evt.ConfigName);
        Assert.Equal(2, evt.RegressionCount);
        // regressions carried verbatim as a JSON array.
        Assert.Equal(JsonValueKind.Array, evt.Regressions.ValueKind);
        Assert.Equal(2, evt.Regressions.GetArrayLength());
    }

    [Fact]
    public void Regression_record_holds_fields()
    {
        var r = new Regression("http2", "throughput_mbps", 100.0, 80.0, -20.0);
        Assert.Equal("http2", r.Phase);
        Assert.Equal("throughput_mbps", r.Metric);
        Assert.Equal(100.0, r.BaselineValue);
        Assert.Equal(80.0, r.CurrentValue);
        Assert.Equal(-20.0, r.ChangePct);
    }
}
