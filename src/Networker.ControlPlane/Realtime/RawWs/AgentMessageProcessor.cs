using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Security;
using Npgsql;
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
    /// The terminal run statuses (subset of the canonical Rust <c>RunStatus</c>
    /// set, rename_all="lowercase"). Two duties (quality audit F6):
    /// <see cref="OnRunFinished"/> validates the agent-reported status against
    /// this set — a <c>run_finished</c> must carry a TERMINAL status, so an
    /// arbitrary/corrupt string (or a non-terminal one like <c>running</c>,
    /// which would resurrect the run) never reaches the DB; and the
    /// run-mutating handlers refuse to update a run that is ALREADY terminal —
    /// a late/duplicate frame from a slow agent must never flip
    /// <c>completed</c>→<c>failed</c> or <c>failed</c>→<c>running</c>. The
    /// per-handler <c>Where</c> preconditions spell the statuses out inline
    /// (<c>r.Status != "completed" …</c>) because <c>ExecuteUpdateAsync</c>
    /// needs a translatable predicate.
    /// </summary>
    private static readonly HashSet<string> TerminalRunStatuses = new(StringComparer.Ordinal)
    {
        "completed", "failed", "cancelled",
    };

    private readonly NetworkerDbContext _db;
    private readonly EventBus _bus;
    private readonly ILogger<AgentMessageProcessor> _logger;
    private readonly Alerting.AlertEvaluator? _alerts;
    private readonly Provisioning.BenchmarkRegressionDetector? _regressions;

    /// <param name="alerts">Optional so hosts/tests that don't wire the
    /// alerting module (<c>AddNetworkerAlerting</c>) keep working; when
    /// present, terminal runs are evaluated against the project's alert
    /// rules (best-effort — see <see cref="OnRunFinished"/>).</param>
    /// <param name="regressions">Optional for the same reason; when present,
    /// completed benchmark runs are compared against their baseline run and
    /// breaches persisted/broadcast (best-effort — see
    /// <see cref="OnRunFinished"/>).</param>
    public AgentMessageProcessor(
        NetworkerDbContext db,
        EventBus bus,
        ILogger<AgentMessageProcessor> logger,
        Alerting.AlertEvaluator? alerts = null,
        Provisioning.BenchmarkRegressionDetector? regressions = null)
    {
        _db = db;
        _bus = bus;
        _logger = logger;
        _alerts = alerts;
        _regressions = regressions;
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
    /// Throttle window for the <see cref="StampApiKeyUsedAsync"/> write: a
    /// successful auth only refreshes <c>api_key_last_used_at</c> when the last
    /// stamp is older than this, so heartbeat/reconnect churn never causes a hot
    /// write on every connection.
    /// </summary>
    public static readonly TimeSpan LastUsedThrottle = TimeSpan.FromMinutes(5);

    /// <summary>
    /// Validate an api-key (the Rust <c>get_by_api_key</c> lookup in
    /// <c>agent_ws_handler</c>). Returns the agent's identity, or <c>null</c>
    /// when the key is missing/unknown/EXPIRED — the caller rejects the
    /// connection (raw: HTTP 401 before upgrade; SignalR: <c>Context.Abort()</c>).
    /// Read-only: marking online is a separate step
    /// (<see cref="HandleConnectAsync"/>) because Rust performs it only after
    /// the upgrade completes; the last-used stamp is
    /// <see cref="StampApiKeyUsedAsync"/>, also called post-accept.
    ///
    /// <para><b>Security (V040):</b> the lookup is keyed on
    /// <c>agent.api_key_hash</c> (SHA-256 of the presented key), never the
    /// plaintext column, so the database's non-constant-time string equality
    /// runs over digests an attacker cannot incrementally control; the digest
    /// is then re-verified in-process with
    /// <see cref="AgentApiKeys.FixedTimeEqualsHex"/>. Rows without a hash
    /// (pre-V040, impossible after the backfill) never match.</para>
    ///
    /// <para><b>Expiry (V044):</b> after the hash match, a non-null
    /// <c>api_key_expires_at</c> in the past rejects the key (returns null).
    /// NULL = no expiry — every fielded agent authenticates unchanged; rotation
    /// defaults to no-expiry so a rotated key never breaks the fleet.</para>
    /// </summary>
    public async Task<AgentIdentity?> AuthenticateAsync(string? apiKey, CancellationToken ct = default)
    {
        if (string.IsNullOrEmpty(apiKey))
        {
            return null;
        }

        var presentedHash = AgentApiKeys.HashHex(apiKey);

        var agent = await _db.Agents
            .AsNoTracking()
            .Where(a => a.ApiKeyHash == presentedHash)
            .Select(a => new { a.AgentId, a.Name, a.ApiKeyHash, a.ApiKeyExpiresAt })
            .FirstOrDefaultAsync(ct);

        if (agent is null
            || !AgentApiKeys.FixedTimeEqualsHex(agent.ApiKeyHash, presentedHash))
        {
            return null;
        }

        // V044: reject an expired key (null = no expiry, back-compat).
        if (agent.ApiKeyExpiresAt is { } expiry && expiry <= DateTime.UtcNow)
        {
            _logger.LogWarning(
                "Agent {AgentId} api-key rejected: expired at {Expiry:o}", agent.AgentId, expiry);
            return null;
        }

        return new AgentIdentity(agent.AgentId, agent.Name);
    }

    /// <summary>
    /// Stamp <c>api_key_last_used_at</c> / <c>api_key_last_used_ip</c> after a
    /// successful auth (V044) — the "last seen" audit signal the UI surfaces.
    /// The write is THROTTLED: it only fires when the existing stamp is null or
    /// older than <see cref="LastUsedThrottle"/>, so a busy agent that
    /// heartbeats/reconnects frequently does not turn every connection into a
    /// row write. Best-effort — a transient failure never blocks the connection.
    /// </summary>
    public async Task StampApiKeyUsedAsync(Guid agentId, string? remoteIp, CancellationToken ct = default)
    {
        try
        {
            var now = DateTime.UtcNow;
            var cutoff = now - LastUsedThrottle;
            // Single set-based UPDATE guarded by the throttle window — no
            // read-modify-write race, and a no-op when recently stamped.
            await _db.Agents
                .Where(a => a.AgentId == agentId
                    && (a.ApiKeyLastUsedAt == null || a.ApiKeyLastUsedAt < cutoff))
                .ExecuteUpdateAsync(s => s
                    .SetProperty(a => a.ApiKeyLastUsedAt, now)
                    .SetProperty(a => a.ApiKeyLastUsedIp, remoteIp), ct);
        }
        catch (Exception ex)
        {
            _logger.LogDebug(ex, "last-used stamp failed for agent {AgentId}", agentId);
        }
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

            // E2E P2-7: nothing ever wrote project_tester.installer_version /
            // last_installed_at, so runner selectors showed "v?" for live,
            // connected runners. The agent IS the installed software — write its
            // reported version through to the bound tester. The guarded
            // predicate (version differs) keeps the steady-state heartbeat
            // write-free and moves last_installed_at only on a real
            // install/upgrade.
            if (agent.TesterId is { } boundTesterId)
            {
                await _db.ProjectTesters
                    .Where(t => t.TesterId == boundTesterId && t.InstallerVersion != hb.Version)
                    .ExecuteUpdateAsync(s => s
                        .SetProperty(t => t.InstallerVersion, hb.Version)
                        .SetProperty(t => t.LastInstalledAt, DateTime.UtcNow)
                        .SetProperty(t => t.UpdatedAt, DateTime.UtcNow), ct);
            }
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

        // Status precondition (audit F6): a late run_started — e.g. arriving
        // after a cancel or a watchdog fail — must not resurrect a terminal run
        // (failed→running would leave a "running" row with no live owner).
        var updated = await _db.TestRuns
            .Where(r => r.Id == rs.RunId
                && r.Status != "completed" && r.Status != "failed" && r.Status != "cancelled")
            .ExecuteUpdateAsync(s => s
                .SetProperty(r => r.Status, "running")
                .SetProperty(r => r.StartedAt, rs.StartedAt.UtcDateTime)
                .SetProperty(r => r.WorkerId, workerId)
                .SetProperty(r => r.TesterId, boundTesterId)
                .SetProperty(r => r.LastHeartbeat, DateTime.UtcNow), ct);

        if (updated == 0)
        {
            _logger.LogWarning(
                "Ignored run_started from agent {AgentId} for run {RunId}: run is missing or already terminal",
                agentId, rs.RunId);
            return;
        }

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

        // Persist the attempt into the tester-owned V001 schema so the run's
        // /attempts route + the reports have data (E2E P0-2: the C# agent
        // relays attempts but nothing wrote them, leaving success_count=60 runs
        // with an empty attempts list). Non-Postgres connections (SQLite in
        // unit tests) and a missing probe schema are skipped, not thrown.
        if (AttemptExtract.Parse(ae.RunId, ae.Attempt) is { } parsed
            && _db.Database.GetDbConnection() is NpgsqlConnection conn)
        {
            try
            {
                await AttemptPersister.PersistAsync(conn, parsed, ct);
            }
            catch (Exception ex)
            {
                // Never let a persistence hiccup drop the live stream / heartbeat.
                _logger.LogWarning(ex,
                    "Failed to persist attempt {AttemptId} for run {RunId}", parsed.AttemptId, ae.RunId);
            }
        }

        _bus.Publish(new AttemptResult(ae.RunId, ae.Attempt));
    }

    /// <summary>
    /// The run-envelope members accepted from a <c>run_finished</c> frame —
    /// the server-side twin of the agent's allowlist
    /// (<c>RunExecutor.RunEnvelopeFields</c>): the run-scoped context fields
    /// of the tester's TestRun JSON. Anything else in a received envelope is
    /// dropped on ingest (defence in depth against a hostile or skewed agent
    /// stuffing arbitrary payloads into <c>test_run.client_envelope</c>).
    /// </summary>
    private static readonly string[] RunEnvelopeFields =
    [
        "client_network", "client_geo", "target_geo",
        "client_load_before", "client_load_after", "clock_sync",
        "client_info", "server_info",
    ];

    /// <summary>
    /// Filter a received <c>run_finished.envelope</c> through
    /// <see cref="RunEnvelopeFields"/> and re-serialize the surviving members
    /// as one compact JSON object (snake_case preserved — values pass through
    /// verbatim). Returns <c>null</c> for a missing/non-object envelope or one
    /// with no allowed members, so old agents (which never send the field) and
    /// junk payloads both leave <c>client_envelope</c> NULL.
    /// </summary>
    internal static string? ExtractEnvelopeJson(JsonElement? envelope)
    {
        if (envelope is not { ValueKind: JsonValueKind.Object } env)
        {
            return null;
        }

        using var buffer = new MemoryStream();
        var any = false;
        using (var writer = new Utf8JsonWriter(buffer))
        {
            writer.WriteStartObject();
            foreach (var field in RunEnvelopeFields)
            {
                if (env.TryGetProperty(field, out var value)
                    && value.ValueKind is not JsonValueKind.Null and not JsonValueKind.Undefined)
                {
                    writer.WritePropertyName(field);
                    value.WriteTo(writer);
                    any = true;
                }
            }
            writer.WriteEndObject();
        }

        return any ? System.Text.Encoding.UTF8.GetString(buffer.ToArray()) : null;
    }

    /// <summary>
    /// RunFinished → set terminal <c>test_run.status</c> (+ the run envelope
    /// when the agent sent one), persist the benchmark artifact if present,
    /// read back the final counts, and publish <see cref="JobComplete"/>.
    /// Rust: <c>update_status</c> + <c>benchmark_artifacts::create</c> +
    /// read-back + <c>JobComplete</c>.
    /// </summary>
    private async Task OnRunFinished(RunFinishedMessage rf, CancellationToken ct)
    {
        // Validate the agent-reported status: a run_finished must carry a
        // TERMINAL status (audit F6) — never write an arbitrary string into
        // test_run.status (a corrupt or hostile frame would otherwise poison
        // every status-keyed query), and never a non-terminal one like
        // "running"/"queued" (which would resurrect the run).
        if (string.IsNullOrEmpty(rf.Status) || !TerminalRunStatuses.Contains(rf.Status))
        {
            _logger.LogWarning(
                "Rejected run_finished for run {RunId}: invalid or non-terminal status '{Status}'",
                rf.RunId, rf.Status);
            return;
        }

        // Run envelope (v0.28.80): allowlist-filtered pass-through of the
        // tester's run-scoped context. Null when the agent predates the field
        // or the tester emitted none — the column simply stays NULL, and
        // setting it unconditionally is safe because the terminal-status
        // precondition below guarantees this row has never carried one.
        var envelopeJson = ExtractEnvelopeJson(rf.Envelope);

        // Status precondition (audit F6): a late/duplicate run_finished must
        // not rewrite a run that already reached a terminal state (e.g. a
        // "failed" frame arriving after a cancel flipping cancelled→failed).
        var updated = await _db.TestRuns
            .Where(r => r.Id == rf.RunId
                && r.Status != "completed" && r.Status != "failed" && r.Status != "cancelled")
            .ExecuteUpdateAsync(s => s
                .SetProperty(r => r.Status, rf.Status)
                .SetProperty(r => r.ClientEnvelope, envelopeJson)
                .SetProperty(r => r.FinishedAt, DateTime.UtcNow), ct);

        if (updated == 0)
        {
            _logger.LogWarning(
                "Ignored run_finished ({Status}) for run {RunId}: run is missing or already terminal",
                rf.Status, rf.RunId);
            return;
        }

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

        // Regression hook: compare this completed benchmark run's per-case
        // stats against its baseline run and persist/broadcast breaches
        // (M5/A12/G2 — the policy the Benchmark Regressions page documents).
        // DetectAsync catches its own exceptions (log-and-continue) and
        // no-ops when the artifact failed to persist above.
        if (_regressions is not null && rf.Status == "completed" && rf.Artifact is not null)
        {
            await _regressions.DetectAsync(rf.RunId, ct);
        }

        // Alerting hook: evaluate the project's rules against this now-terminal
        // run. EvaluateRunAsync catches its own exceptions (log-and-continue)
        // — alerting is best-effort and must never fail run processing.
        if (_alerts is not null)
        {
            await _alerts.EvaluateRunAsync(rf.RunId, ct);
        }
    }

    /// <summary>
    /// Error → if a <c>run_id</c> is present, set <c>test_run.error_message</c> +
    /// <c>status='failed'</c> and publish <see cref="JobUpdate"/>(failed). Rust:
    /// <c>test_runs::set_error</c> + <c>JobUpdate</c>. A run-less error is logged
    /// only (matching Rust's <c>(Some(rid), …)</c> guard).
    ///
    /// <para>The message is ANSI-scrubbed before persistence: the agent relays
    /// tester stderr verbatim, which carries SGR color codes (audit F8) —
    /// stored <c>error_message</c> must be clean for every consumer, not just
    /// the frontend's display-side strip.</para>
    /// </summary>
    private async Task OnError(Guid agentId, ErrorMessage err, CancellationToken ct)
    {
        if (err.RunId is not { } runId)
        {
            _logger.LogError("Agent {AgentId} error (no run): {Message}", agentId, err.Message);
            return;
        }

        var cleanMessage = AnsiText.Strip(err.Message);

        // Relayed subprocess output (RunExecutor step 5) arrives prefixed
        // "[tester] " / "[tester/<workload>] " — it is LOG STREAMING, not a
        // verdict. The tester's tracing subscriber writes INFO to stderr, so
        // treating these as fatal failed every healthy run ~10ms after spawn
        // (E2E pass 2026-07-28, P0-1: a 60/60-success run marked 'failed' with
        // the startup INFO line as its "error"). Only the agent's own
        // unprefixed critical frames (spawn failure, deadline kill, stdout
        // overflow, unparseable JSON) decide status here; the real terminal
        // verdict travels in run_finished.
        if (IsRelayedSubprocessOutput(cleanMessage))
        {
            _logger.LogInformation(
                "Agent {AgentId} run {RunId} stderr: {Message}", agentId, runId, cleanMessage);
            return;
        }

        // Status precondition (audit F6): a late error frame — the agent
        // streams tester stderr as error frames, which can trail the terminal
        // run_finished — must never flip an already-terminal run (a successful
        // "completed" run rewritten to "failed" by a stderr line).
        var updated = await _db.TestRuns
            .Where(r => r.Id == runId
                && r.Status != "completed" && r.Status != "failed" && r.Status != "cancelled")
            .ExecuteUpdateAsync(s => s
                .SetProperty(r => r.Status, "failed")
                .SetProperty(r => r.ErrorMessage, cleanMessage)
                .SetProperty(r => r.FinishedAt, DateTime.UtcNow), ct);

        if (updated == 0)
        {
            _logger.LogDebug(
                "Ignored error frame from agent {AgentId} for run {RunId}: run is missing or already terminal ({Message})",
                agentId, runId, cleanMessage);
            return;
        }

        _bus.Publish(new JobUpdate(runId, "failed", agentId, null, DateTimeOffset.UtcNow));
    }

    /// <summary>
    /// True for error frames that are relayed child-process output rather than
    /// an agent verdict: the agent labels every relayed stderr line
    /// <c>[tester] …</c> (or <c>[tester/&lt;workload&gt;] …</c> in benchmark
    /// mode) — see <c>Networker.Agent.RunExecutor</c> step 5. The agent's own
    /// fatal messages are plain prose and never carry the bracket label.
    /// </summary>
    internal static bool IsRelayedSubprocessOutput(string message) =>
        message.StartsWith("[tester] ", StringComparison.Ordinal)
        || message.StartsWith("[tester/", StringComparison.Ordinal);

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
    /// The error text is ANSI-scrubbed on ingest, same as
    /// <see cref="OnError"/> (agent-relayed process output carries SGR codes).
    /// </summary>
    private async Task OnCommandResult(CommandResultMessage cr, CancellationToken ct)
    {
        var resultJson = cr.Result?.GetRawText();
        var cleanError = AnsiText.Strip(cr.Error);
        await _db.AgentCommands
            .Where(c => c.CommandId == cr.CommandId)
            .ExecuteUpdateAsync(s => s
                .SetProperty(c => c.Status, cr.Status)
                .SetProperty(c => c.Result, resultJson)
                .SetProperty(c => c.ErrorMessage, cleanError)
                .SetProperty(c => c.FinishedAt, DateTime.UtcNow), ct);
    }
}
