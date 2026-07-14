using System.Text.Json;
using Microsoft.AspNetCore.SignalR;
using Microsoft.EntityFrameworkCore;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Realtime;

/// <summary>
/// Agent-facing protocol hub — the C# re-architecture of the Rust
/// <c>ws/agent_hub.rs</c>. Mapped (by Program.cs) at <c>/ws/agent</c>. Speaks
/// the WS-v2 TestConfig / TestRun protocol: connection lifecycle + inbound
/// <see cref="AgentMessage"/> dispatch (EF persistence + event-bus publish) +
/// orphan-run failing on disconnect.
///
/// <para><b>Named <c>AgentProtocolHub</c>, not <c>AgentHub</c></b>, on purpose:
/// a proof-of-concept <c>AgentHub</c> still lives in Program.cs and a duplicate
/// type name would not compile. The integrator removes the PoC hub and maps
/// this one (see <see cref="AgentProtocolExtensions"/>).</para>
///
/// <para><b>Authentication (api-key, NOT JWT).</b> Agents authenticate with
/// <c>?key=&lt;api_key&gt;</c> validated against <c>agent.api_key</c>, exactly
/// like the Rust <c>agent_ws_handler</c>. This hub therefore must NOT carry the
/// JWT <c>[Authorize]</c> attribute — it does its own key check in
/// <see cref="OnConnectedAsync"/> and aborts the connection when the key is
/// missing or unknown.</para>
///
/// <para><b>DI scope.</b> <see cref="NetworkerDbContext"/> is scoped; SignalR
/// creates a fresh DI scope per hub-method invocation, so injecting the context
/// via the constructor is safe (each call gets its own context instance, never
/// shared across concurrent frames). <see cref="EventBus"/> and
/// <see cref="AgentConnectionRegistry"/> are singletons.</para>
/// </summary>
public sealed class AgentProtocolHub : Hub
{
    /// <summary>Query-string key carrying the agent api-key (Rust: <c>?key=</c>).</summary>
    public const string ApiKeyQueryKey = "key";

    /// <summary>Per-connection item key under which the resolved agent id is stashed.</summary>
    private const string AgentIdItemKey = "agent_id";

    /// <summary>Per-connection item key under which the resolved agent name is stashed.</summary>
    private const string AgentNameItemKey = "agent_name";

    /// <summary>
    /// The canonical run statuses (Rust <c>RunStatus</c>, rename_all="lowercase").
    /// <see cref="OnRunFinished"/> validates the agent-reported terminal status
    /// against this set so an arbitrary/corrupt string never reaches the DB.
    /// </summary>
    private static readonly HashSet<string> AllowedRunStatuses = new(StringComparer.Ordinal)
    {
        "queued", "provisioning", "running", "completed", "failed", "cancelled",
    };

    private readonly NetworkerDbContext _db;
    private readonly EventBus _bus;
    private readonly AgentConnectionRegistry _registry;
    private readonly ILogger<AgentProtocolHub> _logger;

    public AgentProtocolHub(
        NetworkerDbContext db,
        EventBus bus,
        AgentConnectionRegistry registry,
        ILogger<AgentProtocolHub> logger)
    {
        _db = db;
        _bus = bus;
        _registry = registry;
        _logger = logger;
    }

    // ── Connection lifecycle ─────────────────────────────────────────────────

