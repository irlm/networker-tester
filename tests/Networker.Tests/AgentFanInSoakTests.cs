using System.Net.WebSockets;
using System.Text;
using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Networker.ControlPlane.Realtime;
using Networker.ControlPlane.Security;
using Xunit;

namespace Networker.Tests;

/// <summary>
/// Many-agent WebSocket fan-in — the last named gap from the 2026-08 test
/// assessment. <see cref="AgentReconnectLifecycleTests"/> pins ONE agent's
/// connect → drop → reconnect; nothing pinned the hub under CONCURRENT load:
/// registration atomicity, per-agent row isolation (a heartbeat from agent i
/// must never land on agent j's row), disconnect-of-some-not-others, and
/// targeted routing (a frame addressed to one agent reaching exactly that
/// agent). Production runs whole matrix campaigns through this hub — ten
/// cells' agents connect within seconds of each other.
///
/// <para>All deadlines are bounded polls on positive signals (the readiness
/// lesson): a test that hangs on regression is barely better than one that
/// passes on it.</para>
/// </summary>
public class AgentFanInSoakTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fx;

    public AgentFanInSoakTests(ControlPlaneFixture fx) => _fx = fx;

    private const string Pid = ControlPlaneFixture.SeededProjectId;
    private static readonly TimeSpan FanInDeadline = TimeSpan.FromSeconds(30);
    private static readonly TimeSpan SettleDeadline = TimeSpan.FromSeconds(15);

    private sealed record Seeded(Guid AgentId, string ApiKey, int Index);

    private async Task<List<Seeded>> SeedAgentsAsync(string label, int n)
    {
        var now = DateTime.UtcNow;
        var seeded = new List<Seeded>(n);
        await using var db = _fx.NewDbContext();
        for (var i = 0; i < n; i++)
        {
            var id = Guid.NewGuid();
            var key = $"fanin-{label}-{i}-{Guid.NewGuid():N}";
            db.Agents.Add(new Networker.Data.Entities.Agent
            {
                AgentId = id,
                Name = $"fanin-{label}-{i:d3}-{id:N}"[..40],
                ProjectId = Pid,
                Status = "offline",
                Region = "eastus",
                Provider = "azure",
                Version = "0.0.0",
                RegisteredAt = now.AddHours(-1),
                LastHeartbeat = now.AddHours(-1),
                ApiKeyHash = AgentApiKeys.HashHex(key),
            });
            seeded.Add(new Seeded(id, key, i));
        }
        await db.SaveChangesAsync();
        return seeded;
    }

    private async Task<WebSocket> ConnectAsync(string apiKey, CancellationToken ct)
    {
        var client = _fx.Server.CreateWebSocketClient();
        var socket = await client.ConnectAsync(
            new Uri($"ws://localhost/ws/agent?key={Uri.EscapeDataString(apiKey)}"), ct);

        // Drain the welcome frame so registration is complete before the test
        // acts (same rationale as the reconnect lifecycle tests).
        var buffer = new byte[16 * 1024];
        using var cts = CancellationTokenSource.CreateLinkedTokenSource(ct);
        cts.CancelAfter(TimeSpan.FromSeconds(20));
        var received = await socket.ReceiveAsync(buffer, cts.Token);
        using var doc = JsonDocument.Parse(Encoding.UTF8.GetString(buffer, 0, received.Count));
        Assert.Equal("welcome", doc.RootElement.GetProperty("type").GetString());
        return socket;
    }

    private static Task SendJsonAsync(WebSocket socket, object frame) =>
        socket.SendAsync(
            Encoding.UTF8.GetBytes(JsonSerializer.Serialize(frame)),
            WebSocketMessageType.Text, true, CancellationToken.None);

    /// <summary>Poll until <paramref name="check"/> returns true or the
    /// deadline passes; the LAST failure message is surfaced on timeout.</summary>
    private static async Task PollUntilAsync(
        TimeSpan deadline, Func<Task<(bool Ok, string Detail)>> check)
    {
        var until = DateTime.UtcNow + deadline;
        var detail = "no iterations ran";
        while (DateTime.UtcNow < until)
        {
            var (ok, d) = await check();
            if (ok)
            {
                return;
            }
            detail = d;
            await Task.Delay(250);
        }
        Assert.Fail($"condition not reached within {deadline}: {detail}");
    }

    [Fact]
    public async Task Forty_agents_fan_in_without_cross_talk_and_half_disconnect_cleanly()
    {
        const int N = 40;
        var seeded = await SeedAgentsAsync("bulk", N);
        var sockets = new WebSocket[N];
        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(60));

        try
        {
            // Concurrent fan-in: all 40 connect at once — this is the deploy /
            // matrix-launch shape, not a polite one-at-a-time parade.
            await Task.WhenAll(seeded.Select(async s =>
                sockets[s.Index] = await ConnectAsync(s.ApiKey, cts.Token)));

            // Every agent heartbeats a DISTINCT version marker concurrently.
            await Task.WhenAll(seeded.Select(s => SendJsonAsync(
                sockets[s.Index],
                new { type = "heartbeat", load = 0.1, version = $"9.9.{s.Index}" })));

            // All 40 online with EXACTLY their own marker (cross-talk check).
            await PollUntilAsync(FanInDeadline, async () =>
            {
                await using var db = _fx.NewDbContext();
                var ids = seeded.Select(s => s.AgentId).ToList();
                var rows = await db.Agents.AsNoTracking()
                    .Where(a => ids.Contains(a.AgentId))
                    .Select(a => new { a.AgentId, a.Status, a.Version })
                    .ToListAsync();
                var online = rows.Count(r => r.Status == "online");
                if (online != N)
                {
                    return (false, $"{online}/{N} online");
                }
                foreach (var s in seeded)
                {
                    var row = rows.Single(r => r.AgentId == s.AgentId);
                    if (row.Version != $"9.9.{s.Index}")
                    {
                        // The one outcome this test exists to forbid.
                        return (false,
                            $"agent {s.Index} carries version {row.Version} — cross-talk");
                    }
                }
                return (true, "");
            });

            // Registry agrees with the DB.
            var registry = _fx.Services.GetRequiredService<AgentConnectionRegistry>();
            foreach (var s in seeded)
            {
                Assert.True(registry.IsOnline(s.AgentId), $"agent {s.Index} not in registry");
            }

            // Close the even half concurrently; the odd half must be untouched.
            await Task.WhenAll(seeded.Where(s => s.Index % 2 == 0).Select(s =>
                sockets[s.Index].CloseAsync(
                    WebSocketCloseStatus.NormalClosure, "bye", CancellationToken.None)));

            await PollUntilAsync(SettleDeadline, async () =>
            {
                await using var db = _fx.NewDbContext();
                var ids = seeded.Select(s => s.AgentId).ToList();
                var rows = await db.Agents.AsNoTracking()
                    .Where(a => ids.Contains(a.AgentId))
                    .Select(a => new { a.AgentId, a.Status })
                    .ToListAsync();
                foreach (var s in seeded)
                {
                    var status = rows.Single(r => r.AgentId == s.AgentId).Status;
                    var expected = s.Index % 2 == 0 ? "offline" : "online";
                    if (status != expected)
                    {
                        return (false, $"agent {s.Index} is {status}, expected {expected}");
                    }
                }
                return (true, "");
            });

            // Registry-side: exactly the odd half remains.
            foreach (var s in seeded)
            {
                Assert.Equal(s.Index % 2 != 0, registry.IsOnline(s.AgentId));
            }
        }
        finally
        {
            foreach (var socket in sockets.Where(s =>
                         s is { State: WebSocketState.Open }))
            {
                try
                {
                    await socket.CloseAsync(
                        WebSocketCloseStatus.NormalClosure, "cleanup", CancellationToken.None);
                }
                catch
                {
                    // Cleanup must not mask the real assertion.
                }
            }
        }
    }

    [Fact]
    public async Task Targeted_send_reaches_exactly_the_addressed_agent()
    {
        const int N = 10;
        var seeded = await SeedAgentsAsync("route", N);
        var sockets = new WebSocket[N];
        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(60));

        try
        {
            await Task.WhenAll(seeded.Select(async s =>
                sockets[s.Index] = await ConnectAsync(s.ApiKey, cts.Token)));

            var registry = _fx.Services.GetRequiredService<AgentConnectionRegistry>();
            var target = seeded[7];
            var delivered = await registry.HeartbeatPingAsync(
                target.AgentId, DateTimeOffset.UtcNow, cts.Token);
            Assert.True(delivered, "registry reported the targeted send undeliverable");

            // The addressed socket receives the ping…
            var buffer = new byte[16 * 1024];
            using var recvCts = new CancellationTokenSource(TimeSpan.FromSeconds(5));
            var result = await sockets[7].ReceiveAsync(buffer, recvCts.Token);
            using var doc = JsonDocument.Parse(Encoding.UTF8.GetString(buffer, 0, result.Count));
            Assert.Equal("heartbeat_ping", doc.RootElement.GetProperty("type").GetString());

            // …and a sample of the others receives NOTHING in a bounded window.
            foreach (var idx in new[] { 0, 3, 9 })
            {
                using var quiet = new CancellationTokenSource(TimeSpan.FromMilliseconds(700));
                try
                {
                    var stray = await sockets[idx].ReceiveAsync(buffer, quiet.Token);
                    var frame = Encoding.UTF8.GetString(buffer, 0, stray.Count);
                    Assert.Fail($"agent {idx} received a frame addressed to agent 7: {frame}");
                }
                catch (OperationCanceledException)
                {
                    // Silence is the pass condition here — and it is a REAL
                    // negative assert (bounded), not an unbounded wait.
                }
            }
        }
        finally
        {
            foreach (var socket in sockets.Where(s =>
                         s is { State: WebSocketState.Open }))
            {
                try
                {
                    await socket.CloseAsync(
                        WebSocketCloseStatus.NormalClosure, "cleanup", CancellationToken.None);
                }
                catch
                {
                    // Cleanup must not mask the real assertion.
                }
            }
        }
    }

    [Fact]
    public async Task Heartbeat_storm_settles_every_agent_on_its_last_marker()
    {
        const int N = 12;
        const int Beats = 10;
        var seeded = await SeedAgentsAsync("storm", N);
        var sockets = new WebSocket[N];
        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(60));

        try
        {
            await Task.WhenAll(seeded.Select(async s =>
                sockets[s.Index] = await ConnectAsync(s.ApiKey, cts.Token)));

            // Each agent fires a burst of heartbeats; agents run concurrently,
            // frames within one agent are sequential (WebSocket send ordering).
            await Task.WhenAll(seeded.Select(async s =>
            {
                for (var beat = 1; beat <= Beats; beat++)
                {
                    await SendJsonAsync(sockets[s.Index], new
                    {
                        type = "heartbeat",
                        load = 0.5,
                        version = $"7.{s.Index}.{beat}",
                    });
                }
            }));

            // Every row settles on ITS final beat — a lost or cross-applied
            // heartbeat under the storm shows up as a stale/foreign marker.
            await PollUntilAsync(FanInDeadline, async () =>
            {
                await using var db = _fx.NewDbContext();
                var ids = seeded.Select(s => s.AgentId).ToList();
                var rows = await db.Agents.AsNoTracking()
                    .Where(a => ids.Contains(a.AgentId))
                    .Select(a => new { a.AgentId, a.Version })
                    .ToListAsync();
                foreach (var s in seeded)
                {
                    var version = rows.Single(r => r.AgentId == s.AgentId).Version;
                    if (version != $"7.{s.Index}.{Beats}")
                    {
                        return (false, $"agent {s.Index} settled on {version}");
                    }
                }
                return (true, "");
            });
        }
        finally
        {
            foreach (var socket in sockets.Where(s =>
                         s is { State: WebSocketState.Open }))
            {
                try
                {
                    await socket.CloseAsync(
                        WebSocketCloseStatus.NormalClosure, "cleanup", CancellationToken.None);
                }
                catch
                {
                    // Cleanup must not mask the real assertion.
                }
            }
        }
    }
}
