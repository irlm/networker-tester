using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Realtime.RawWs;

/// <summary>
/// The resolved identity of an api-key-authenticated agent — what both
/// transports stash per connection after a successful
/// <see cref="AgentMessageProcessor.AuthenticateAsync"/>.
/// </summary>
public sealed record AgentIdentity(Guid AgentId, string Name);

/// <summary>
/// Transport-agnostic core of the agent protocol — ALL of the per-message
/// persistence + event-bus logic that used to live inside
/// <see cref="AgentProtocolHub"/>'s method bodies, extracted so the raw
/// WebSocket endpoint (<see cref="AgentSocketEndpoint"/>, the transport the
/// fielded Rust agents actually speak) and the SignalR hub share one
/// implementation. The code was MOVED here verbatim from the hub (M2 slice 2),
/// not duplicated; the hub is now a thin shell over this class.
///
/// <para><b>Lifetime / DI.</b> Depends on the scoped
/// <see cref="NetworkerDbContext"/>, so an instance is only valid for one DI
/// scope: SignalR constructs one per hub-method invocation (the hub news it up
/// from its own scoped dependencies, so no extra service registration is
/// required for the existing Program.cs to keep working); the raw endpoint
/// creates a scope per inbound frame and resolves/activates one from it
/// (see <see cref="AgentSocketExtensions.AddAgentRawSocket"/>).</para>
///
/// <para><b>File location.</b> Lives under <c>Realtime/RawWs/</c> because this
/// milestone owns only that directory plus the two files it refactors; the
/// class itself is transport-neutral.</para>
/// </summary>
public sealed class AgentMessageProcessor
{
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
    private readonly ILogger<AgentMessageProcessor> _logger;

    public AgentMessageProcessor(
        NetworkerDbContext db,
        EventBus bus,
        ILogger<AgentMessageProcessor> logger)
    {
        _db = db;
        _bus = bus;
        _logger = logger;
    }

    // ── Frame codec (shared parse seam — also what the unit tests exercise) ──

    /// <summary>
    /// Decode one inbound <c>{"type":"...", ...}</c> WS text frame into the
    /// polymorphic <see cref="AgentMessage"/>. Returns <c>null</c> for
    /// undecodable frames and unknown/legacy-v1 type tags — the Rust hub drops
    /// both silently (<c>protocol::decode(...).ok()</c> + the
    /// <c>_ =&gt; trace!("Ignored legacy v1 agent message")</c> arm).
    /// </summary>
    public static AgentMessage? Decode(string frame)
    {
        try
        {
            return JsonSerializer.Deserialize<AgentMessage>(frame);
        }
        catch (JsonException)
        {
            return null;
        }
        catch (NotSupportedException)
        {
            // Unknown "type" discriminator (STJ polymorphism throws
            // NotSupportedException for unrecognised tags) — e.g. a legacy v1
            // variant like "job_ack". Rust ignores these; so do we.
            return null;
        }
    }

    /// <summary>
    /// Serialize one outbound <see cref="ControlMessage"/> to the flat
    /// <c>{"type":"...", ...}</c> envelope — byte-compatible with the WS text
    /// frame the Rust hub writes (<c>protocol::encode</c>).
    /// </summary>
    public static string EncodeControl(ControlMessage message)
        => JsonSerializer.Serialize(message);

    /// <summary>
    /// The <c>{"type":"welcome","agent_id":...,"agent_name":...}</c> frame sent
    /// on connect. Mirrors Rust <c>ControlMessage::Welcome</c>.
    /// </summary>
    public static string WelcomeFrame(Guid agentId, string agentName)
        => EncodeControl(new WelcomeMessage(agentId, agentName));

    // ── Connection lifecycle ─────────────────────────────────────────────────