    /// <summary>
    /// Validate the <c>?key=</c> api-key against <c>agent.api_key</c>; abort the
    /// connection if it is missing or unknown (Rust returns 401 from the
    /// upgrade handler — SignalR's nearest equivalent is aborting the connection
    /// so no frames are ever processed). On success: register the connection,
    /// mark the agent <c>online</c> + stamp <c>last_heartbeat</c>, publish an
    /// <see cref="AgentStatus"/> event, and send <see cref="WelcomeMessage"/>.
    /// </summary>
    public override async Task OnConnectedAsync()
    {
        var http = Context.GetHttpContext();
        var apiKey = http?.Request.Query[ApiKeyQueryKey].ToString();

        if (string.IsNullOrEmpty(apiKey))
        {
            _logger.LogWarning("Agent connection {ConnId} rejected: no api key", Context.ConnectionId);
            Context.Abort();
            return;
        }

        var agent = await _db.Agents
            .AsTracking()
            .FirstOrDefaultAsync(a => a.ApiKey == apiKey, Context.ConnectionAborted);

        if (agent is null)
        {
            _logger.LogWarning("Agent connection {ConnId} rejected: unknown api key", Context.ConnectionId);
            Context.Abort();
            return;
        }

        var agentId = agent.AgentId;
        Context.Items[AgentIdItemKey] = agentId;
        Context.Items[AgentNameItemKey] = agent.Name;

        _logger.LogInformation(
            "Agent connected (v2): {AgentId} name={Name} conn={ConnId}",
            agentId, agent.Name, Context.ConnectionId);

        // Register connection so the dispatcher can push ControlMessages.
        _registry.Register(agentId, Context.ConnectionId);

        // Mark online + heartbeat (Rust: update_status "online").
        var now = DateTime.UtcNow;
        agent.Status = "online";
        agent.LastHeartbeat = now;
        await _db.SaveChangesAsync(Context.ConnectionAborted);

        _bus.Publish(new AgentStatus(agentId, "online", now));

        // Send Welcome as a native SignalR method (the one control message the
        // hub emits directly rather than through the registry's "message" push).
        await Clients.Caller.SendAsync(
            AgentConnectionRegistry.ClientReceiveMethod,
            JsonSerializer.Serialize<ControlMessage>(new WelcomeMessage(agentId, agent.Name)),
            Context.ConnectionAborted);

        await base.OnConnectedAsync();
    }

    /// <summary>
    /// Deregister the connection, mark the agent <c>offline</c>, publish an
    /// <see cref="AgentStatus"/>(offline) event, and fail the agent's orphaned
    /// runs — the SQL the Rust cleanup runs:
    /// <c>UPDATE test_run SET status='failed', error_message=…, finished_at=now()
    /// WHERE tester_id=&lt;agent_id&gt; AND status IN ('running','queued')</c>.
    /// </summary>
    public override async Task OnDisconnectedAsync(Exception? exception)
    {
        if (Context.Items.TryGetValue(AgentIdItemKey, out var raw) && raw is Guid agentId)
        {
            _registry.Unregister(agentId, Context.ConnectionId);

            var agent = await _db.Agents.AsTracking()
                .FirstOrDefaultAsync(a => a.AgentId == agentId);
            if (agent is not null)
            {
                agent.Status = "offline";
                await _db.SaveChangesAsync();
            }

            // Fail orphaned runs (running/queued) owned by this agent. Executed
            // as a set-based UPDATE — the direct analogue of the Rust SQL.
            var affected = await _db.TestRuns
                .Where(r => r.TesterId == agentId
                    && (r.Status == "running" || r.Status == "queued"))
                .ExecuteUpdateAsync(s => s
                    .SetProperty(r => r.Status, "failed")
                    .SetProperty(r => r.ErrorMessage, "Agent disconnected during execution")
                    .SetProperty(r => r.FinishedAt, DateTime.UtcNow));

            _bus.Publish(new AgentStatus(agentId, "offline", null));

            _logger.LogInformation(
                "Agent disconnected: {AgentId} conn={ConnId}; failed {Count} orphaned run(s)",
                agentId, Context.ConnectionId, affected);
        }

        await base.OnDisconnectedAsync(exception);
    }

    // ── Inbound AgentMessage handlers ────────────────────────────────────────
    //
    // The agent invokes ONE hub method, `Receive`, with the serialized
    // {"type":"...", ...} envelope — the same frame it sends the Rust hub as WS
    // text. This preserves the exact wire payload (no per-variant SignalR method
    // rename). `Receive` deserialises to the polymorphic AgentMessage and
    // dispatches, mirroring the Rust `handle_agent_message` match.

