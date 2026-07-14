using Microsoft.AspNetCore.Authorization;
using Microsoft.AspNetCore.SignalR;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;

namespace Networker.ControlPlane.Realtime;

/// <summary>
/// Project-scoped tester-queue hub mapped (by Program.cs) at <c>/ws/testers</c>.
///
/// C# port of the Rust raw-WebSocket handler
/// (crates/networker-dashboard/src/ws/tester_hub.rs). Clients authenticate with
/// the same JWT the REST API uses — for a WebSocket the token arrives as the
/// <c>?access_token=&lt;jwt&gt;</c> query string, which the JwtBearer handler is
/// configured to read (see the wiring note in
/// <see cref="TesterQueueHubExtensions"/>). The <see cref="AuthorizeAttribute"/>
/// then rejects the connection before <see cref="OnConnectedAsync"/> if the
/// token is missing/invalid — the same 401 the Rust handler returns for an empty
/// or bad <c>?token=</c>.
///
/// Client → server methods:
///   * <see cref="SubscribeTesterQueue"/> — validate membership + tester
///     ownership, enforce the rate limit + per-project cap, join the SignalR
///     group for each valid tester, and push an initial snapshot.
///   * <see cref="UnsubscribeTesterQueue"/> — leave the groups.
///
/// Server → client: a single client method <c>TesterMessage</c> carries the
/// <c>type</c>-tagged payload object (snapshot/update/phase). The payload body
/// is byte-identical to the Rust JSON; only the SignalR frame envelope differs
/// from the raw-WS Rust build.
/// </summary>
[Authorize]
public sealed class TesterQueueHub(
    TesterQueueRegistry registry,
    AuthRepository auth,
    NetworkerDbContext db,
    ILogger<TesterQueueHub> logger) : Hub
{
    /// <summary>SignalR client method name the frontend listens on.</summary>
    public const string ClientMethod = "TesterMessage";

    private const string BucketItemKey = "tq_sub_bucket";
    private const int DefaultMaxSubMsgsPerMin = 10;

    private static readonly int MaxSubMsgsPerMin = ResolveMaxSubMsgsPerMin();

    // ── Client → server ──────────────────────────────────────────────────────

    /// <summary>
    /// Subscribe to queue updates for <paramref name="testerIds"/> within
    /// <paramref name="projectId"/>. Steps mirror the Rust <c>handle_subscribe</c>:
    ///   1. rate-limit this connection (sliding 60s window);
    ///   2. verify project membership (platform admins bypass);
    ///   3. keep only tester ids that actually belong to the project;
    ///   4. register + join the group (per-project cap enforced);
    ///   5. send the initial <c>tester_queue_snapshot</c>.
    /// Invalid ids are silently skipped, matching the Rust behaviour.
    /// </summary>
    public async Task SubscribeTesterQueue(string projectId, IReadOnlyList<string> testerIds)
    {
        if (!AllowMessage())
        {
            logger.LogDebug("tester ws: subscribe rate limit exceeded (conn {Conn})", Context.ConnectionId);
            return;
        }

        var user = CurrentUser();
        if (user is null)
        {
            return; // [Authorize] should prevent this, but fail closed.
        }

        // 1. Project membership. Platform admins get implicit access.
        if (!user.IsPlatformAdmin)
        {
            var role = await auth.GetMemberRoleAsync(projectId, user.UserId, Context.ConnectionAborted);
            if (role is null)
            {
                logger.LogWarning(
                    "tester ws: user {User} is not a member of project {Project}",
                    user.UserId, projectId);
                return;
            }
        }

        if (testerIds is null || testerIds.Count == 0)
        {
            return;
        }

        // 2. Validate tester ownership: keep only ids in project_tester for this
        // project. Non-guid strings can never match a Guid PK, so they drop out.
        var candidateGuids = testerIds
            .Select(id => Guid.TryParse(id, out var g) ? (Guid?)g : null)
            .Where(g => g is not null)
            .Select(g => g!.Value)
            .Distinct()
            .ToList();

        if (candidateGuids.Count == 0)
        {
            return;
        }

        var validIds = await db.ProjectTesters
            .AsNoTracking()
            .Where(t => t.ProjectId == projectId && candidateGuids.Contains(t.TesterId))
            .Select(t => t.TesterId)
            .ToListAsync(Context.ConnectionAborted);

        // 3. Register + join group + snapshot each valid tester.
        foreach (var testerId in validIds)
        {
            var testerKey = testerId.ToString();

            if (!registry.TrySubscribe(projectId, testerKey, Context.ConnectionId))
            {
                logger.LogWarning(
                    "tester ws: per-project subscription cap reached for project {Project}", projectId);
                continue;
            }

            await Groups.AddToGroupAsync(
                Context.ConnectionId,
                TesterQueueRegistry.GroupName(projectId, testerKey),
                Context.ConnectionAborted);

            var snapshot = await BuildSnapshotAsync(projectId, testerId, Context.ConnectionAborted);
            await Clients.Caller.SendAsync(ClientMethod, snapshot, Context.ConnectionAborted);
        }
    }

    /// <summary>
    /// Unsubscribe this connection from <paramref name="testerIds"/> across every
    /// project it is subscribed under (the Rust inbound message carries tester
    /// ids only, no project). Rate-limited on the same bucket as subscribe.
    /// </summary>
    public Task UnsubscribeTesterQueue(IReadOnlyList<string> testerIds)
    {
        if (!AllowMessage() || testerIds is null)
        {
            return Task.CompletedTask;
        }

        // The Rust inbound message carries tester ids only (no project), so the
        // registry resolves which project(s) each tester was subscribed under
        // for this connection and leaves the matching SignalR group(s).
        var connId = Context.ConnectionId;
        foreach (var raw in testerIds)
        {
            registry.RemoveConnectionFromTester(raw, connId, (projectId, testerId) =>
                Groups.RemoveFromGroupAsync(
                    connId, TesterQueueRegistry.GroupName(projectId, testerId)));
        }

        return Task.CompletedTask;
    }

    // ── Lifecycle ─────────────────────────────────────────────────────────────

    public override async Task OnDisconnectedAsync(Exception? exception)
    {
        registry.RemoveConnection(Context.ConnectionId);
        await base.OnDisconnectedAsync(exception);
    }

    // ── Snapshot builder (parity with TestersEndpoints.GetQueue) ──────────────

    /// <summary>
    /// Build the initial <c>tester_queue_snapshot</c> for a tester using the same
    /// running/queued query + ETA logic the M1 <c>GET .../{testerId}/queue</c>
    /// endpoint uses (see TestersEndpoints.GetQueue): running rows first, then
    /// queued oldest-first; queued positions are 1-based and ETAs are computed
    /// from the tester's rolling average benchmark duration.
    /// </summary>
    private async Task<TesterQueueSnapshotMessage> BuildSnapshotAsync(
        string projectId, Guid testerId, CancellationToken ct)
    {
        var seq = registry.NextSeq(projectId, testerId.ToString());

        var avgSecs = await db.ProjectTesters
            .AsNoTracking()
            .Where(t => t.ProjectId == projectId && t.TesterId == testerId)
            .Select(t => t.AvgBenchmarkDurationSeconds)
            .FirstOrDefaultAsync(ct);

        var rows = await db.TestRuns
            .AsNoTracking()
            .Where(r => r.TesterId == testerId && (r.Status == "running" || r.Status == "queued"))
            .Join(db.TestConfigs, r => r.TestConfigId, c => c.Id, (r, c) => new
            {
                config_id = r.TestConfigId,
                name = c.Name,
                status = r.Status,
                queued_at = r.CreatedAt,
            })
            .OrderBy(x => x.status == "running" ? 0 : 1)
            .ThenBy(x => x.queued_at)
            .ToListAsync(ct);

        TesterQueueEntry? running = null;
        var queued = new List<TesterQueueEntry>();

        foreach (var row in rows)
        {
            if (row.status == "running" && running is null)
            {
                running = new TesterQueueEntry(row.config_id.ToString(), row.name);
            }
            else if (row.status == "queued")
            {
                queued.Add(new TesterQueueEntry(row.config_id.ToString(), row.name));
            }
        }

        // Assign 1-based positions + ETA seconds from the rolling average.
        for (var i = 0; i < queued.Count; i++)
        {
            var position = (uint)(i + 1);
            uint? etaSeconds = avgSecs.HasValue
                ? (uint)((long)i * avgSecs.Value)
                : null;
            queued[i] = queued[i] with { Position = position, EtaSeconds = etaSeconds };
        }

        return new TesterQueueSnapshotMessage(
            projectId,
            testerId.ToString(),
            seq,
            queued,
            running);
    }

    // ── Per-connection sliding-window rate limit ──────────────────────────────

    /// <summary>
    /// Sliding 60s window: at most <see cref="MaxSubMsgsPerMin"/> subscribe/
    /// unsubscribe messages per connection. Mirrors the Rust <c>SubMsgBucket</c>.
    /// The bucket lives in <c>Context.Items</c> (per-connection state).
    /// </summary>
    private bool AllowMessage()
    {
        if (Context.Items.TryGetValue(BucketItemKey, out var raw) && raw is SubMsgBucket bucket)
        {
            return bucket.Allow();
        }

        var created = new SubMsgBucket(MaxSubMsgsPerMin);
        Context.Items[BucketItemKey] = created;
        return created.Allow();
    }

    private AuthUser? CurrentUser() => AuthUser.FromPrincipal(Context.User);

    private static int ResolveMaxSubMsgsPerMin()
    {
        var raw = Environment.GetEnvironmentVariable("DASHBOARD_MAX_SUB_MSGS_PER_MIN");
        return int.TryParse(raw, out var v) && v > 0 ? v : DefaultMaxSubMsgsPerMin;
    }

    /// <summary>Sliding-window token bucket, one per hub connection.</summary>
    private sealed class SubMsgBucket(int cap)
    {
        private readonly Queue<DateTimeOffset> _stamps = new();
        private readonly object _gate = new();

        public bool Allow()
        {
            var now = DateTimeOffset.UtcNow;
            var cutoff = now - TimeSpan.FromSeconds(60);
            lock (_gate)
            {
                while (_stamps.Count > 0 && _stamps.Peek() < cutoff)
                {
                    _stamps.Dequeue();
                }

                if (_stamps.Count >= cap)
                {
                    return false;
                }

                _stamps.Enqueue(now);
                return true;
            }
        }
    }
}