    /// <summary>
    /// Validate an api-key against <c>agent.api_key</c> (the Rust
    /// <c>get_by_api_key</c> lookup in <c>agent_ws_handler</c>). Returns the
    /// agent's identity, or <c>null</c> when the key is missing/unknown — the
    /// caller rejects the connection (raw: HTTP 401 before upgrade; SignalR:
    /// <c>Context.Abort()</c>). Read-only: marking online is a separate step
    /// (<see cref="HandleConnectAsync"/>) because Rust performs it only after
    /// the upgrade completes.
    /// </summary>
    public async Task<AgentIdentity?> AuthenticateAsync(string? apiKey, CancellationToken ct = default)
    {
        if (string.IsNullOrEmpty(apiKey))
        {
            return null;
        }

        var agent = await _db.Agents
            .AsNoTracking()
            .FirstOrDefaultAsync(a => a.ApiKey == apiKey, ct);

        return agent is null ? null : new AgentIdentity(agent.AgentId, agent.Name);
    }

    /// <summary>
    /// Post-accept connect bookkeeping: mark the agent <c>online</c> + stamp
    /// <c>last_heartbeat</c>, and publish <see cref="AgentStatus"/>(online).
    /// Rust: <c>update_status("online")</c> + the <c>AgentStatus</c> event at
    /// the top of <c>handle_agent_socket</c>.
    /// </summary>
    public async Task HandleConnectAsync(Guid agentId, CancellationToken ct = default)
    {
        var now = DateTime.UtcNow;

        var agent = await _db.Agents
            .AsTracking()
            .FirstOrDefaultAsync(a => a.AgentId == agentId, ct);
        if (agent is not null)
        {
            agent.Status = "online";
            agent.LastHeartbeat = now;
            await _db.SaveChangesAsync(ct);
        }

        _bus.Publish(new AgentStatus(agentId, "online", now));
    }

    /// <summary>
    /// Disconnect cleanup shared by both transports: mark the agent
    /// <c>offline</c>, fail its orphaned runs, and publish
    /// <see cref="AgentStatus"/>(offline) — the Rust cleanup at the bottom of
    /// <c>handle_agent_socket</c>:
    /// <c>UPDATE test_run SET status='failed', error_message=…, finished_at=now()
    /// WHERE worker_id=&lt;agent_id&gt; AND status IN ('running','queued')</c>.
    /// Runs are matched by <c>worker_id</c> (the FK-free string recording the
    /// executing agent), NOT <c>tester_id</c> (a project_tester FK, not an agent
    /// id). The caller performs the registry unregister (compare-and-remove)
    /// BEFORE invoking this, since the registry op is connection-id-scoped.
    /// </summary>
    public async Task HandleDisconnectAsync(Guid agentId, CancellationToken ct = default)
    {
        var agent = await _db.Agents.AsTracking()
            .FirstOrDefaultAsync(a => a.AgentId == agentId, ct);
        if (agent is not null)
        {
            agent.Status = "offline";
            await _db.SaveChangesAsync(ct);
        }

        // Fail orphaned runs (running/queued) owned by this agent. Ownership is
        // keyed on worker_id (agent_id as text) — the reliable, FK-free key —
        // NOT tester_id (a project_tester FK). Set-based UPDATE.
        var workerId = agentId.ToString();
        var affected = await _db.TestRuns
            .Where(r => r.WorkerId == workerId
                && (r.Status == "running" || r.Status == "queued"))
            .ExecuteUpdateAsync(s => s
                .SetProperty(r => r.Status, "failed")
                .SetProperty(r => r.ErrorMessage, "Agent disconnected during execution")
                .SetProperty(r => r.FinishedAt, DateTime.UtcNow), ct);

        _bus.Publish(new AgentStatus(agentId, "offline", null));

        _logger.LogInformation(
            "Agent disconnected: {AgentId}; failed {Count} orphaned run(s)",
            agentId, affected);
    }

    // ── Inbound AgentMessage dispatch ────────────────────────────────────────