    /// <summary>
    /// Single inbound entry point: decode the raw <c>{"type":"...", ...}</c>
    /// frame and dispatch to the matching handler. Unknown / undecodable frames
    /// are ignored (Rust drops decode failures and legacy v1 variants silently).
    /// </summary>
    public async Task Receive(string frame)
    {
        AgentMessage? msg;
        try
        {
            msg = JsonSerializer.Deserialize<AgentMessage>(frame);
        }
        catch (JsonException ex)
        {
            _logger.LogDebug(ex, "Dropped undecodable agent frame from {ConnId}", Context.ConnectionId);
            return;
        }

        if (msg is null)
        {
            return;
        }

        var agentId = AgentId();

        switch (msg)
        {
            case HeartbeatMessage hb:
                await OnHeartbeat(agentId, hb);
                break;
            case RunStartedMessage rs:
                await OnRunStarted(agentId, rs);
                break;
            case RunProgressMessage rp:
                await OnRunProgress(rp);
                break;
            case AttemptEventMessage ae:
                await OnAttemptEvent(ae);
                break;
            case RunFinishedMessage rf:
                await OnRunFinished(rf);
                break;
            case ErrorMessage err:
                await OnError(agentId, err);
                break;
            case CommandLogMessage cl:
                await OnCommandLog(cl);
                break;
            case CommandResultMessage cr:
                await OnCommandResult(cr);
                break;
            default:
                _logger.LogDebug("Ignored agent message {Type}", msg.GetType().Name);
                break;
        }
    }

    /// <summary>
    /// Heartbeat → update <c>agent.last_heartbeat</c> (+ <c>version</c> if
    /// reported), keep <c>status='online'</c>. Rust: <c>update_heartbeat</c>.
    /// Publishes nothing on the DashboardEvent bus in Rust; here we mirror that
    /// (no per-heartbeat browser event — the M2 note that heartbeats publish
    /// <c>AgentStatus</c> is honoured by the connect/disconnect events, and a
    /// heartbeat AgentStatus would be a redundant flap, so it is omitted to stay
    /// byte-for-byte with the Rust bus output).
    /// </summary>
    private async Task OnHeartbeat(Guid agentId, HeartbeatMessage hb)
    {
        var agent = await _db.Agents.AsTracking().FirstOrDefaultAsync(a => a.AgentId == agentId);
        if (agent is null)
        {
            return;
        }

        agent.LastHeartbeat = DateTime.UtcNow;
        agent.Status = "online";
        if (!string.IsNullOrEmpty(hb.Version))
        {
            agent.Version = hb.Version;
        }
        await _db.SaveChangesAsync();
    }

    /// <summary>
    /// RunStarted → <c>test_run.status='running'</c> + <c>started_at</c> +
    /// <c>tester_id=&lt;this agent&gt;</c> + <c>last_heartbeat=now</c>; publish
    /// <see cref="JobUpdate"/>(running). Rust:
    /// <c>test_runs::update_status(Running)</c> + <c>JobUpdate</c>. Stamping
    /// <c>tester_id</c> with the EXECUTING agent's id is what lets the watchdog
    /// check the right agent's registry liveness and the disconnect cleanup find
    /// this run (<c>WHERE tester_id=$1</c>); stamping <c>last_heartbeat</c>
    /// keeps a just-started run out of the 120s staleness window.
    /// </summary>
    private async Task OnRunStarted(Guid agentId, RunStartedMessage rs)
    {
        await _db.TestRuns
            .Where(r => r.Id == rs.RunId)
            .ExecuteUpdateAsync(s => s
                .SetProperty(r => r.Status, "running")
                .SetProperty(r => r.StartedAt, rs.StartedAt.UtcDateTime)
                .SetProperty(r => r.TesterId, agentId)
                .SetProperty(r => r.LastHeartbeat, DateTime.UtcNow));

        _bus.Publish(new JobUpdate(rs.RunId, "running", agentId, rs.StartedAt, null));
    }

