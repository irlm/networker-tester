using System.Text.Json;
using Microsoft.AspNetCore.SignalR;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Microsoft.Extensions.Logging.Abstractions;
using Networker.ControlPlane.Realtime;
using Networker.ControlPlane.Realtime.RawWs;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Bridge-level tests: the two fan-out paths that feed raw WebSockets.
///
/// 1. Browser feed — <see cref="EventBus.Publish"/> must deliver each event to
///    registered raw connections as the flat <c>{"seq":N,"type":...}</c> frame.
/// 2. Tester feed — <see cref="RawWsTesterQueueLifetimeManager"/> must mirror
///    SignalR <c>tq:*</c> group sends into <see cref="RawSocketRegistry"/> as
///    bare type-tagged JSON, and <c>AddRawWebSockets</c> must actually win the
///    <see cref="HubLifetimeManager{THub}"/> registration for TesterQueueHub.
/// </summary>
public sealed class RawWsBridgeTests
{
    private static ServiceProvider BuildProvider()
    {
        var services = new ServiceCollection();
        services.AddLogging(b => b.SetMinimumLevel(LogLevel.None));
        services.AddSignalR();
        services.AddDashboardEventBus();
        services.AddRawWebSockets();
        return services.BuildServiceProvider();
    }

    [Fact]
    public void AddRawWebSockets_ReplacesTesterQueueHubLifetimeManager()
    {
        using var provider = BuildProvider();

        var manager = provider.GetRequiredService<HubLifetimeManager<TesterQueueHub>>();
        Assert.IsType<RawWsTesterQueueLifetimeManager>(manager);

        // Other hubs keep SignalR's default manager — the decorator is scoped to
        // the tester-queue hub only.
        var browserManager = provider.GetRequiredService<HubLifetimeManager<BrowserHub>>();
        Assert.IsType<DefaultHubLifetimeManager<BrowserHub>>(browserManager);
    }

    [Fact]
    public async Task EventBusPublish_FansOutToRawBrowserSockets()
    {
        using var provider = BuildProvider();
        var bus = provider.GetRequiredService<EventBus>();
        var registry = provider.GetRequiredService<RawSocketRegistry>();

        var sent = new List<string>();
        var conn = new RawSocketConnection(
            "browser-1",
            (json, _) =>
            {
                lock (sent) { sent.Add(json); }
                return Task.CompletedTask;
            });
        registry.RegisterBrowser(conn);

        var jobId = Guid.NewGuid();
        var seq = bus.Publish(new JobLog(jobId, "probe started", "info"));

        conn.CompleteQueue();
        await conn.RunSendPumpAsync(CancellationToken.None);

        var frame = Assert.Single(sent);
        using var doc = JsonDocument.Parse(frame);
        Assert.Equal(seq, doc.RootElement.GetProperty("seq").GetInt64());
        Assert.Equal("job_log", doc.RootElement.GetProperty("type").GetString());
        Assert.Equal(jobId.ToString(), doc.RootElement.GetProperty("job_id").GetString());
        Assert.Equal("probe started", doc.RootElement.GetProperty("line").GetString());
    }

    [Fact]
    public async Task EventBusPublish_UnregisteredConnection_ReceivesNothing()
    {
        using var provider = BuildProvider();
        var bus = provider.GetRequiredService<EventBus>();
        var registry = provider.GetRequiredService<RawSocketRegistry>();

        var sent = new List<string>();
        var conn = new RawSocketConnection(
            "browser-1",
            (json, _) => { lock (sent) { sent.Add(json); } return Task.CompletedTask; });
        registry.RegisterBrowser(conn);
        registry.UnregisterBrowser(conn);

        bus.Publish(new JobLog(Guid.NewGuid(), "after disconnect", "info"));

        conn.CompleteQueue();
        await conn.RunSendPumpAsync(CancellationToken.None);
        Assert.Empty(sent);
    }

    [Fact]
    public async Task LifetimeManager_MirrorsTesterGroupSends_AsBareJson()
    {
        var rawRegistry = new RawSocketRegistry(NullLogger<RawSocketRegistry>.Instance);
        var manager = new RawWsTesterQueueLifetimeManager(
            new DefaultHubLifetimeManager<TesterQueueHub>(
                NullLogger<DefaultHubLifetimeManager<TesterQueueHub>>.Instance),
            rawRegistry);

        var sent = new List<string>();
        var conn = new RawSocketConnection(
            "tester-1",
            (json, _) => { lock (sent) { sent.Add(json); } return Task.CompletedTask; });

        var group = TesterQueueRegistry.GroupName("proj-1", "tester-a");
        rawRegistry.SubscribeTesterGroup(group, conn);

        var update = new TesterQueueUpdateMessage(
            "proj-1", "tester-a", 4, "benchmark_queued",
            new[] { new TesterQueueEntry("cfg-7", "smoke", Position: 1) });

        // Exactly what TesterQueueBroadcaster does via IHubContext.
        await manager.SendGroupAsync(group, TesterQueueHub.ClientMethod, new object?[] { update });

        conn.CompleteQueue();
        await conn.RunSendPumpAsync(CancellationToken.None);

        var frame = Assert.Single(sent);
        using var doc = JsonDocument.Parse(frame);
        Assert.Equal("tester_queue_update", doc.RootElement.GetProperty("type").GetString());
        Assert.Equal("benchmark_queued", doc.RootElement.GetProperty("trigger").GetString());
        Assert.Equal(4ul, doc.RootElement.GetProperty("seq").GetUInt64());
        // Bare payload — no SignalR envelope fields.
        Assert.False(doc.RootElement.TryGetProperty("target", out _));
        Assert.False(doc.RootElement.TryGetProperty("arguments", out _));
    }

    [Fact]
    public async Task LifetimeManager_IgnoresNonTesterGroups()
    {
        var rawRegistry = new RawSocketRegistry(NullLogger<RawSocketRegistry>.Instance);
        var manager = new RawWsTesterQueueLifetimeManager(
            new DefaultHubLifetimeManager<TesterQueueHub>(
                NullLogger<DefaultHubLifetimeManager<TesterQueueHub>>.Instance),
            rawRegistry);

        var sent = new List<string>();
        var conn = new RawSocketConnection(
            "c", (json, _) => { lock (sent) { sent.Add(json); } return Task.CompletedTask; });
        // Subscribed under a tq group, but the send targets an unrelated group.
        rawRegistry.SubscribeTesterGroup("tq:p:t", conn);

        await manager.SendGroupAsync("some-other-group", "Method", new object?[] { new { x = 1 } });

        conn.CompleteQueue();
        await conn.RunSendPumpAsync(CancellationToken.None);
        Assert.Empty(sent);
    }
}
