using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;

namespace Networker.ControlPlane.Realtime.RawWs;

/// <summary>
/// Raw-WebSocket tester-queue feed at <c>/ws/testers</c> — the endpoint the
/// React <c>useTesterSubscription.ts</c> / <c>usePhaseSubscription.ts</c> hooks
/// dial. The Rust <c>ws/tester_hub.rs</c> contract, re-served over the existing
/// C# services:
///
/// <list type="bullet">
///   <item><b>Auth before upgrade</b>: <c>?token=&lt;jwt&gt;</c> (also
///     <c>access_token</c>); missing/invalid → 401, non-WebSocket → 400.</item>
///   <item><b>Inbound text frames</b>:
///     <c>{"type":"subscribe_tester_queue","project_id":..,"tester_ids":[..]}</c> and
///     <c>{"type":"unsubscribe_tester_queue","tester_ids":[..]}</c>. Malformed or
///     unknown frames are ignored (Rust: parse-error → continue).</item>
///   <item><b>Subscribe pipeline</b> (mirrors <see cref="TesterQueueHub.SubscribeTesterQueue"/>,
///     using the same services in a per-message DI scope): sliding-window rate
///     limit (<c>DASHBOARD_MAX_SUB_MSGS_PER_MIN</c>, default 10) → project
///     membership via <see cref="AuthRepository.GetMemberRoleAsync"/> (platform
///     admins bypass) → tester ownership via <c>project_tester</c> → per-project
///     cap via <see cref="TesterQueueRegistry.TrySubscribe"/> → send the initial
///     <c>tester_queue_snapshot</c>.</item>
///   <item><b>Outbound</b>: snapshot/update/phase messages as flat JSON text
///     frames (<c>{"type":"tester_queue_snapshot",...}</c> — the exact
///     <see cref="TesterQueueMessages"/> shapes, no SignalR envelope). Live
///     updates arrive via <see cref="RawWsTesterQueueLifetimeManager"/>, which
///     mirrors the broadcaster's SignalR group sends into
///     <see cref="RawSocketRegistry.BroadcastTesterGroup"/>. Seqs come from the
///     same shared <see cref="TesterQueueRegistry"/> counters, so raw and
///     SignalR clients observe an identical monotonic stream.</item>
///   <item><b>Cleanup</b>: on disconnect the connection is removed from both
///     registries; a full send queue ejects the socket (Rust bounded-mpsc
///     slow-subscriber drop).</item>
/// </list>
/// </summary>
public static class TesterQueueSocketEndpoint
{
    public static async Task HandleAsync(HttpContext context)
    {
        if (!context.WebSockets.IsWebSocketRequest)
        {
            context.Response.StatusCode = StatusCodes.Status400BadRequest;
            await context.Response.WriteAsync("WebSocket upgrade required");
            return;
        }

        var principal = RawWsIo.Authenticate(context);
        var user = AuthUser.FromPrincipal(principal);
        if (user is null)
        {
            context.Response.StatusCode = StatusCodes.Status401Unauthorized;
            return;
        }

        var services = context.RequestServices;
        var tqRegistry = services.GetRequiredService<TesterQueueRegistry>();
        var rawRegistry = services.GetRequiredService<RawSocketRegistry>();
        var scopeFactory = services.GetRequiredService<IServiceScopeFactory>();
        var logger = services
            .GetRequiredService<ILoggerFactory>()
            .CreateLogger("Networker.ControlPlane.Realtime.RawWs.TesterQueueSocketEndpoint");

        using var socket = await context.WebSockets.AcceptWebSocketAsync();
        var aborted = context.RequestAborted;

        var connection = new RawSocketConnection(
            $"raw-tester-{Guid.NewGuid():N}",
            (json, ct) => RawWsIo.SendTextAsync(socket, json, ct),
            onDropped: _ => RawWsIo.SafeAbort(socket));

        // One rate-limit bucket per connection, same env knob + default as the
        // hub and the Rust SubMsgBucket.
        var bucket = new SlidingWindowRateLimiter(SlidingWindowRateLimiter.ResolveCapFromEnv());

        using var pumpCts = CancellationTokenSource.CreateLinkedTokenSource(aborted);
        var pump = connection.RunSendPumpAsync(pumpCts.Token);

        logger.LogInformation(
            "raw ws: tester feed {Conn} authenticated (user {User})", connection.Id, user.UserId);

        try
        {
            while (!aborted.IsCancellationRequested)
            {
                var text = await RawWsIo.ReceiveTextMessageAsync(socket, aborted);
                if (text is null)
                {
                    break; // closed / errored / oversized
                }

                string? type = null;
                try
                {
                    using var doc = JsonDocument.Parse(text);
                    var root = doc.RootElement;
                    if (root.ValueKind != JsonValueKind.Object ||
                        !root.TryGetProperty("type", out var typeProp) ||
                        typeProp.ValueKind != JsonValueKind.String)
                    {
                        continue;
                    }

                    type = typeProp.GetString();

                    switch (type)
                    {
                        case TesterQueueMessageTypes.SubscribeTesterQueue:
                        {
                            if (!bucket.Allow())
                            {
                                logger.LogDebug(
                                    "raw ws: subscribe rate limit exceeded (conn {Conn})", connection.Id);
                                continue;
                            }

                            var projectId = root.TryGetProperty("project_id", out var pid) &&
                                            pid.ValueKind == JsonValueKind.String
                                ? pid.GetString()
                                : null;
                            var testerIds = ReadStringArray(root, "tester_ids");
                            if (string.IsNullOrEmpty(projectId) || testerIds.Count == 0)
                            {
                                continue;
                            }

                            await HandleSubscribeAsync(
                                scopeFactory, tqRegistry, rawRegistry, connection, user,
                                projectId, testerIds, logger, aborted);
                            break;
                        }

                        case TesterQueueMessageTypes.UnsubscribeTesterQueue:
                        {
                            if (!bucket.Allow())
                            {
                                continue;
                            }

                            // Tester ids only (no project) — resolve the owning
                            // project(s) from the shared registry, like the Rust
                            // handler filtering its per-connection sub map.
                            foreach (var testerId in ReadStringArray(root, "tester_ids"))
                            {
                                tqRegistry.RemoveConnectionFromTester(
                                    testerId, connection.Id, (proj, tester) =>
                                    {
                                        rawRegistry.UnsubscribeTesterGroup(
                                            TesterQueueRegistry.GroupName(proj, tester), connection.Id);
                                        return Task.CompletedTask;
                                    });
                            }
                            break;
                        }

                        default:
                            // Server→client variants (or junk) are not valid
                            // inbound — ignore, matching Rust.
                            break;
                    }
                }
                catch (JsonException)
                {
                    // Bad client message — ignore and keep reading (Rust: debug-log + continue).
                }
                catch (OperationCanceledException)
                {
                    break;
                }
                catch (Exception ex)
                {
                    // Subscribe failures (DB hiccup etc.) must not kill the socket.
                    logger.LogWarning(
                        ex, "raw ws: handling {Type} failed (conn {Conn})", type, connection.Id);
                }
            }
        }
        finally
        {
            tqRegistry.RemoveConnection(connection.Id);
            rawRegistry.RemoveTesterConnection(connection);
            connection.CompleteQueue();
            pumpCts.Cancel();
            try
            {
                await pump;
            }
            catch
            {
                // Pump exceptions already ejected the connection.
            }
            await RawWsIo.TryCloseAsync(socket);
            logger.LogDebug("raw ws: tester feed {Conn} disconnected", connection.Id);
        }
    }

