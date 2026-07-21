using System.Text.Json;
using System.Text.Json.Nodes;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Endpoints;
using Networker.ControlPlane.Realtime;
using Networker.Data;
using Networker.Security;

namespace Networker.ControlPlane.Dispatch;

/// <summary>
/// Default <see cref="IRunDispatcher"/> — EF Core + <see cref="AgentConnectionRegistry"/>
/// re-architecture of the Rust launch/dispatch/cancel/redispatch flow. See
/// <see cref="IRunDispatcher"/> for the Rust source map.
///
/// <para>Registered scoped (see <see cref="DispatchServiceCollectionExtensions"/>)
/// because it takes a scoped <see cref="NetworkerDbContext"/>. The
/// <see cref="AgentConnectionRegistry"/> and <see cref="EventBus"/> it depends on
/// are singletons, so a scoped consumer is safe.</para>
/// </summary>
public sealed class RunDispatcher : IRunDispatcher
{
    // The canonical wire status strings (Rust RunStatus is rename_all="lowercase").
    private const string StatusQueued = "queued";
    private const string StatusRunning = "running";
    private const string StatusCancelled = "cancelled";

    // The endpoint_kind column value for a Pending endpoint (deferred to M4).
    // Matches the Rust EndpointRef::Pending => "pending".
    private const string EndpointKindPending = "pending";

    // The endpoint_kind column value for a Proxy endpoint — resolved to a
    // concrete Network{host,port} at dispatch time (Rust resolve_proxy_endpoint).
    private const string EndpointKindProxy = "proxy";

    // Minimum age (seconds) before the redispatcher considers a queued run, so we
    // don't race the inline dispatch that runs synchronously inside LaunchAsync.
    // Mirrors the Rust scheduler's QUEUED_MIN_AGE_SECS.
    private const int QueuedMinAgeSecs = 15;

    // Cap on how many queued runs a single redispatch pass will try — bounds the
    // work each tick under a pathological backlog. Mirrors QUEUED_REDISPATCH_LIMIT.
    private const int QueuedRedispatchLimit = 50;

    private readonly NetworkerDbContext _db;
    private readonly AgentConnectionRegistry _agents;
    private readonly EventBus _bus;
    private readonly ILogger<RunDispatcher> _logger;
    private readonly CredentialCipher _cipher;

    public RunDispatcher(
        NetworkerDbContext db,
        AgentConnectionRegistry agents,
        EventBus bus,
        ILogger<RunDispatcher> logger,
        CredentialCipher cipher)
    {
        _db = db;
        _agents = agents;
        _bus = bus;
        _logger = logger;
        _cipher = cipher;
    }

    /// <inheritdoc />
    public async Task<Guid> LaunchAsync(
        Guid testConfigId,
        Guid? comparisonGroupId,
        Guid? testerId,
        AuthUser caller,
        CancellationToken ct)
    {
        var cfg = await _db.TestConfigs
            .AsNoTracking()
            .FirstOrDefaultAsync(c => c.Id == testConfigId, ct)
            ?? throw new RunDispatchNotFoundException($"test_config {testConfigId} not found");

        var now = DateTime.UtcNow;
        var run = new Data.Entities.TestRun
        {
            Id = Guid.NewGuid(),
            TestConfigId = cfg.Id,
            ProjectId = cfg.ProjectId,
            Status = StatusQueued,
            SuccessCount = 0,
            FailureCount = 0,
            CreatedAt = now,
            ComparisonGroupId = comparisonGroupId,
            // test_run.tester_id is a FK to project_tester(tester_id) — the
            // PROJECT-TESTER (the tester/VM a run targets/executes on), NEVER an
            // agent_id. When the caller pins a tester up-front
            // (LaunchRequest.tester_id) we seed it so dispatch prefers the agent
            // BOUND to that tester (agent.tester_id == run.tester_id). The
            // executing agent's own id is recorded separately in worker_id (a
            // nullable, FK-free string) after a successful assignment.
            TesterId = testerId,
        };

        _db.TestRuns.Add(run);
        await _db.SaveChangesAsync(ct);

        _logger.LogInformation(
            "Launched test_run {RunId} for config {ConfigId} ({ConfigName}) by {UserId}",
            run.Id, cfg.Id, cfg.Name, caller.UserId);

        // Best-effort dispatch (or defer to provisioning). Never blocks the launch
        // response on agent availability — a queued run is a valid outcome.
        await DispatchAsync(run.Id, ct);

        return run.Id;
    }

