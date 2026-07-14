using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Realtime;
using Networker.Data;

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
    private const string StatusCancelled = "cancelled";

    // The endpoint_kind column value for a Pending endpoint (deferred to M4).
    // Matches the Rust EndpointRef::Pending => "pending".
    private const string EndpointKindPending = "pending";

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

    public RunDispatcher(
        NetworkerDbContext db,
        AgentConnectionRegistry agents,
        EventBus bus,
        ILogger<RunDispatcher> logger)
    {
        _db = db;
        _agents = agents;
        _bus = bus;
        _logger = logger;
    }

    /// <inheritdoc />
    public async Task<Guid> LaunchAsync(
        Guid testConfigId,
        Guid? comparisonGroupId,
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
        // `provisioning`. Until the M4 provisioning orchestrator lands we leave
        // the run `queued` and annotate the log so it's visibly deferred (not
        // silently stranded). The redispatcher explicitly skips Pending runs.
        if (string.Equals(cfg.EndpointKind, EndpointKindPending, StringComparison.OrdinalIgnoreCase))
        {
            _logger.LogInformation(
                "Run {RunId} has a Pending endpoint — leaving queued; provisioning is deferred to M4",
                run.Id);
            return;
        }

        // ── Target selection (mirrors the Rust preference order). ────────────
        // 1. The run's tester's own agent, if online (agent WHERE tester_id = run.tester_id).
        // 2. Otherwise any online agent.
        var targetAgentId = await SelectTargetAgentAsync(run.TesterId, ct);
        if (targetAgentId is null)
        {
            // No agent online — leave queued; the redispatcher retries next tick.
            _logger.LogDebug(
                "Run {RunId} has no online agent — remains queued for later dispatch",
                run.Id);
            return;
        }

        var agentId = targetAgentId.Value;
        var (runJson, configJson) = SerializeForAssign(run, cfg);

        var sent = await _agents.AssignRunAsync(agentId, runJson, configJson, ct);
        if (sent)
        {
            _logger.LogInformation(
                "Dispatched run {RunId} to agent {AgentId} (endpoint_kind={Kind})",
                run.Id, agentId, cfg.EndpointKind);
            // Publish a JobUpdate so the browser bus reflects the assignment.
            // Status stays `queued` on the DB until the agent sends RunStarted;
            // this event carries the assigned agent id for the live UI.
            _bus.Publish(new JobUpdate(run.Id, StatusQueued, agentId, null, null));
        }
        else
        {
            // Send failed (agent raced offline) — leave queued, redispatcher retries.
            _logger.LogWarning(
                "Dispatch to agent {AgentId} failed for run {RunId} — will retry",
                agentId, run.Id);
        }
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
            var targetAgentId = await SelectTargetAgentAsync(run.TesterId, ct);
            if (targetAgentId is null)
            {
                continue;
            }

            var cfg = await _db.TestConfigs
                .AsNoTracking()
                .FirstOrDefaultAsync(c => c.Id == run.TestConfigId, ct);
            if (cfg is null)
            {
                continue;
            }

            var (runJson, configJson) = SerializeForAssign(run, cfg);
            var sent = await _agents.AssignRunAsync(targetAgentId.Value, runJson, configJson, ct);
            if (sent)
            {
                dispatched++;
                _logger.LogInformation(
                    "Redispatched previously-queued run {RunId} to agent {AgentId}",
                    run.Id, targetAgentId.Value);
                _bus.Publish(new JobUpdate(run.Id, StatusQueued, targetAgentId.Value, null, null));
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

    // ── Target-agent selection ───────────────────────────────────────────────

    /// <summary>
    /// Prefer the run's tester's own agent if it is online (agent WHERE
    /// tester_id = run.tester_id), else fall back to any online agent. Returns
    /// null when no agent is connected. Mirrors the Rust dispatch preference.
    /// </summary>
    private async Task<Guid?> SelectTargetAgentAsync(Guid? testerId, CancellationToken ct)
    {
        if (testerId is Guid tid)
        {
            // There may be more than one agent row for a tester across its
            // lifetime (re-registration); take the online one if present.
            var agentIds = await _db.Agents
                .AsNoTracking()
                .Where(a => a.TesterId == tid)
                .Select(a => a.AgentId)
                .ToListAsync(ct);

            foreach (var agentId in agentIds)
            {
                if (_agents.IsOnline(agentId))
                {
                    return agentId;
                }
            }
        }

        return _agents.AnyOnlineAgent();
    }

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
    /// </summary>
    private static (JsonElement Run, JsonElement Config) SerializeForAssign(
        Data.Entities.TestRun run,
        Data.Entities.TestConfig cfg)
    {
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
            endpoint = RawJson(cfg.EndpointRef),
            workload = RawJson(cfg.Workload),
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
/// Thrown when a dispatcher operation targets a run/config that does not exist.
/// The write endpoints translate this into a 404.
/// </summary>
public sealed class RunDispatchNotFoundException(string message) : Exception(message);