    /// <summary>
    /// The Rust <c>handle_subscribe</c> / C# hub <c>SubscribeTesterQueue</c>
    /// pipeline against a fresh DI scope (this loop outlives any single request
    /// scope, so AuthRepository/NetworkerDbContext are resolved per message).
    /// </summary>
    private static async Task HandleSubscribeAsync(
        IServiceScopeFactory scopeFactory,
        TesterQueueRegistry tqRegistry,
        RawSocketRegistry rawRegistry,
        RawSocketConnection connection,
        AuthUser user,
        string projectId,
        IReadOnlyList<string> testerIds,
        ILogger logger,
        CancellationToken ct)
    {
        await using var scope = scopeFactory.CreateAsyncScope();
        var auth = scope.ServiceProvider.GetRequiredService<AuthRepository>();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();

        // 1. Project membership (platform admins bypass) — same check the hub
        //    and the Rust handler make.
        if (!user.IsPlatformAdmin)
        {
            var role = await auth.GetMemberRoleAsync(projectId, user.UserId, ct);
            if (role is null)
            {
                logger.LogWarning(
                    "raw ws: user {User} is not a member of project {Project}",
                    user.UserId, projectId);
                return;
            }
        }

        // 2. Tester ownership: keep only ids in project_tester for this project.
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
            .ToListAsync(ct);

        // 3. Register (shared cap + seq accounting) + raw fan-out group + snapshot.
        foreach (var testerId in validIds)
        {
            var testerKey = testerId.ToString();

            if (!tqRegistry.TrySubscribe(projectId, testerKey, connection.Id))
            {
                logger.LogWarning(
                    "raw ws: per-project subscription cap reached for project {Project}", projectId);
                continue;
            }

            rawRegistry.SubscribeTesterGroup(
                TesterQueueRegistry.GroupName(projectId, testerKey), connection);

            var snapshot = await BuildSnapshotAsync(db, tqRegistry, projectId, testerId, ct);
            // Flat {"type":"tester_queue_snapshot",...} — the record's own
            // JsonPropertyName contract; queued through the send pump so it
            // serializes with any concurrent live update.
            connection.TryEnqueue(JsonSerializer.Serialize(snapshot));
        }
    }

