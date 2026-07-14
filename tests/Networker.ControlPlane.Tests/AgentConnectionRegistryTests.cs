using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Realtime;

namespace Networker.ControlPlane.Tests;

/// Unit tests for the agent connection registry — the SignalR replacement for
/// the Rust agent hub's HashMap<agent_id, sender>. The M3 dispatcher checks
/// IsOnline before assigning a run, so the map + the compare-and-remove guard
/// (which protects a fresh reconnect from a stale disconnect) are load-bearing.
public sealed class AgentConnectionRegistryTests
{
    private static AgentConnectionRegistry NewRegistry()
    {
        var services = new ServiceCollection();
        services.AddLogging(b => b.SetMinimumLevel(LogLevel.Warning));
        services.AddSignalR();
        services.AddAgentProtocol();
        return services.BuildServiceProvider().GetRequiredService<AgentConnectionRegistry>();
    }

    [Fact]
    public void Register_makes_agent_online()
    {
        var reg = NewRegistry();
        var agent = Guid.NewGuid();

        Assert.False(reg.IsOnline(agent));
        reg.Register(agent, "conn-1");

        Assert.True(reg.IsOnline(agent));
        Assert.Contains(agent, reg.OnlineAgents());
    }

    [Fact]
    public void Unregister_with_matching_connection_removes_agent()
    {
        var reg = NewRegistry();
        var agent = Guid.NewGuid();
        reg.Register(agent, "conn-1");

        reg.Unregister(agent, "conn-1");

        Assert.False(reg.IsOnline(agent));
    }

    [Fact]
    public void Unregister_with_stale_connection_keeps_the_reconnect()
    {
        var reg = NewRegistry();
        var agent = Guid.NewGuid();

        // Agent connects (conn-1), drops, and reconnects (conn-2) before the
        // old socket's OnDisconnected fires. The late disconnect for conn-1
        // must NOT evict the live conn-2 mapping.
        reg.Register(agent, "conn-1");
        reg.Register(agent, "conn-2");
        reg.Unregister(agent, "conn-1"); // stale

        Assert.True(reg.IsOnline(agent));
    }

    [Fact]
    public void AnyOnlineAgent_reflects_registration_state()
    {
        var reg = NewRegistry();
        Assert.Null(reg.AnyOnlineAgent());

        var agent = Guid.NewGuid();
        reg.Register(agent, "conn-1");

        Assert.Equal(agent, reg.AnyOnlineAgent());
    }
}