    /// <summary>
    /// Single inbound entry point: decode the raw <c>{"type":"...", ...}</c>
    /// frame and dispatch to the matching handler — mirroring the Rust
    /// <c>handle_agent_message</c> match. Unknown / undecodable frames are
    /// ignored (Rust drops decode failures and legacy v1 variants silently).
    /// </summary>
    public async Task HandleFrameAsync(Guid agentId, string frame, CancellationToken ct = default)
    {
        var msg = Decode(frame);
        if (msg is null)
        {
            _logger.LogDebug("Dropped undecodable agent frame from {AgentId}", agentId);
            return;
        }

        switch (msg)
        {
            case HeartbeatMessage hb:
                await OnHeartbeat(agentId, hb, ct);
                break;
            case RunStartedMessage rs:
                await OnRunStarted(agentId, rs, ct);
                break;
            case RunProgressMessage rp:
                await OnRunProgress(rp, ct);
                break;
            case AttemptEventMessage ae:
                await OnAttemptEvent(ae, ct);
                break;
            case RunFinishedMessage rf:
                await OnRunFinished(rf, ct);
                break;
            case ErrorMessage err:
                await OnError(agentId, err, ct);
                break;
            case CommandLogMessage cl:
                await OnCommandLog(cl, ct);
                break;
            case CommandResultMessage cr:
                await OnCommandResult(cr, ct);
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
    private async Task OnHeartbeat(Guid agentId, HeartbeatMessage hb, CancellationToken ct)
    {
        var agent = await _db.Agents.AsTracking().FirstOrDefaultAsync(a => a.AgentId == agentId, ct);
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
        await _db.SaveChangesAsync(ct);
    }

    /// <summary>
    /// RunStarted → <c>test_run.status='running'</c> + <c>started_at</c> +
    /// <c>worker_id=&lt;this agent&gt;</c> + <c>tester_id=&lt;agent.tester_id or
    /// null&gt;</c> + <c>last_heartbeat=now</c>; publish
    /// <see cref="JobUpdate"/>(running). Rust:
    /// <c>test_runs::update_status(Running)</c> + <c>JobUpdate</c>.
    /// <c>worker_id</c> (a nullable, FK-free string) records the EXECUTING agent
    /// — the reliable key the watchdog/disconnect cleanup use to map a run to its
    /// agent. <c>tester_id</c> is a project_tester FK, so it gets the tester the
    /// agent is BOUND to (<c>agent.tester_id</c>) — NULL for a standalone agent,
    /// and NEVER the agent_id (which would violate <c>test_run_tester_id_fkey</c>
    /// and 500 run_started persistence). Stamping <c>last_heartbeat</c> keeps a
    /// just-started run out of the 120s staleness window.
    /// </summary>
    private async Task OnRunStarted(Guid agentId, RunStartedMessage rs, CancellationToken ct)
    {
        // The project_tester the agent is bound to (may be null for a standalone
        // agent). NEVER the agent_id — that is not a valid project_tester FK.
        var boundTesterId = await _db.Agents
            .AsNoTracking()
            .Where(a => a.AgentId == agentId)
            .Select(a => a.TesterId)
            .FirstOrDefaultAsync(ct);
        var workerId = agentId.ToString();

        await _db.TestRuns
            .Where(r => r.Id == rs.RunId)
            .ExecuteUpdateAsync(s => s
                .SetProperty(r => r.Status, "running")
                .SetProperty(r => r.StartedAt, rs.StartedAt.UtcDateTime)
                .SetProperty(r => r.WorkerId, workerId)
                .SetProperty(r => r.TesterId, boundTesterId)
                .SetProperty(r => r.LastHeartbeat, DateTime.UtcNow), ct);

        _bus.Publish(new JobUpdate(rs.RunId, "running", agentId, rs.StartedAt, null));
    }

    /// <summary>
    /// RunProgress → update <c>test_run.success_count</c> / <c>failure_count</c>
    /// and refresh <c>last_heartbeat</c>. Rust: <c>test_runs::update_counts</c>
    /// (whose UPDATE also sets <c>last_heartbeat = now()</c> — the signal the
    /// stale-run watchdog keys on). No browser event (counts are read back into
    /// the terminal JobComplete), matching Rust.
    /// </summary>
    private async Task OnRunProgress(RunProgressMessage rp, CancellationToken ct)
    {
        await _db.TestRuns
            .Where(r => r.Id == rp.RunId)
            .ExecuteUpdateAsync(s => s
                .SetProperty(r => r.SuccessCount, rp.Success)
                .SetProperty(r => r.FailureCount, rp.Failure)
                .SetProperty(r => r.LastHeartbeat, DateTime.UtcNow), ct);
    }

    /// <summary>
    /// AttemptEvent → refresh <c>test_run.last_heartbeat</c> (each streamed
    /// attempt is proof of life, keeping long low-count runs out of the
    /// watchdog's 120s staleness window) and publish <see cref="AttemptResult"/>.
    /// Rust: <c>DashboardEvent::AttemptResult</c>. The opaque <c>attempt</c>
    /// JSON is forwarded verbatim.
    /// </summary>
    private async Task OnAttemptEvent(AttemptEventMessage ae, CancellationToken ct)
    {
        await _db.TestRuns
            .Where(r => r.Id == ae.RunId)
            .ExecuteUpdateAsync(s => s
                .SetProperty(r => r.LastHeartbeat, DateTime.UtcNow), ct);

        _bus.Publish(new AttemptResult(ae.RunId, ae.Attempt));
    }

    /// <summary>
    /// RunFinished → set terminal <c>test_run.status</c>, persist the benchmark
    /// artifact if present, read back the final counts, and publish
    /// <see cref="JobComplete"/>. Rust: <c>update_status</c> +
    /// <c>benchmark_artifacts::create</c> + read-back + <c>JobComplete</c>.
    /// </summary>
    private async Task OnRunFinished(RunFinishedMessage rf, CancellationToken ct)
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
                .SetProperty(r => r.FinishedAt, DateTime.UtcNow), ct);

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
                await _db.SaveChangesAsync(ct);

                // Link the run to its artifact (Rust persists it standalone;
                // stamping artifact_id keeps the FK navigable for readers).
                await _db.TestRuns
                    .Where(r => r.Id == rf.RunId)
                    .ExecuteUpdateAsync(s => s.SetProperty(r => r.ArtifactId, artifact.Id), ct);

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
            .FirstOrDefaultAsync(ct);

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
    private async Task OnError(Guid agentId, ErrorMessage err, CancellationToken ct)
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
                .SetProperty(r => r.FinishedAt, DateTime.UtcNow), ct);

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
    private async Task OnCommandLog(CommandLogMessage cl, CancellationToken ct)
    {
        await _db.AgentCommands
            .Where(c => c.CommandId == cl.CommandId && c.StartedAt == null)
            .ExecuteUpdateAsync(s => s.SetProperty(c => c.StartedAt, DateTime.UtcNow), ct);
    }

    /// <summary>
    /// CommandResult → mark the <c>agent_command</c> row terminal: set
    /// <c>status</c>, <c>result</c>, <c>error_message</c>, <c>finished_at</c>.
    /// Mirrors the Rust <c>mark_finished</c> half of <c>handle_command_result</c>.
    /// </summary>
    private async Task OnCommandResult(CommandResultMessage cr, CancellationToken ct)
    {
        var resultJson = cr.Result?.GetRawText();
        await _db.AgentCommands
            .Where(c => c.CommandId == cr.CommandId)
            .ExecuteUpdateAsync(s => s
                .SetProperty(c => c.Status, cr.Status)
                .SetProperty(c => c.Result, resultJson)
                .SetProperty(c => c.ErrorMessage, cr.Error)
                .SetProperty(c => c.FinishedAt, DateTime.UtcNow), ct);
    }
}
