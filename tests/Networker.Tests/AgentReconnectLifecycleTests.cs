using System.Net.WebSockets;
using System.Text;
using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Security;
using Networker.Data;
using Networker.Data.Entities;
using Xunit;

namespace Networker.Tests;

/// <summary>
/// Audit P1-14: the agent's connect → drop → reconnect lifecycle, driven over
/// REAL WebSockets against the real <c>/ws/agent</c> endpoint and real
/// Postgres.
///
/// <para>This is the flow that broke in production. On 2026-08-03 a deploy
/// restarted the control plane; every agent socket dropped;
/// <c>HandleDisconnectAsync</c> eagerly failed each agent's in-flight runs —
/// and destroyed four matrix cells of roughly a thousand attempts each, while
/// the agents' tester processes were still happily executing and reconnected
/// seconds later.</para>
///
/// <para>Existing coverage calls <c>HandleDisconnectAsync</c> directly on
/// SQLite. That pins the method but not the lifecycle: it never opens a
/// socket, so it cannot see the registry's compare-and-remove, the
/// supersede-on-reconnect branch, or whether a reconnected agent can still
/// finish the run it was carrying. Those are the parts that actually failed.
/// </para>
/// </summary>
public class AgentReconnectLifecycleTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fx;

    public AgentReconnectLifecycleTests(ControlPlaneFixture fx) => _fx = fx;

    private const string Pid = ControlPlaneFixture.SeededProjectId;

    /// <summary>How long to allow the server's `finally` cleanup to run after a
    /// client-side close before asserting on its effects.</summary>
    private static readonly TimeSpan CleanupGrace = TimeSpan.FromSeconds(10);

    private sealed record Fixture(Guid AgentId, string ApiKey, Guid RunId, Guid ConfigId);

    /// <summary>Seed an agent with a known api key plus one RUNNING run
    /// attributed to it, exactly as a mid-flight matrix cell looks.</summary>
    private async Task<Fixture> SeedAgentWithRunningRunAsync(string label)
    {
        var agentId = Guid.NewGuid();
        var apiKey = $"itest-{label}-{Guid.NewGuid():N}";
        var runId = Guid.NewGuid();
        var cfgId = Guid.NewGuid();
        var now = DateTime.UtcNow;

        await using var db = _fx.NewDbContext();
        db.Agents.Add(new Networker.Data.Entities.Agent
        {
            AgentId = agentId,
            Name = $"reconnect-{label}-{agentId:N}"[..40],
            ProjectId = Pid,
            Status = "offline",
            Region = "eastus",
            Provider = "azure",
            Version = "0.28.156",
            RegisteredAt = now.AddHours(-1),
            LastHeartbeat = now,
            ApiKeyHash = AgentApiKeys.HashHex(apiKey),
        });
        db.TestConfigs.Add(new TestConfig
        {
            Id = cfgId,
            ProjectId = Pid,
            Name = $"reconnect-{cfgId:N}",
            EndpointKind = "network",
            EndpointRef = """{"kind":"network","host":"10.0.0.7","port":8444}""",
            Workload = """{"modes":["http1"],"runs":50}""",
            MaxDurationSecs = 1800,
            CreatedAt = now.AddMinutes(-30),
            UpdatedAt = now.AddMinutes(-30),
        });
        db.TestRuns.Add(new TestRun
        {
            Id = runId,
            TestConfigId = cfgId,
            ProjectId = Pid,
            Status = "running",
            WorkerId = agentId.ToString(),
            SuccessCount = 947,   // mid-flight, like the cells that were lost
            FailureCount = 0,
            StartedAt = now.AddMinutes(-20),
            CreatedAt = now.AddMinutes(-25),
        });
        await db.SaveChangesAsync();
        return new Fixture(agentId, apiKey, runId, cfgId);
    }

    private async Task<WebSocket> ConnectAgentAsync(string apiKey, CancellationToken ct = default)
    {
        var client = _fx.Server.CreateWebSocketClient();
        var socket = await client.ConnectAsync(
            new Uri($"ws://localhost/ws/agent?key={Uri.EscapeDataString(apiKey)}"), ct);

        // Drain the welcome frame so the connection is fully established before
        // the test acts — otherwise a close can race the server's registration
        // and the assertions would be testing connect ordering, not reconnect.
        var buffer = new byte[16 * 1024];
        using var cts = CancellationTokenSource.CreateLinkedTokenSource(ct);
        cts.CancelAfter(TimeSpan.FromSeconds(15));
        var received = await socket.ReceiveAsync(buffer, cts.Token);
        var frame = Encoding.UTF8.GetString(buffer, 0, received.Count);
        using var doc = JsonDocument.Parse(frame);
        Assert.Equal("welcome", doc.RootElement.GetProperty("type").GetString());
        return socket;
    }

    private static async Task SendAsync(WebSocket socket, object frame)
    {
        var json = JsonSerializer.Serialize(frame);
        await socket.SendAsync(
            Encoding.UTF8.GetBytes(json), WebSocketMessageType.Text, true, CancellationToken.None);
    }

    /// <summary>Poll until <paramref name="predicate"/> holds or the grace
    /// expires. The server's disconnect cleanup runs asynchronously after the
    /// client closes, so a bare read would be a race.</summary>
    private async Task<T> EventuallyAsync<T>(
        Func<NetworkerDbContext, Task<T>> read, Func<T, bool> predicate, string what)
    {
        var deadline = DateTime.UtcNow + CleanupGrace;
        T last = default!;
        while (DateTime.UtcNow < deadline)
        {
            await using var db = _fx.NewDbContext();
            last = await read(db);
            if (predicate(last))
            {
                return last;
            }
            await Task.Delay(200);
        }
        Assert.Fail($"timed out after {CleanupGrace.TotalSeconds}s waiting for {what}; last value: {last}");
        return last;
    }

    private Task<string> RunStatusAsync(Guid runId) =>
        EventuallyAsync(
            db => db.TestRuns.AsNoTracking().Where(r => r.Id == runId)
                    .Select(r => r.Status).FirstAsync(),
            _ => true, "run status");

    // ── 1. A drop must not destroy in-flight work ─────────────────────────────

    [Fact]
    public async Task A_socket_drop_marks_the_agent_offline_but_leaves_its_run_running()
    {
        var f = await SeedAgentWithRunningRunAsync("drop");

        var socket = await ConnectAgentAsync(f.ApiKey);
        // The agent is online while connected.
        await EventuallyAsync(
            db => db.Agents.AsNoTracking().Where(a => a.AgentId == f.AgentId)
                    .Select(a => a.Status).FirstAsync(),
            s => s == "online", "agent to come online");

        // Drop it the way a control-plane restart does.
        await socket.CloseAsync(WebSocketCloseStatus.NormalClosure, "restart", CancellationToken.None);
        socket.Dispose();

        var agentStatus = await EventuallyAsync(
            db => db.Agents.AsNoTracking().Where(a => a.AgentId == f.AgentId)
                    .Select(a => a.Status).FirstAsync(),
            s => s == "offline", "agent to go offline");
        Assert.Equal("offline", agentStatus);

        // The load-bearing assertion: the run SURVIVES. Failing it here is what
        // cost four matrix cells on 2026-08-03. Reaping belongs to the
        // watchdog, which additionally demands 120s of heartbeat silence.
        var status = await RunStatusAsync(f.RunId);
        Assert.True(status == "running",
            $"an in-flight run was moved to '{status}' by a mere socket drop — "
            + "a deploy would destroy every running run again");

        // …and its progress must not be rewritten on the way out.
        await using var db = _fx.NewDbContext();
        var success = await db.TestRuns.AsNoTracking()
            .Where(r => r.Id == f.RunId).Select(r => r.SuccessCount).FirstAsync();
        Assert.Equal(947, success);
    }

    // ── 2. The property that actually matters to a user ───────────────────────

    [Fact]
    public async Task A_reconnected_agent_can_still_finish_the_run_it_was_carrying()
    {
        // End to end: a run is in flight, the control plane "restarts" (socket
        // drops), the agent reconnects, and the work it kept doing across the
        // gap is still accepted. If any link in that chain is broken the user
        // loses a run that actually completed.
        var f = await SeedAgentWithRunningRunAsync("resume");

        var first = await ConnectAgentAsync(f.ApiKey);
        await first.CloseAsync(WebSocketCloseStatus.NormalClosure, "restart", CancellationToken.None);
        first.Dispose();

        await EventuallyAsync(
            db => db.Agents.AsNoTracking().Where(a => a.AgentId == f.AgentId)
                    .Select(a => a.Status).FirstAsync(),
            s => s == "offline", "agent to go offline after the drop");

        // Reconnect, exactly as the real agent does after a deploy.
        using var second = await ConnectAgentAsync(f.ApiKey);
        await EventuallyAsync(
            db => db.Agents.AsNoTracking().Where(a => a.AgentId == f.AgentId)
                    .Select(a => a.Status).FirstAsync(),
            s => s == "online", "agent to come back online");

        // Report the run it never stopped executing.
        await SendAsync(second, new
        {
            type = "run_finished",
            run_id = f.RunId,
            status = "completed",
        });

        var status = await EventuallyAsync(
            db => db.TestRuns.AsNoTracking().Where(r => r.Id == f.RunId)
                    .Select(r => r.Status).FirstAsync(),
            s => s == "completed", "the resumed run to be accepted as completed");
        Assert.Equal("completed", status);
    }

    // ── 3. The supersede race (quality audit F1) ──────────────────────────────

    [Fact]
    public async Task A_stale_socket_closing_after_a_reconnect_does_not_knock_the_agent_offline()
    {
        // A dead socket can go unnoticed for up to the 120s idle timeout, so its
        // cleanup routinely runs AFTER the agent has already reconnected. If
        // that cleanup isn't guarded by the registry's compare-and-remove it
        // marks a live, online agent offline — and the watchdog then reaps runs
        // that are executing fine on the new connection.
        var f = await SeedAgentWithRunningRunAsync("supersede");

        var stale = await ConnectAgentAsync(f.ApiKey);
        using var fresh = await ConnectAgentAsync(f.ApiKey);   // supersedes `stale`

        await EventuallyAsync(
            db => db.Agents.AsNoTracking().Where(a => a.AgentId == f.AgentId)
                    .Select(a => a.Status).FirstAsync(),
            s => s == "online", "agent to be online on the fresh connection");

        // Now the OLD socket finally notices it is dead.
        await stale.CloseAsync(WebSocketCloseStatus.NormalClosure, "stale", CancellationToken.None);
        stale.Dispose();

        // Give the (skipped) cleanup a chance to do damage if the guard is gone.
        await Task.Delay(TimeSpan.FromSeconds(2));

        await using var db = _fx.NewDbContext();
        var agentStatus = await db.Agents.AsNoTracking()
            .Where(a => a.AgentId == f.AgentId).Select(a => a.Status).FirstAsync();
        var runStatus = await db.TestRuns.AsNoTracking()
            .Where(r => r.Id == f.RunId).Select(r => r.Status).FirstAsync();

        Assert.True(agentStatus == "online",
            $"a superseded socket's cleanup marked a LIVE agent '{agentStatus}' — "
            + "the watchdog would then reap runs that are executing normally");
        Assert.Equal("running", runStatus);

        // The fresh connection must still work after the stale one's teardown.
        await SendAsync(fresh, new { type = "heartbeat", load = 0.1, version = "0.28.156" });
        var beat = await EventuallyAsync(
            d => d.Agents.AsNoTracking().Where(a => a.AgentId == f.AgentId)
                  .Select(a => a.LastHeartbeat).FirstAsync(),
            h => h != null, "a heartbeat on the surviving connection");
        Assert.NotNull(beat);
    }

    // ── 4. Auth is still enforced on reconnect ────────────────────────────────

    [Fact]
    public async Task A_reconnect_with_the_wrong_key_is_refused()
    {
        // Reconnect handling must not become a hole in agent auth.
        var f = await SeedAgentWithRunningRunAsync("badkey");
        using var good = await ConnectAgentAsync(f.ApiKey);

        var client = _fx.Server.CreateWebSocketClient();
        await Assert.ThrowsAnyAsync<Exception>(async () =>
        {
            using var socket = await client.ConnectAsync(
                new Uri($"ws://localhost/ws/agent?key={f.ApiKey}-tampered"), CancellationToken.None);
            var buffer = new byte[1024];
            using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(5));
            var r = await socket.ReceiveAsync(buffer, cts.Token);
            if (r.MessageType != WebSocketMessageType.Close)
            {
                throw new InvalidOperationException("a bad agent key was served the agent protocol");
            }
        });

        // …and the rejection must not disturb the legitimate connection.
        await using var db = _fx.NewDbContext();
        var runStatus = await db.TestRuns.AsNoTracking()
            .Where(r => r.Id == f.RunId).Select(r => r.Status).FirstAsync();
        Assert.Equal("running", runStatus);
    }
}
