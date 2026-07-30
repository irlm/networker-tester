using System.Net;
using System.Net.Sockets;
using Networker.ControlPlane.Dispatch;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Launch-time reachability probe (the gate behind "target unreachable" 409s).
/// A run launched against a deallocated VM used to silently burn full
/// per-attempt timeouts across every mode (E2E 2026-07-30); the probe turns
/// that into an instant, actionable error. Never throws — a probe failure is
/// an answer, not an error.
/// </summary>
public class TargetReachabilityTests
{
    [Fact]
    public async Task Listening_port_is_reachable()
    {
        var listener = new TcpListener(IPAddress.Loopback, 0);
        listener.Start();
        try
        {
            var port = ((IPEndPoint)listener.LocalEndpoint).Port;
            Assert.True(await TargetReachability.TcpReachableAsync(
                "127.0.0.1", port, TimeSpan.FromSeconds(3), CancellationToken.None));
        }
        finally
        {
            listener.Stop();
        }
    }

    [Fact]
    public async Task Closed_port_is_unreachable_and_does_not_throw()
    {
        // Bind then stop → the port is definitively closed (RST, not filtered).
        var listener = new TcpListener(IPAddress.Loopback, 0);
        listener.Start();
        var port = ((IPEndPoint)listener.LocalEndpoint).Port;
        listener.Stop();

        Assert.False(await TargetReachability.TcpReachableAsync(
            "127.0.0.1", port, TimeSpan.FromSeconds(3), CancellationToken.None));
    }

    [Fact]
    public async Task Unresolvable_host_is_unreachable_not_a_throw()
    {
        Assert.False(await TargetReachability.TcpReachableAsync(
            "definitely-not-a-real-host.invalid", 443, TimeSpan.FromSeconds(2), CancellationToken.None));
    }
}