    /// <summary>
    /// RunProgress → update <c>test_run.success_count</c> / <c>failure_count</c>
    /// and refresh <c>last_heartbeat</c>. Rust: <c>test_runs::update_counts</c>
    /// (whose UPDATE also sets <c>last_heartbeat = now()</c> — the signal the
    /// stale-run watchdog keys on). No browser event (counts are read back into
    /// the terminal JobComplete), matching Rust.
    /// </summary>
    private async Task OnRunProgress(RunProgressMessage rp)
    {
        await _db.TestRuns
            .Where(r => r.Id == rp.RunId)
            .ExecuteUpdateAsync(s => s
                .SetProperty(r => r.SuccessCount, rp.Success)
                .SetProperty(r => r.FailureCount, rp.Failure)
                .SetProperty(r => r.LastHeartbeat, DateTime.UtcNow));
    }

    /// <summary>
    /// AttemptEvent → refresh <c>test_run.last_heartbeat</c> (each streamed
    /// attempt is proof of life, keeping long low-count runs out of the
    /// watchdog's 120s staleness window) and publish <see cref="AttemptResult"/>.
    /// Rust: <c>DashboardEvent::AttemptResult</c>. The opaque <c>attempt</c>
    /// JSON is forwarded verbatim.
    /// </summary>
    private async Task OnAttemptEvent(AttemptEventMessage ae)
    {
        await _db.TestRuns
            .Where(r => r.Id == ae.RunId)
            .ExecuteUpdateAsync(s => s
                .SetProperty(r => r.LastHeartbeat, DateTime.UtcNow));

        _bus.Publish(new AttemptResult(ae.RunId, ae.Attempt));
    }

    /// <summary>
    /// RunFinished → set terminal <c>test_run.status</c>, persist the benchmark
    /// artifact if present, read back the final counts, and publish
    /// <see cref="JobComplete"/>. Rust: <c>update_status</c> +
    /// <c>benchmark_artifacts::create</c> + read-back + <c>JobComplete</c>.
    /// </summary>
    private async Task OnRunFinished(RunFinishedMessage rf)
    {
        // Validate the agent-reported status against the canonical RunStatus set
        // — never write an arbitrary string into test_run.status (a corrupt or
        // hostile frame would otherwise poison every status-keyed query).
        if (string.IsNullOrEmpty(rf.Status) || !AllowedRunStatuses.Contains(rf.Status))
        {
            _logger.LogWarning(
                "Rejected run_finished for run {RunId}: invalid status '{Status}'",
                rf.RunId, rf.Status);
            return;
        }

        await _db.TestRuns
            .Where(r => r.Id == rf.RunId)
            .ExecuteUpdateAsync(s => s
                .SetProperty(r => r.Status, rf.Status)
                .SetProperty(r => r.FinishedAt, DateTime.UtcNow));

        if (rf.Artifact is { } art)
        {
            try
            {
                var artifact = new BenchmarkArtifact
                {
                    Id = Guid.NewGuid(),
                    TestRunId = rf.RunId,
                    Environment = art.Environment.GetRawText(),
                    Methodology = art.Methodology.GetRawText(),
                    Launches = art.Launches.GetRawText(),
                    Cases = art.Cases.GetRawText(),
                    Samples = art.Samples?.GetRawText(),
                    Summaries = art.Summaries.GetRawText(),
                    DataQuality = art.DataQuality.GetRawText(),
                    CreatedAt = DateTime.UtcNow,
                };
                _db.BenchmarkArtifacts.Add(artifact);
                await _db.SaveChangesAsync();

                // Link the run to its artifact (Rust persists it standalone;
                // stamping artifact_id keeps the FK navigable for readers).
                await _db.TestRuns
                    .Where(r => r.Id == rf.RunId)
                    .ExecuteUpdateAsync(s => s.SetProperty(r => r.ArtifactId, artifact.Id));

                _logger.LogInformation(
                    "Artifact {ArtifactId} persisted for run {RunId}", artifact.Id, rf.RunId);
            }
            catch (Exception ex)
            {
                // Rust logs and continues — the run status is already durable.
                _logger.LogError(ex, "Failed to persist artifact for run {RunId}", rf.RunId);
            }
        }

        // Read back the final counts for the complete event (Rust re-reads the
        // run row; defaults to (0,0) if it vanished).
        var counts = await _db.TestRuns
            .Where(r => r.Id == rf.RunId)
            .Select(r => new { r.SuccessCount, r.FailureCount })
            .FirstOrDefaultAsync();

        _bus.Publish(new JobComplete(
            rf.RunId, rf.RunId,
            counts?.SuccessCount ?? 0,
            counts?.FailureCount ?? 0));
    }

