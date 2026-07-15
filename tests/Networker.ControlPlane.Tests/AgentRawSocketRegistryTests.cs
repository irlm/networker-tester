using System.Text.Json;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Realtime;

namespace Networker.ControlPlane.Tests;

/// Dual-transport tests for the M6 registry refactor: a connection registered
/// with an explicit sender delegate (the raw /ws/agent WebSocket path) must
/// receive the serialized `{"type":"...", ...}` ControlMessage JSON — the
/// byte-identical frame the Rust hub writes — through the whole public
/// outbound API the M3 dispatcher and M5 agent-commands call. The SignalR
/// two-argument Register overload is covered by AgentConnectionRegistryTests
/// (which must keep passing unmodified).
public sealed class AgentRawSocketRegistryTests
{
    private static AgentConnectionRegistry NewRegistry()
    {
        var services = new ServiceCollection();
        services.AddLogging(b => b.SetMinimumLevel(LogLevel.Warning));
        services.AddSignalR();
        services.AddAgentProtocol();
        return services.BuildServiceProvider().GetRequiredService<AgentConnectionRegistry>();
    }

    private static (List<string> Sent, Func<string, CancellationToken, Task> Sender) CapturingSender()
    {
        var sent = new List<string>();
        return (sent, (json, _) =>
        {
            lock (sent)
            {
                sent.Add(json);
            }
            return Task.CompletedTask;
        });
    }

    private static JsonElement Json(string raw)
    {
        using var doc = JsonDocument.Parse(raw);
        return doc.RootElement.Clone();
    }

    [Fact]
    public void Raw_sender_registration_makes_agent_online()
    {
        var reg = NewRegistry();
        var agent = Guid.NewGuid();
        var (_, sender) = CapturingSender();

        reg.Register(agent, "raw-conn-1", sender);

        Assert.True(reg.IsOnline(agent));
        Assert.Contains(agent, reg.OnlineAgents());
        Assert.Equal(agent, reg.AnyOnlineAgent());
    }

    [Fact]
    public async Task AssignRun_delivers_serialized_control_message_to_raw_sender()
    {
        var reg = NewRegistry();
        var agent = Guid.NewGuid();
        var (sent, sender) = CapturingSender();
        reg.Register(agent, "raw-conn-1", sender);

        var ok = await reg.AssignRunAsync(
            agent,
            run: Json("""{"id":"11111111-1111-1111-1111-111111111111","status":"queued"}"""),
            config: Json("""{"mode":"http2","url":"https://example.test/health"}"""));

        Assert.True(ok);
        var frame = Assert.Single(sent);

        // The raw sender receives the exact WS text frame the Rust agent
        // decodes: flat envelope, "type" discriminator, snake_case payloads.
        using var doc = JsonDocument.Parse(frame);
        Assert.Equal("assign_run", doc.RootElement.GetProperty("type").GetString());
        Assert.Equal("queued", doc.RootElement.GetProperty("run").GetProperty("status").GetString());
        Assert.Equal("http2", doc.RootElement.GetProperty("config").GetProperty("mode").GetString());
    }

    [Fact]
    public async Task Every_outbound_api_call_reaches_the_raw_sender_with_the_right_type_tag()
    {
        var reg = NewRegistry();
        var agent = Guid.NewGuid();
        var runId = Guid.NewGuid();
        var commandId = Guid.NewGuid();
        var (sent, sender) = CapturingSender();
        reg.Register(agent, "raw-conn-1", sender);

        Assert.True(await reg.CancelRunAsync(agent, runId));
        Assert.True(await reg.SendCommandAsync(agent, new CommandMessage(
            commandId, null, "jwt-token", "restart_service", Json("""{"name":"nginx"}"""), 30)));
        Assert.True(await reg.CancelCommandAsync(agent, commandId));
        Assert.True(await reg.HeartbeatPingAsync(agent, DateTimeOffset.UtcNow));
        Assert.True(await reg.ShutdownAsync(agent));

        Assert.Equal(5, sent.Count);
        var types = sent.Select(f =>
        {
            using var doc = JsonDocument.Parse(f);
            return doc.RootElement.GetProperty("type").GetString();
        }).ToArray();
        Assert.Equal(
            new[] { "cancel_run", "command", "cancel", "heartbeat_ping", "shutdown" },
            types);

        // Spot-check the flattened command envelope fields.
        using var cmd = JsonDocument.Parse(sent[1]);
        Assert.Equal(commandId, cmd.RootElement.GetProperty("command_id").GetGuid());
        Assert.Equal("restart_service", cmd.RootElement.GetProperty("verb").GetString());
        Assert.Equal(30, cmd.RootElement.GetProperty("timeout_secs").GetInt64());
    }

    [Fact]
    public async Task Send_to_unregistered_agent_returns_false()
    {
        var reg = NewRegistry();

        Assert.False(await reg.CancelRunAsync(Guid.NewGuid(), Guid.NewGuid()));
    }

    [Fact]
    public async Task Failing_raw_sender_is_reported_as_not_sent()
    {
        var reg = NewRegistry();
        var agent = Guid.NewGuid();
        // A raw socket mid-teardown: its bounded channel is completed and the
        // enqueue throws. The registry must translate that into false, never
        // propagate it to the dispatcher.
        reg.Register(agent, "raw-conn-1",
            (_, _) => throw new InvalidOperationException("channel closed"));

        Assert.False(await reg.ShutdownAsync(agent));
    }

    [Fact]
    public void Stale_raw_unregister_never_evicts_a_reconnect()
    {
        var reg = NewRegistry();
        var agent = Guid.NewGuid();
        var (_, sender) = CapturingSender();

        // Raw socket 1 drops; the agent reconnects (raw socket 2) before the
        // old socket's teardown runs its compare-and-remove.
        reg.Register(agent, "raw-conn-1", sender);
        reg.Register(agent, "raw-conn-2", sender);
        reg.Unregister(agent, "raw-conn-1"); // stale

        Assert.True(reg.IsOnline(agent));

        reg.Unregister(agent, "raw-conn-2"); // current
        Assert.False(reg.IsOnline(agent));
    }

    [Fact]
    public async Task Reregistering_with_a_raw_sender_supersedes_the_signalr_registration()
    {
        var reg = NewRegistry();
        var agent = Guid.NewGuid();
        var (sent, sender) = CapturingSender();

        // Agent was known via SignalR (default sender), then reconnects over
        // the raw transport — last writer wins and outbound traffic must flow
        // through the raw sender, not the dead hub connection.
        reg.Register(agent, "signalr-conn-1");
        reg.Register(agent, "raw-conn-1", sender);

        Assert.True(await reg.ShutdownAsync(agent));
        var frame = Assert.Single(sent);
        using var doc = JsonDocument.Parse(frame);
        Assert.Equal("shutdown", doc.RootElement.GetProperty("type").GetString());

        // And the stale SignalR disconnect must not evict the raw mapping.
        reg.Unregister(agent, "signalr-conn-1");
        Assert.True(reg.IsOnline(agent));
    }
}
