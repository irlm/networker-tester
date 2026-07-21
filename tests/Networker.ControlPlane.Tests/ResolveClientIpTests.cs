using System.Net;
using Microsoft.AspNetCore.Http;
using Networker.ControlPlane.Realtime.RawWs;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// The agent WS auth path sits behind nginx, so the socket peer is the proxy
/// (127.0.0.1). <see cref="AgentSocketEndpoint.ResolveClientIp"/> must recover
/// the real client IP from nginx's forwarding headers so the brute-force
/// limiter is per-attacker (not one global bucket) and api_key_last_used_ip is
/// meaningful. Priority: X-Real-IP → first X-Forwarded-For hop → socket peer.
/// </summary>
public sealed class ResolveClientIpTests
{
    private static HttpContext Ctx(string? realIp, string? xff, string? socket)
    {
        var http = new DefaultHttpContext();
        if (realIp is not null) http.Request.Headers["X-Real-IP"] = realIp;
        if (xff is not null) http.Request.Headers["X-Forwarded-For"] = xff;
        if (socket is not null) http.Connection.RemoteIpAddress = IPAddress.Parse(socket);
        return http;
    }

    [Fact]
    public void Prefers_X_Real_IP_over_socket_and_xff()
    {
        var ip = AgentSocketEndpoint.ResolveClientIp(Ctx("203.0.113.7", "198.51.100.9", "127.0.0.1"));
        Assert.Equal("203.0.113.7", ip);
    }

    [Fact]
    public void Falls_back_to_first_X_Forwarded_For_hop_when_no_real_ip()
    {
        var ip = AgentSocketEndpoint.ResolveClientIp(Ctx(null, "203.0.113.7, 10.0.0.1, 127.0.0.1", "127.0.0.1"));
        Assert.Equal("203.0.113.7", ip);
    }

    [Fact]
    public void Falls_back_to_socket_peer_when_no_forwarding_headers()
    {
        var ip = AgentSocketEndpoint.ResolveClientIp(Ctx(null, null, "192.0.2.5"));
        Assert.Equal("192.0.2.5", ip);
    }

    [Fact]
    public void Blank_headers_do_not_shadow_the_socket_peer()
    {
        var ip = AgentSocketEndpoint.ResolveClientIp(Ctx("", "  ", "192.0.2.5"));
        Assert.Equal("192.0.2.5", ip);
    }
}