    /// <summary>
    /// Error → if a <c>run_id</c> is present, set <c>test_run.error_message</c> +
    /// <c>status='failed'</c> and publish <see cref="JobUpdate"/>(failed). Rust:
    /// <c>test_runs::set_error</c> + <c>JobUpdate</c>. A run-less error is logged
    /// only (matching Rust's <c>(Some(rid), …)</c> guard).
    /// </summary>
    private async Task OnError(Guid agentId, ErrorMessage err)
    {
        if (err.RunId is not { } runId)
        {
            _logger.LogError("Agent {AgentId} error (no run): {Message}", agentId, err.Message);
            return;
        }

        await _db.TestRuns
            .Where(r => r.Id == runId)
            .ExecuteUpdateAsync(s => s
                .SetProperty(r => r.Status, "failed")
                .SetProperty(r => r.ErrorMessage, err.Message)
                .SetProperty(r => r.FinishedAt, DateTime.UtcNow));

        _bus.Publish(new JobUpdate(runId, "failed", agentId, null, DateTimeOffset.UtcNow));
    }

    /// <summary>
    /// CommandLog → lazily stamp <c>agent_command.started_at</c> (idempotent:
    /// only when still null) — the first log line is the earliest evidence the
    /// command actually started. Mirrors the Rust <c>mark_started</c> half of
    /// <c>handle_command_log</c>.
    ///
    /// <para><b>Divergence:</b> the Rust handler also writes the log line to a
    /// <c>service_log</c> table (its ops-log DB). That table is not part of the
    /// EF model reused by this slice, so the line itself is not persisted here;
    /// the command-lifecycle stamp (the part that affects <c>agent_command</c>)
    /// is preserved. See the return note.</para>
    /// </summary>
    private async Task OnCommandLog(CommandLogMessage cl)
    {
        await _db.AgentCommands
            .Where(c => c.CommandId == cl.CommandId && c.StartedAt == null)
            .ExecuteUpdateAsync(s => s.SetProperty(c => c.StartedAt, DateTime.UtcNow));
    }

    /// <summary>
    /// CommandResult → mark the <c>agent_command</c> row terminal: set
    /// <c>status</c>, <c>result</c>, <c>error_message</c>, <c>finished_at</c>.
    /// Mirrors the Rust <c>mark_finished</c> half of <c>handle_command_result</c>.
    /// </summary>
    private async Task OnCommandResult(CommandResultMessage cr)
    {
        var resultJson = cr.Result?.GetRawText();
        await _db.AgentCommands
            .Where(c => c.CommandId == cr.CommandId)
            .ExecuteUpdateAsync(s => s
                .SetProperty(c => c.Status, cr.Status)
                .SetProperty(c => c.Result, resultJson)
                .SetProperty(c => c.ErrorMessage, cr.Error)
                .SetProperty(c => c.FinishedAt, DateTime.UtcNow));
    }

    /// <summary>
    /// The agent id resolved at connect time. Non-null for every inbound frame
    /// because <see cref="OnConnectedAsync"/> aborts unauthenticated connections
    /// before any hub method runs.
    /// </summary>
    private Guid AgentId()
        => Context.Items.TryGetValue(AgentIdItemKey, out var raw) && raw is Guid id
            ? id
            : Guid.Empty;
}