    /// <summary>
    /// Initial snapshot for a tester — the same running/queued query + 1-based
    /// positions + rolling-average ETA logic as <c>TesterQueueHub.BuildSnapshotAsync</c>
    /// and <c>TestersEndpoints.GetQueue</c> (duplicated here because the hub's
    /// builder is private and the hub file is frozen for this milestone; both
    /// read the same EF model, so shapes cannot drift independently of the DB).
    /// </summary>
    internal static async Task<TesterQueueSnapshotMessage> BuildSnapshotAsync(
        NetworkerDbContext db,
        TesterQueueRegistry registry,
        string projectId,
        Guid testerId,
        CancellationToken ct)
    {
        var seq = registry.NextSeq(projectId, testerId.ToString());
        var (running, queued) = await BuildQueueStateAsync(db, projectId, testerId, ct)
            .ConfigureAwait(false);

        return new TesterQueueSnapshotMessage(
            projectId,
            testerId.ToString(),
            seq,
            queued,
            running);
    }

    /// <summary>
    /// The seq-free core of the snapshot: current (running, queued) for a tester
    /// with 1-based positions + rolling-average ETAs. Shared by the snapshot
    /// above and by <see cref="TesterQueueUpdateProducer"/>, which pushes the
    /// same shape as a <c>tester_queue_update</c> delta on run transitions.
    /// </summary>
    internal static async Task<(TesterQueueEntry? Running, IReadOnlyList<TesterQueueEntry> Queued)>
        BuildQueueStateAsync(
            NetworkerDbContext db,
            string projectId,
            Guid testerId,
            CancellationToken ct)
    {
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

        for (var i = 0; i < queued.Count; i++)
        {
            var position = (uint)(i + 1);
            uint? etaSeconds = avgSecs.HasValue
                ? (uint)((long)i * avgSecs.Value)
                : null;
            queued[i] = queued[i] with { Position = position, EtaSeconds = etaSeconds };
        }

        return (running, queued);
    }

    private static List<string> ReadStringArray(JsonElement root, string property)
    {
        var result = new List<string>();
        if (root.TryGetProperty(property, out var arr) && arr.ValueKind == JsonValueKind.Array)
        {
            foreach (var item in arr.EnumerateArray())
            {
                if (item.ValueKind == JsonValueKind.String)
                {
                    var s = item.GetString();
                    if (!string.IsNullOrEmpty(s))
                    {
                        result.Add(s);
                    }
                }
            }
        }
        return result;
    }
}

/// <summary>
/// Sliding 60s-window rate limiter for subscribe/unsubscribe messages — one per
/// raw connection. The same semantics as the Rust <c>SubMsgBucket</c> and the
/// hub's private bucket: at most <c>cap</c> messages in any rolling 60 seconds.
/// Public (rather than nested/private) so the channel-independent logic is
/// directly unit-testable.
/// </summary>
public sealed class SlidingWindowRateLimiter(int cap, TimeSpan? window = null)
{
    public const int DefaultMaxSubMsgsPerMin = 10;

    private readonly Queue<DateTimeOffset> _stamps = new();
    private readonly TimeSpan _window = window ?? TimeSpan.FromSeconds(60);
    private readonly object _gate = new();

    /// <summary><c>DASHBOARD_MAX_SUB_MSGS_PER_MIN</c>, default 10 — the shared knob.</summary>
    public static int ResolveCapFromEnv()
    {
        var raw = Environment.GetEnvironmentVariable("DASHBOARD_MAX_SUB_MSGS_PER_MIN");
        return int.TryParse(raw, out var v) && v > 0 ? v : DefaultMaxSubMsgsPerMin;
    }

    public bool Allow()
    {
        var now = DateTimeOffset.UtcNow;
        var cutoff = now - _window;
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