    /// <inheritdoc />
    public async Task DispatchAsync(Guid runId, CancellationToken ct)
    {
        var run = await _db.TestRuns
            .AsNoTracking()
            .FirstOrDefaultAsync(r => r.Id == runId, ct);
        if (run is null)
        {
            _logger.LogWarning("DispatchAsync: run {RunId} not found", runId);
            return;
        }

        var cfg = await _db.TestConfigs
            .AsNoTracking()
            .FirstOrDefaultAsync(c => c.Id == run.TestConfigId, ct);
        if (cfg is null)
        {
            _logger.LogWarning(
                "DispatchAsync: run {RunId} references missing test_config {ConfigId}",
                run.Id, run.TestConfigId);
            return;
        }

        // ── M4 deferral: Pending endpoints provision their own VM. ───────────
        // The Rust dispatch_or_provision kicks a deployment and flips the run to
        // `provisioning`. The provisioning orchestrator owns these runs; the
        // redispatcher explicitly skips Pending runs.
        if (string.Equals(cfg.EndpointKind, EndpointKindPending, StringComparison.OrdinalIgnoreCase))
        {
            _logger.LogInformation(
                "Run {RunId} has a Pending endpoint — leaving queued for the provisioning orchestrator",
                run.Id);
            return;
        }

        await TryAssignAsync(run, cfg, ct);
    }

