using System.Net.WebSockets;
using System.Text;
using System.Text.Json;
using Microsoft.Extensions.DependencyInjection;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Realtime;

namespace Networker.Tests;

/// End-to-end proof of the M6 cutover transport: a plain WebSocket client (the
/// same thing the React frontend and the Rust agents are) connects to
/// /ws/dashboard on the REAL in-process app, and an event published through the
/// EventBus arrives as the flat {"seq":N,"type":...} JSON frame — no SignalR
/// envelope. If this passes, the frontend can connect unmodified at cutover.
public sealed class RawWebSocketIntegrationTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fixture;

    public RawWebSocketIntegrationTests(ControlPlaneFixture fixture) => _fixture = fixture;

    [Fact]
    public async Task Dashboard_socket_receives_published_event_as_flat_json_frame()
    {
        var tokens = _fixture.Services.GetRequiredService<JwtTokenService>();
        var jwt = tokens.CreateToken(
            ControlPlaneFixture.SeededUserId, ControlPlaneFixture.SeededUserEmail,
            "operator", isPlatformAdmin: false);

        // Publish FIRST, then connect with ?since=<earlier seq> — the replay
        // path (the same mechanism the frontend uses on reconnect) must deliver
        // the later frame. Two publishes because since=0 means "no replay" per
        // the Rust contract, so we anchor since on a real earlier seq (>0).
        var bus = _fixture.Services.GetRequiredService<EventBus>();
        var anchorSeq = bus.Publish(new JobUpdate(Guid.NewGuid(), "queued", null, null, null));
        var jobId = Guid.NewGuid();
        var seq = bus.Publish(new JobUpdate(jobId, "running", null, null, null));

        var wsClient = _fixture.Server.CreateWebSocketClient();
        using var socket = await wsClient.ConnectAsync(
            new Uri($"ws://localhost/ws/dashboard?token={jwt}&since={anchorSeq}"),
            CancellationToken.None);

        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(10));
        var buffer = new byte[16 * 1024];
        var result = await socket.ReceiveAsync(buffer, cts.Token);
        var frame = Encoding.UTF8.GetString(buffer, 0, result.Count);

        using var doc = JsonDocument.Parse(frame);
        Assert.Equal("job_update", doc.RootElement.GetProperty("type").GetString());
        Assert.Equal(jobId.ToString(), doc.RootElement.GetProperty("job_id").GetString());
        Assert.Equal(seq, doc.RootElement.GetProperty("seq").GetInt64());
    }

    [Fact]
    public async Task Dashboard_socket_rejects_missing_token()
    {
        var wsClient = _fixture.Server.CreateWebSocketClient();

        await Assert.ThrowsAnyAsync<Exception>(async () =>
        {
            using var socket = await wsClient.ConnectAsync(
                new Uri("ws://localhost/ws/dashboard"), CancellationToken.None);
            // If the server let the upgrade through, the close/first-receive
            // must reject us instead.
            var buffer = new byte[256];
            using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(5));
            var r = await socket.ReceiveAsync(buffer, cts.Token);
            if (r.MessageType != WebSocketMessageType.Close)
            {
                throw new InvalidOperationException("unauthenticated socket was served data");
            }
            throw new OperationCanceledException("closed as expected");
        });
    }
}