    /// <inheritdoc />
    public async Task<int> RedispatchQueuedAsync(CancellationToken ct)
    {
        // Nothing to do if no agent is online — every attempt would no-op.
        if (_agents.AnyOnlineAgent() is null)
        {
            return 0;
        }

        var cutoff = DateTime.UtcNow.AddSeconds(-QueuedMinAgeSecs);

        // Candidate queued runs old enough that the inline launch dispatch has
        // certainly finished. Join the config so we can skip Pending endpoints
        // without a second round-trip.
        var candidates = await _db.TestRuns
            .AsNoTracking()
            .Where(r => r.Status == StatusQueued && r.CreatedAt < cutoff)
            .OrderBy(r => r.CreatedAt)
            .Take(QueuedRedispatchLimit)
            .Select(r => new { Run = r, r.TestConfig.EndpointKind })
            .ToListAsync(ct);

        if (candidates.Count == 0)
        {
            return 0;
        }

        var dispatched = 0;
        foreach (var candidate in candidates)
        {
            ct.ThrowIfCancellationRequested();

            // M4 owns Pending runs — the provisioning orchestrator hands them off
            // once the deployment completes; don't disturb them here.
            if (string.Equals(candidate.EndpointKind, EndpointKindPending, StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }

            var run = candidate.Run;
            var cfg = await _db.TestConfigs
                .AsNoTracking()
                .FirstOrDefaultAsync(c => c.Id == run.TestConfigId, ct);
            if (cfg is null)
            {
                continue;
            }

            if (await TryAssignAsync(run, cfg, ct))
            {
                dispatched++;
                _logger.LogInformation(
                    "Redispatched previously-queued run {RunId}", run.Id);
            }
        }

        if (dispatched > 0)
        {
            _logger.LogInformation("Redispatched {Count} previously-queued runs", dispatched);
        }

        return dispatched;
    }

    /// <inheritdoc />
    public async Task CancelAsync(Guid runId, CancellationToken ct)
    {
        // Set status → cancelled. Mirrors the Rust test_runs::update_status(Cancelled).
        var affected = await _db.TestRuns
            .Where(r => r.Id == runId)
            .ExecuteUpdateAsync(s => s.SetProperty(r => r.Status, StatusCancelled), ct);

        if (affected == 0)
        {
            throw new RunDispatchNotFoundException($"test_run {runId} not found");
        }

        // Fan-out cancel to the owning agent if we know it, else any online agent
        // (mirrors the Rust cancel_handler which sends to any online agent).
        var run = await _db.TestRuns
            .AsNoTracking()
            .FirstOrDefaultAsync(r => r.Id == runId, ct);

        var targetAgentId = run is not null
            ? await SelectTargetAgentAsync(run.TesterId, ct)
            : _agents.AnyOnlineAgent();

        if (targetAgentId is Guid agentId)
        {
            await _agents.CancelRunAsync(agentId, runId, ct);
        }

        _logger.LogInformation("Cancelled test_run {RunId}", runId);
        _bus.Publish(new JobUpdate(runId, StatusCancelled, targetAgentId, null, DateTimeOffset.UtcNow));
    }

    // ── Core assignment ──────────────────────────────────────────────────────

    /// <summary>
    /// Select a target agent, serialize the run + (proxy-resolved) config, send
    /// <c>assign_run</c>, and — on success — stamp the executing agent's identity
    /// FK-safely: <c>test_run.worker_id = agentId</c> (a nullable, FK-free string
    /// — the reliable key the watchdog/cancel/disconnect paths use to find the
    /// owner) and <c>test_run.tester_id = agent.tester_id</c> (the project_tester
    /// the agent is bound to, which may be NULL for a standalone agent). The
    /// agent_id is NEVER written into tester_id (it is not a valid project_tester
    /// id and would violate <c>test_run_tester_id_fkey</c>). Shared by the inline
    /// dispatch and the periodic redispatcher (the C# analogue of the Rust
    /// <c>try_dispatch_run</c>).
    /// </summary>
    private async Task<bool> TryAssignAsync(
        Data.Entities.TestRun run,
        Data.Entities.TestConfig cfg,
        CancellationToken ct)
    {
        var targetAgentId = await SelectTargetAgentAsync(run.TesterId, ct);
        if (targetAgentId is null)
        {
            // No compatible agent online — leave queued; the redispatcher retries.
            _logger.LogDebug(
                "Run {RunId} has no compatible online agent (min version {MinVersion}) — remains queued for later dispatch",
                run.Id, AgentVersionGate.MinAssignRunVersionString);
            return false;
        }

        var agentId = targetAgentId.Value;
        var (runJson, configJson) = await SerializeForAssignAsync(run, cfg, ct);

        var sent = await _agents.AssignRunAsync(agentId, runJson, configJson, ct);
        if (!sent)
        {
            // Send failed (agent raced offline) — leave queued, redispatcher retries.
            _logger.LogWarning(
                "Dispatch to agent {AgentId} failed for run {RunId} — will retry",
                agentId, run.Id);
            return false;
        }

        // Stamp the executing agent's identity FK-safely. worker_id (nullable,
        // no FK) ALWAYS records the agent id as text — this is the reliable key
        // the watchdog / disconnect orphan-fail use to map a run to its agent.
        // tester_id is a project_tester FK, so it gets the tester the agent is
        // BOUND to (agent.tester_id) — which is NULL for a standalone agent, in
        // which case we leave it null (NEVER the agent_id). Guarded so a run
        // that already reached a terminal state is never clobbered.
        var boundTesterId = await _db.Agents
            .AsNoTracking()
            .Where(a => a.AgentId == agentId)
            .Select(a => a.TesterId)
            .FirstOrDefaultAsync(ct);
        var workerId = agentId.ToString();
        await _db.TestRuns
            .Where(r => r.Id == run.Id && (r.Status == StatusQueued || r.Status == StatusRunning))
            .ExecuteUpdateAsync(s => s
                .SetProperty(r => r.WorkerId, workerId)
                .SetProperty(r => r.TesterId, boundTesterId), ct);

        _logger.LogInformation(
            "Dispatched run {RunId} to agent {AgentId} (endpoint_kind={Kind})",
            run.Id, agentId, cfg.EndpointKind);

        // Publish a JobUpdate so the browser bus reflects the assignment.
        // Status stays `queued` on the DB until the agent sends RunStarted;
        // this event carries the assigned agent id for the live UI.
        _bus.Publish(new JobUpdate(run.Id, StatusQueued, agentId, null, null));
        return true;
    }

    // ── Target-agent selection ───────────────────────────────────────────────

    /// <summary>
    /// Pick the target agent for a run. <paramref name="preferredTesterId"/> is
    /// the run's <c>tester_id</c>, a PROJECT-TESTER id (FK to
    /// <c>project_tester</c>): if an online, version-compatible agent is BOUND to
    /// that tester (<c>agent.tester_id == run.tester_id</c>) it wins (tester
    /// affinity); otherwise fall back to any online agent whose reported
    /// <c>agent.version</c> parses and is ≥ 0.28.0 — older agents silently drop
    /// <c>assign_run</c> (the Rust <c>any_online_agent_min_version</c> gate,
    /// MIN_AGENT_VERSION_FOR_ASSIGN_RUN). Returns null when no compatible agent
    /// is connected.
    /// </summary>
    private async Task<Guid?> SelectTargetAgentAsync(Guid? preferredTesterId, CancellationToken ct)
    {
        var online = _agents.OnlineAgents();
        if (online.Count == 0)
        {
            return null;
        }

        var onlineIds = online.ToHashSet();
        var rows = await _db.Agents
            .AsNoTracking()
            .Where(a => onlineIds.Contains(a.AgentId))
            .Select(a => new { a.AgentId, a.Version, a.TesterId })
            .ToListAsync(ct);

        var compatible = rows
            .Where(a => AgentVersionGate.IsCompatible(a.Version))
            .ToList();

        if (compatible.Count == 0)
        {
            return null;
        }

        // Tester affinity: prefer the agent BOUND to the requested project_tester
        // (agent.tester_id == run.tester_id). NOT agent.agent_id == run.tester_id
        // — tester_id is a project_tester FK, never an agent id.
        if (preferredTesterId is Guid tid)
        {
            var bound = compatible.FirstOrDefault(a => a.TesterId == tid);
            if (bound is not null)
            {
                return bound.AgentId;
            }
        }

        return compatible[0].AgentId;
    }

    // ── Proxy endpoint resolution (Rust resolve_proxy_endpoint) ─────────────

    /// <summary>
    /// Resolve a <c>Proxy { proxy_endpoint_id }</c> endpoint into a concrete
    /// <c>Network { host, port }</c> the standalone agent can probe — the port of
    /// the Rust <c>resolve_proxy_endpoint</c> (provisioning.rs). The UI stores
    /// the DEPLOYMENT id in <c>proxy_endpoint_id</c>; the deployed target's host
    /// lives in <c>deployment.endpoint_ips[0]</c> and the proxy stack (which
    /// selects the HTTPS listener port) in
    /// <c>deployment.config.endpoints[0].http_stacks[0]</c>. Without this rewrite
    /// every Network Test against a deployed target fails with "Unsupported
    /// endpoint kind for standalone agent: proxy" and 0 attempts (v0.28.10 prod
    /// fix). Returns null on any resolution failure — the caller dispatches the
    /// config unresolved so the agent reports the (accurate) unsupported-kind
    /// error, matching Rust.
    /// </summary>
    private async Task<JsonElement?> ResolveProxyEndpointAsync(string endpointRefText, CancellationToken ct)
    {
        Guid deploymentId;
        string? stackHint = null;
        try
        {
            using var doc = JsonDocument.Parse(endpointRefText);
            if (!doc.RootElement.TryGetProperty("proxy_endpoint_id", out var idProp) ||
                !Guid.TryParse(idProp.GetString(), out deploymentId))
            {
                _logger.LogWarning("Proxy endpoint has no parseable proxy_endpoint_id — dispatching unresolved");
                return null;
            }

            // Optional stack override carried on the endpoint itself; falls back
            // to the deployment's recorded stack below.
            if (doc.RootElement.TryGetProperty("proxy_stack", out var stackProp) &&
                stackProp.ValueKind == JsonValueKind.String)
            {
                stackHint = stackProp.GetString();
            }
        }
        catch (JsonException ex)
        {
            _logger.LogWarning(ex, "Proxy endpoint_ref is not valid JSON — dispatching unresolved");
            return null;
        }

        var dep = await _db.Deployments
            .AsNoTracking()
            .FirstOrDefaultAsync(d => d.DeploymentId == deploymentId, ct);
        if (dep is null)
        {
            _logger.LogWarning(
                "Failed to resolve proxy endpoint — deployment {DeploymentId} not found; dispatching unresolved",
                deploymentId);
            return null;
        }

        var host = FirstEndpointIp(dep.EndpointIps);
        if (host is null)
        {
            _logger.LogWarning(
                "Failed to resolve proxy endpoint — deployment {DeploymentId} has no endpoint IPs (status: {Status}); dispatching unresolved",
                deploymentId, dep.Status);
            return null;
        }

        var stack = !string.IsNullOrWhiteSpace(stackHint)
            ? stackHint!
            : ProxyStackFromDeploymentConfig(dep.Config) ?? "nginx";

        var resolved = new
        {
            kind = "network",
            host,
            port = ProxyHttpsPort(stack),
        };
        return JsonSerializer.SerializeToElement(resolved);
    }

    /// <summary>First non-empty string in the <c>endpoint_ips</c> JSON array.</summary>
    private static string? FirstEndpointIp(string? endpointIps)
    {
        if (string.IsNullOrWhiteSpace(endpointIps))
        {
            return null;
        }

        try
        {
            using var doc = JsonDocument.Parse(endpointIps);
            if (doc.RootElement.ValueKind != JsonValueKind.Array)
            {
                return null;
            }
            foreach (var el in doc.RootElement.EnumerateArray())
            {
                if (el.ValueKind == JsonValueKind.String)
                {
                    var s = el.GetString()?.Trim();
                    if (!string.IsNullOrEmpty(s))
                    {
                        return s;
                    }
                }
            }
        }
        catch (JsonException)
        {
            return null;
        }
        return null;
    }

    /// <summary>
    /// The Rust path <c>deployment.config.endpoints[0].http_stacks[0]</c>.
    /// </summary>
    private static string? ProxyStackFromDeploymentConfig(string configText)
    {
        try
        {
            using var doc = JsonDocument.Parse(configText);
            if (doc.RootElement.TryGetProperty("endpoints", out var eps) &&
                eps.ValueKind == JsonValueKind.Array &&
                eps.GetArrayLength() > 0 &&
                eps[0].TryGetProperty("http_stacks", out var stacks) &&
                stacks.ValueKind == JsonValueKind.Array &&
                stacks.GetArrayLength() > 0 &&
                stacks[0].ValueKind == JsonValueKind.String)
            {
                var s = stacks[0].GetString();
                return string.IsNullOrWhiteSpace(s) ? null : s;
            }
        }
        catch (JsonException)
        {
            // fall through
        }
        return null;
    }

    /// <summary>
    /// HTTPS listener port for a proxy stack after a standard deploy — the same
    /// table as <see cref="Provisioning.ProvisioningOrchestrator"/> (ported from
    /// <c>networker_common::test_config::proxy_https_port</c>). Public so the
    /// unit tests exercise the actual table the dispatcher uses.
    /// </summary>
    public static int ProxyHttpsPort(string stack)
        => Provisioning.ProvisioningOrchestrator.ProxyHttpsPort(stack);

    // ── Wire serialization ────────────────────────────────────────────────────

    /// <summary>
    /// Serialize the run + config to the canonical snake_case
    /// <c>networker_common::TestRun</c> / <c>TestConfig</c> JSON the agent decodes
    /// — the same shapes the M1 read endpoints emit. The JSONB-as-text columns
    /// (<c>endpoint_ref</c>, <c>workload</c>, <c>methodology</c>) are spliced in as
    /// raw JSON (not escaped strings) so the polymorphic <c>endpoint</c>/
    /// <c>workload</c> objects arrive intact. Returned as <see cref="JsonElement"/>
    /// so <see cref="AgentConnectionRegistry.AssignRunAsync"/> can carry them
    /// opaquely into the <c>assign_run</c> envelope.
    ///
    /// <para><c>Proxy</c> endpoints are resolved on a COPY for the wire payload
    /// — endpoint rewritten to <c>Network{host,port}</c> and
    /// <c>workload.insecure = true</c> (deployed targets serve self-signed
    /// certificates by construction) — while the stored config row is left
    /// untouched, matching the Rust <c>try_dispatch_run</c> clone.</para>
    /// </summary>
    private async Task<(JsonElement Run, JsonElement Config)> SerializeForAssignAsync(
        Data.Entities.TestRun run,
        Data.Entities.TestConfig cfg,
        CancellationToken ct)
    {
        object endpointJson = RawJson(cfg.EndpointRef);
        object workloadJson = RawJson(cfg.Workload);

        if (string.Equals(cfg.EndpointKind, EndpointKindProxy, StringComparison.OrdinalIgnoreCase))
        {
            var resolved = await ResolveProxyEndpointAsync(cfg.EndpointRef, ct);
            if (resolved is JsonElement resolvedEndpoint)
            {
                endpointJson = resolvedEndpoint;
                workloadJson = WithInsecure(cfg.Workload);
            }
        }

        // sdkprobe: decrypt the stored LagHound token and splice it into the
        // wire clone's workload as `laghound_token` (the agent maps it to the
        // tester's --laghound-token). The stored config row is NEVER mutated,
        // and the token is NEVER logged (SendAsync logs only the message type;
        // this method logs nothing). Mirrors the WithInsecure copy-on-write.
        if (IsSdkProbeWorkload(workloadJson) && cfg.TokenEnc is { Length: > 0 } && cfg.TokenNonce is { Length: > 0 })
        {
            workloadJson = WithLagHoundToken(workloadJson, cfg.TokenEnc, cfg.TokenNonce);
        }

        var runDto = new
        {
            id = run.Id,
            test_config_id = run.TestConfigId,
            project_id = run.ProjectId,
            status = run.Status,
            started_at = run.StartedAt,
            finished_at = run.FinishedAt,
            success_count = run.SuccessCount,
            failure_count = run.FailureCount,
            error_message = run.ErrorMessage,
            artifact_id = run.ArtifactId,
            tester_id = run.TesterId,
            worker_id = run.WorkerId,
            last_heartbeat = run.LastHeartbeat,
            created_at = run.CreatedAt,
            comparison_group_id = run.ComparisonGroupId,
        };

        var configDto = new
        {
            id = cfg.Id,
            project_id = cfg.ProjectId,
            name = cfg.Name,
            description = cfg.Description,
            endpoint = endpointJson,
            workload = workloadJson,
            methodology = RawJsonOrNull(cfg.Methodology),
            baseline_run_id = cfg.BaselineRunId,
            max_duration_secs = cfg.MaxDurationSecs,
            created_by = cfg.CreatedBy,
            created_at = cfg.CreatedAt,
            updated_at = cfg.UpdatedAt,
        };

        var runElement = JsonSerializer.SerializeToElement(runDto);
        var configElement = JsonSerializer.SerializeToElement(configDto);
        return (runElement, configElement);
    }

    /// <summary>
    /// Copy of the workload JSON with <c>insecure: true</c> — the Rust
    /// <c>c.workload.insecure = true</c> applied to the wire clone only.
    /// </summary>
    private static object WithInsecure(string workloadText)
    {
        try
        {
            var node = JsonNode.Parse(workloadText);
            if (node is JsonObject obj)
            {
                obj["insecure"] = true;
                return JsonSerializer.SerializeToElement(obj);
            }
        }
        catch (JsonException)
        {
            // fall through — ship the workload unmodified
        }
        return RawJson(workloadText);
    }

    /// <summary>
    /// True when the (already-resolved) wire workload runs the <c>sdkprobe</c>
    /// mode — i.e. its <c>modes</c> array contains "sdkprobe". Operates on the
    /// boxed <see cref="JsonElement"/> the serializer will actually ship.
    /// </summary>
    private static bool IsSdkProbeWorkload(object workloadJson)
    {
        if (workloadJson is not JsonElement el || el.ValueKind != JsonValueKind.Object)
        {
            return false;
        }
        if (!el.TryGetProperty("modes", out var modes) || modes.ValueKind != JsonValueKind.Array)
        {
            return false;
        }
        foreach (var m in modes.EnumerateArray())
        {
            if (m.ValueKind == JsonValueKind.String
                && string.Equals(m.GetString(), SdkEndpointsEndpoints.SdkProbeMode, StringComparison.OrdinalIgnoreCase))
            {
                return true;
            }
        }
        return false;
    }

    /// <summary>
    /// Decrypt the SDK token and return a COPY of the workload with
    /// <c>laghound_token</c> spliced in (wire clone only; the stored row keeps
    /// the ciphertext). If decryption fails, the original workload is shipped
    /// unchanged — the tester then classifies the SDK routes as a config error
    /// (404), which is the correct visible failure. The plaintext token is
    /// never logged.
    /// </summary>
    private object WithLagHoundToken(object workloadJson, byte[] tokenEnc, byte[] tokenNonce)
    {
        string token;
        try
        {
            token = System.Text.Encoding.UTF8.GetString(_cipher.Decrypt(tokenEnc, tokenNonce));
        }
        catch (Exception ex)
        {
            // Do NOT include the token or ciphertext in the log. A lost/rotated
            // key is operational, not a crash.
            _logger.LogWarning(
                "sdkprobe dispatch: failed to decrypt LagHound token ({Reason}); "
                + "shipping the run without a token (SDK routes will answer 404)",
                ex.GetType().Name);
            return workloadJson;
        }

        if (string.IsNullOrEmpty(token))
        {
            return workloadJson;
        }

        try
        {
            var node = workloadJson is JsonElement el
                ? JsonNode.Parse(el.GetRawText())
                : null;
            if (node is JsonObject obj)
            {
                obj["laghound_token"] = token;
                return JsonSerializer.SerializeToElement(obj);
            }
        }
        catch (JsonException)
        {
            // fall through — ship the workload unmodified
        }
        return workloadJson;
    }

    // Parse a JSONB-as-text column into a JsonElement so it serializes as raw
    // JSON rather than an escaped string. Falls back to the original text if it
    // isn't valid JSON (defensive; the DB constraint should guarantee validity).
    private static object RawJson(string value)
    {
        try
        {
            using var doc = JsonDocument.Parse(value);
            return doc.RootElement.Clone();
        }
        catch (JsonException)
        {
            return value;
        }
    }

    private static object? RawJsonOrNull(string? value)
        => value is null ? null : RawJson(value);
}

/// <summary>
/// Minimum-agent-version gate for <c>assign_run</c> — the C# port of the Rust
/// <c>parse_version</c> / <c>any_online_agent_min_version</c> pair
/// (ws/agent_hub.rs) and <c>MIN_AGENT_VERSION_FOR_ASSIGN_RUN = "0.28.0"</c>
/// (provisioning.rs). Agents older than 0.28.0 silently drop <c>assign_run</c>,
/// so they must never be selected. Parsing is intentionally permissive (Rust:
/// malformed parts fall back to 0, so garbage parses to 0.0.0 and is rejected
/// by the ≥ 0.28.0 comparison — never a throw).
/// </summary>
public static class AgentVersionGate
{
    /// <summary>Rust <c>MIN_AGENT_VERSION_FOR_ASSIGN_RUN</c>.</summary>
    public const string MinAssignRunVersionString = "0.28.0";

    private static readonly (int Major, int Minor, int Patch) MinAssignRunVersion = (0, 28, 0);

    /// <summary>
    /// Whether <paramref name="version"/> (as reported by the agent) parses and
    /// is ≥ 0.28.0. Null/blank is rejected (the Rust loop <c>continue</c>s over
    /// NULL versions); garbage parses to 0.0.0 and is rejected by comparison.
    /// </summary>
    public static bool IsCompatible(string? version)
    {
        if (string.IsNullOrWhiteSpace(version))
        {
            return false;
        }
        var v = Parse(version);
        return v.CompareTo(MinAssignRunVersion) >= 0;
    }

    /// <summary>
    /// Parse a dotted-triple version string into a tuple — the exact Rust
    /// <c>parse_version</c>: leading <c>v</c> stripped, pre-release suffix after
    /// <c>-</c> stripped per part, malformed parts fall back to 0.
    /// </summary>
    public static (int Major, int Minor, int Patch) Parse(string s)
    {
        var trimmed = s.TrimStart('v');
        var parts = trimmed.Split('.');
        return (PartOrZero(parts, 0), PartOrZero(parts, 1), PartOrZero(parts, 2));

        static int PartOrZero(string[] parts, int index)
        {
            if (index >= parts.Length)
            {
                return 0;
            }
            var head = parts[index].Split('-')[0];
            return int.TryParse(head, out var n) && n >= 0 ? n : 0;
        }
    }
}

/// <summary>
/// Thrown when a dispatcher operation targets a run/config that does not exist.
/// The write endpoints translate this into a 404.
/// </summary>
public sealed class RunDispatchNotFoundException(string message) : Exception(message);
