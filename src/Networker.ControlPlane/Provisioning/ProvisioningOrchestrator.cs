using System.Text.Json;
using System.Text.Json.Nodes;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Background;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// The provisioning orchestrator — the C# port of the two halves of the Rust
/// dashboard's <c>provisioning::kick_provisioning</c>
/// (<c>crates/networker-dashboard/src/provisioning.rs</c>) and
/// <c>benchmark_worker</c> promote loop
/// (<c>crates/networker-dashboard/src/benchmark_worker.rs</c>), unified into one
/// ~5s-tick <see cref="BackgroundService"/>.
///
/// <para>M3's <see cref="Dispatch.RunDispatcher"/> deliberately leaves a run with a
/// <c>Pending</c> endpoint sitting in <c>queued</c> (it logs a deferral and does
/// NOT dispatch). This service closes that gap: it drives such runs through
/// their provisioning lifecycle and re-queues them once a VM is live, so the
/// dispatcher/redispatcher then assigns them to an agent normally.</para>
///
/// <para><b>Each tick, in a fresh DI scope:</b></para>
/// <list type="number">
///   <item><b>Kick</b> — find <c>queued</c> runs whose config endpoint is
///     <c>Pending</c> and which have no <c>provisioning_deployment_id</c> yet.
///     For each: resolve the provider from the cloud account, build the
///     <c>deploy.json</c>, insert a <c>deployment</c> row (status <c>pending</c>),
///     set <c>test_run.provisioning_deployment_id</c> + status
///     <c>provisioning</c>, and start the <see cref="DeployRunner"/> on a
///     detached background task (matching the Rust <c>tokio::spawn</c>).</item>
///   <item><b>Promote</b> — find <c>provisioning</c> runs whose deployment is
///     <c>completed</c>: rewrite the config's endpoint <c>Pending → Network{host,
///     port}</c> (host = first captured endpoint IP, port =
///     <c>proxy_https_port(proxy_stack)</c>) and flip the run back to
///     <c>queued</c>. If the deployment is <c>failed</c>, fail the run.</item>
/// </list>
///
/// <para><b>SHARED-CONFIG CAVEAT (matches Rust, flagged):</b> promote rewrites
/// the shared <c>test_config.endpoint_ref</c> row in place — exactly what the
/// Rust <c>promote()</c> does via <c>test_configs::update_endpoint</c>. If the
/// same <c>TestConfig</c> is reused by more than one run (e.g. a scheduled config
/// launched repeatedly), the last provision's host clobbers the template's
/// <c>Pending</c> endpoint for every future run. The scope doc's preferred design
/// is to store the resolved endpoint on the <b>run</b> (a per-run override
/// column) and leave the config template untouched. That needs a new column/DTO
/// path not present in this slice, so this port matches Rust's behaviour and
/// leaves the improvement as follow-up.</para>
/// </summary>
public sealed class ProvisioningOrchestrator : BackgroundService
{
    private static readonly TimeSpan TickInterval = TimeSpan.FromSeconds(5);
    private static readonly TimeSpan StartupDelay = TimeSpan.FromSeconds(2);

    // Wire status strings (Rust RunStatus / deployment.status are lowercase).
    private const string RunQueued = "queued";
    private const string RunProvisioning = "provisioning";
    private const string EndpointKindPending = "pending";
    private const string EndpointKindNetwork = "network";
    private const string DeploymentPending = "pending";
    private const string DeploymentCompleted = "completed";
    private const string DeploymentFailed = "failed";
    private const string DeploymentCancelled = "cancelled";

    private const int KickBatchLimit = 25;

    private readonly IServiceScopeFactory _scopeFactory;
    private readonly DeployRunner _runner;
    private readonly ILogger<ProvisioningOrchestrator> _logger;
    private readonly PgAdvisoryLeaderLock? _leader;
    private readonly TickMonitor _monitor;

    public ProvisioningOrchestrator(
        IServiceScopeFactory scopeFactory,
        DeployRunner runner,
        ILogger<ProvisioningOrchestrator> logger,
        PgAdvisoryLeaderLock? leaderLock = null,
        TickMonitor? tickMonitor = null)
    {
        _scopeFactory = scopeFactory;
        _runner = runner;
        _logger = logger;
        // M6 ops infra (AddOpsInfrastructure); optional for bare test hosts.
        _leader = leaderLock;
        _monitor = tickMonitor ?? new TickMonitor();
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        try
        {
            await Task.Delay(StartupDelay, stoppingToken).ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            return;
        }

        _logger.LogInformation("Provisioning orchestrator started (tick every {Seconds}s)", TickInterval.TotalSeconds);
        _monitor.ReportStarted(OpsServiceNames.ProvisioningOrchestrator);

        using var timer = new PeriodicTimer(TickInterval);
        while (await timer.WaitForNextTickAsync(stoppingToken).ConfigureAwait(false))
        {
            try
            {
                var ranAsLeader = await _leader
                    .TryRunGuardedAsync(LeaderLockKeys.ProvisioningOrchestrator, TickAsync, stoppingToken)
                    .ConfigureAwait(false);
                if (!ranAsLeader)
                {
                    _logger.LogDebug("Provisioning orchestrator tick skipped — another replica holds the leader lock");
                }
            }
            catch (OperationCanceledException) when (stoppingToken.IsCancellationRequested)
            {
                break;
            }
            catch (Exception ex)
            {
                _monitor.ReportError(OpsServiceNames.ProvisioningOrchestrator, ex);
                _logger.LogError(ex, "Provisioning orchestrator tick failed");
            }
        }
    }

    private async Task TickAsync(CancellationToken ct)
    {
        using var scope = _scopeFactory.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();

        var kicked = await KickPendingRunsAsync(db, ct).ConfigureAwait(false);
        var resolved = await PromoteProvisioningRunsAsync(db, ct).ConfigureAwait(false);

        _monitor.ReportTick(
            OpsServiceNames.ProvisioningOrchestrator,
            kicked + resolved,
            $"kicked={kicked} resolved={resolved}");
    }

    // ── Kick: queued + Pending + no deployment ⇒ start provisioning ──────────

    /// <returns>Number of runs whose provisioning was actually kicked off.</returns>
    private async Task<int> KickPendingRunsAsync(NetworkerDbContext db, CancellationToken ct)
    {
        // queued runs, config endpoint is Pending, not yet linked to a deployment.
        var candidates = await db.TestRuns
            .Where(r => r.Status == RunQueued
                        && r.ProvisioningDeploymentId == null
                        && r.TestConfig.EndpointKind == EndpointKindPending)
            .OrderBy(r => r.CreatedAt)
            .Take(KickBatchLimit)
            .Select(r => new { Run = r, r.TestConfig })
            .ToListAsync(ct);

        var kicked = 0;
        foreach (var c in candidates)
        {
            ct.ThrowIfCancellationRequested();
            try
            {
                if (await KickOneAsync(db, c.Run, c.TestConfig, ct).ConfigureAwait(false))
                {
                    kicked++;
                }
            }
            catch (Exception ex)
            {
                _logger.LogError(ex, "Failed to kick provisioning for run {RunId}", c.Run.Id);
            }
        }

        return kicked;
    }

    /// <returns><c>true</c> when this call kicked off the deployment.</returns>
    private async Task<bool> KickOneAsync(NetworkerDbContext db, TestRun run, TestConfig cfg, CancellationToken ct)
    {
        var pending = ParsePending(cfg.EndpointRef, _logger);
        if (pending is null)
        {
            _logger.LogWarning(
                "Run {RunId} config {ConfigId} is endpoint_kind=pending but endpoint_ref did not parse as Pending — skipping",
                run.Id, cfg.Id);
            return false;
        }

        // Resolve the concrete provider from the cloud account. install.sh has no
        // DB access, so `provider: "auto"` is never resolvable there — every
        // Pending deploy must carry the real provider. Mirrors kick_provisioning.
        var provider = await db.CloudAccounts
            .AsNoTracking()
            .Where(a => a.AccountId == pending.CloudAccountId)
            .Select(a => a.Provider)
            .FirstOrDefaultAsync(ct);
        if (string.IsNullOrEmpty(provider))
        {
            _logger.LogWarning(
                "Cloud account {AccountId} not found for run {RunId} — cannot provision",
                pending.CloudAccountId, run.Id);
            return false;
        }

        var deployJson = BuildDeployJson(pending, provider, cfg.Name);
        var deployText = deployJson.ToJsonString();
        var providerSummary = BuildProviderSummary(deployJson);

        var deploymentId = Guid.NewGuid();
        var now = DateTime.UtcNow;
        db.Deployments.Add(new Deployment
        {
            DeploymentId = deploymentId,
            Name = $"auto-{cfg.Name}-{ShortId(run.Id)}",
            Status = DeploymentPending,
            Config = deployText,
            ProviderSummary = providerSummary,
            CreatedBy = cfg.CreatedBy,
            CreatedAt = now,
            ProjectId = cfg.ProjectId,
            CloudAccountId = pending.CloudAccountId,
        });

        // Link the run + flip to provisioning. Guard with a status/link check so
        // two overlapping ticks can't double-kick the same run (the ExecuteUpdate
        // is atomic; the second one affects 0 rows and we roll back the insert).
        await db.SaveChangesAsync(ct).ConfigureAwait(false);

        var linked = await db.TestRuns
            .Where(r => r.Id == run.Id && r.Status == RunQueued && r.ProvisioningDeploymentId == null)
            .ExecuteUpdateAsync(s => s
                .SetProperty(r => r.Status, RunProvisioning)
                .SetProperty(r => r.ProvisioningDeploymentId, deploymentId), ct)
            .ConfigureAwait(false);

        if (linked == 0)
        {
            // Lost the race — another tick already linked this run. Drop the
            // orphan deployment row we just created so it doesn't run twice.
            _logger.LogInformation("Run {RunId} was already linked to a deployment — discarding duplicate kick", run.Id);
            await db.Deployments
                .Where(d => d.DeploymentId == deploymentId)
                .ExecuteDeleteAsync(ct)
                .ConfigureAwait(false);
            return false;
        }

        _logger.LogInformation(
            "Provisioning kicked off for run {RunId} (config {ConfigName}, provider {Provider}, region {Region}, proxy {Proxy}) → deployment {DeploymentId}",
            run.Id, cfg.Name, provider, pending.Region, pending.ProxyStack, deploymentId);

        // Detached background deploy — matches the Rust tokio::spawn. Uses the
        // application lifetime (CancellationToken.None) so it isn't torn down by
        // the tick's scope; the runner opens its own DB scope.
        _ = Task.Run(async () =>
        {
            try
            {
                await _runner.RunDeploymentAsync(deploymentId, deployText, CancellationToken.None).ConfigureAwait(false);
            }
            catch (Exception ex)
            {
                _logger.LogError(ex, "Auto-provisioning deploy runner failed for deployment {DeploymentId}", deploymentId);
            }
        }, CancellationToken.None);

        return true;
    }

    // ── Promote: provisioning runs whose deployment finished ─────────────────

    /// <returns>Number of runs resolved this pass (promoted or failed).</returns>
    private async Task<int> PromoteProvisioningRunsAsync(NetworkerDbContext db, CancellationToken ct)
    {
        var pairs = await db.TestRuns
            .AsNoTracking()
            .Where(r => r.Status == RunProvisioning && r.ProvisioningDeploymentId != null)
            .Select(r => new { r.Id, r.TestConfigId, DeploymentId = r.ProvisioningDeploymentId!.Value })
            .ToListAsync(ct);

        var resolved = 0;
        foreach (var p in pairs)
        {
            ct.ThrowIfCancellationRequested();
            try
            {
                if (await HandleProvisioningRunAsync(db, p.Id, p.TestConfigId, p.DeploymentId, ct).ConfigureAwait(false))
                {
                    resolved++;
                }
            }
            catch (Exception ex)
            {
                _logger.LogError(ex,
                    "Orchestrator failed to handle provisioning run {RunId} (deployment {DeploymentId})",
                    p.Id, p.DeploymentId);
            }
        }

        return resolved;
    }

    /// <returns><c>true</c> when the run reached a resolution this pass
    /// (re-queued after promote, or failed); <c>false</c> when it is still
    /// waiting on its deployment.</returns>
    private async Task<bool> HandleProvisioningRunAsync(
        NetworkerDbContext db, Guid runId, Guid testConfigId, Guid deploymentId, CancellationToken ct)
    {
        var deployment = await db.Deployments
            .AsNoTracking()
            .FirstOrDefaultAsync(d => d.DeploymentId == deploymentId, ct);
        if (deployment is null)
        {
            _logger.LogWarning("Deployment {DeploymentId} vanished for provisioning run {RunId}", deploymentId, runId);
            return false;
        }

        switch (deployment.Status)
        {
            case DeploymentCompleted:
                await PromoteAsync(db, runId, testConfigId, deployment, ct).ConfigureAwait(false);
                return true;

            case DeploymentFailed:
                var msg = deployment.ErrorMessage ?? "deployment failed";
                await db.TestRuns
                    .Where(r => r.Id == runId)
                    .ExecuteUpdateAsync(s => s
                        .SetProperty(r => r.Status, "failed")
                        .SetProperty(r => r.ErrorMessage, $"Provisioning failed: {msg}")
                        .SetProperty(r => r.FinishedAt, DateTime.UtcNow), ct)
                    .ConfigureAwait(false);
                _logger.LogWarning(
                    "Run {RunId} failed: provisioning deployment {DeploymentId} failed ({Error})",
                    runId, deploymentId, msg);
                return true;

            case DeploymentCancelled:
                // A cancelled deployment is terminal — its run can never make
                // progress, so fail it instead of leaving it in `provisioning`
                // forever (quality audit F3(a)). Mirror the DeploymentFailed arm.
                await db.TestRuns
                    .Where(r => r.Id == runId)
                    .ExecuteUpdateAsync(s => s
                        .SetProperty(r => r.Status, "failed")
                        .SetProperty(r => r.ErrorMessage, "Provisioning cancelled")
                        .SetProperty(r => r.FinishedAt, DateTime.UtcNow), ct)
                    .ConfigureAwait(false);
                _logger.LogWarning(
                    "Run {RunId} failed: provisioning deployment {DeploymentId} was cancelled",
                    runId, deploymentId);
                return true;

            // pending / running — leave alone; re-check next tick.
            default:
                return false;
        }
    }

    /// <summary>Deployment completed: rewrite the config endpoint Pending→Network
    /// and re-queue the run for the dispatcher. Mirrors Rust <c>promote()</c>.</summary>
    private async Task PromoteAsync(
        NetworkerDbContext db, Guid runId, Guid testConfigId, Deployment deployment, CancellationToken ct)
    {
        var cfg = await db.TestConfigs.FirstOrDefaultAsync(c => c.Id == testConfigId, ct);
        if (cfg is null)
        {
            _logger.LogWarning("test_config {ConfigId} vanished while promoting run {RunId}", testConfigId, runId);
            return;
        }

        var pending = ParsePending(cfg.EndpointRef, _logger);
        if (pending is null)
        {
            // Already rewritten by an earlier tick (shared config, or a prior
            // promote) — just move the run along.
            await db.TestRuns
                .Where(r => r.Id == runId)
                .ExecuteUpdateAsync(s => s.SetProperty(r => r.Status, RunQueued), ct)
                .ConfigureAwait(false);
            return;
        }

        var host = FirstEndpointHost(deployment.EndpointIps);
        if (host is null)
        {
            // A completed deployment with no captured endpoint IPs is a PERMANENT
            // condition — retrying every tick never produces a host, it only spins
            // the run in `provisioning` and spams the log forever (quality audit
            // F3(d)). Fail the run terminally instead.
            await db.TestRuns
                .Where(r => r.Id == runId)
                .ExecuteUpdateAsync(s => s
                    .SetProperty(r => r.Status, "failed")
                    .SetProperty(r => r.ErrorMessage, "Provisioning completed but captured no endpoint IPs")
                    .SetProperty(r => r.FinishedAt, DateTime.UtcNow), ct)
                .ConfigureAwait(false);
            _logger.LogWarning(
                "Deployment {DeploymentId} completed but captured no endpoint IPs — failing run {RunId} (permanent)",
                deployment.DeploymentId, runId);
            return;
        }

        var port = ProxyHttpsPort(pending.ProxyStack);

        // Rewrite endpoint_ref → Network{host,port} + endpoint_kind → network.
        // SHARED-CONFIG CAVEAT (see class doc): this mutates the shared template.
        var newEndpoint = new JsonObject
        {
            ["kind"] = EndpointKindNetwork,
            ["host"] = host,
            ["port"] = port,
        };
        cfg.EndpointRef = newEndpoint.ToJsonString();
        cfg.EndpointKind = EndpointKindNetwork;
        cfg.UpdatedAt = DateTime.UtcNow;
        await db.SaveChangesAsync(ct).ConfigureAwait(false);

        await db.TestRuns
            .Where(r => r.Id == runId)
            .ExecuteUpdateAsync(s => s.SetProperty(r => r.Status, RunQueued), ct)
            .ConfigureAwait(false);

        _logger.LogInformation(
            "Provisioning complete for run {RunId}: endpoint rewritten to {Host}:{Port} (proxy {Proxy}), run re-queued",
            runId, host, port, pending.ProxyStack);
    }

    // ── deploy.json builder (port of build_deploy_json) ──────────────────────

    /// <summary>Build the deploy.json document handed to <c>install.sh --deploy</c>.
    /// Byte-shape-compatible with the Rust <c>build_deploy_json</c>: a
    /// per-provider endpoint block, <c>tester:{provider:"local"}</c>,
    /// <c>tests:{run_tests:false}</c>, <c>version:1</c>, and the concrete
    /// <c>cloud_account_id</c>.</summary>
    internal static JsonObject BuildDeployJson(PendingEndpoint p, string provider, string cfgName)
    {
        var suffix = ShortIdFromName(cfgName);
        var vmLabel = SanitizeVmLabel($"nwk-auto-{suffix}");

        JsonObject providerBlock = provider switch
        {
            "aws" => new JsonObject
            {
                ["region"] = p.Region,
                ["instance_type"] = p.VmSize,
                ["os"] = p.Os,
                ["instance_name"] = vmLabel,
            },
            "gcp" => new JsonObject
            {
                ["region"] = p.Region,
                ["zone"] = $"{p.Region}-a",
                ["machine_type"] = p.VmSize,
                ["os"] = p.Os,
                ["instance_name"] = vmLabel,
            },
            // azure + default
            _ => new JsonObject
            {
                ["region"] = p.Region,
                ["vm_size"] = p.VmSize,
                ["os"] = p.Os,
                ["vm_name"] = vmLabel,
            },
        };

        var endpoint = new JsonObject
        {
            ["provider"] = provider,
            ["label"] = cfgName,
            ["http_stacks"] = new JsonArray(p.ProxyStack),
            [provider] = providerBlock,
        };
        if (!string.IsNullOrEmpty(p.Language))
        {
            endpoint["languages"] = new JsonArray(p.Language);
        }

        return new JsonObject
        {
            ["version"] = 1,
            ["tester"] = new JsonObject { ["provider"] = "local" },
            ["cloud_account_id"] = p.CloudAccountId.ToString(),
            ["endpoints"] = new JsonArray(endpoint),
            ["tests"] = new JsonObject { ["run_tests"] = false },
        };
    }

    /// <summary>Human-readable "provider region + ..." summary, mirroring the Rust
    /// <c>build_provider_summary</c> used on the deployment row.</summary>
    internal static string? BuildProviderSummary(JsonObject deployJson)
    {
        if (deployJson["endpoints"] is not JsonArray endpoints || endpoints.Count == 0)
        {
            return null;
        }

        var parts = new List<string>();
        foreach (var ep in endpoints)
        {
            var provider = ep?["provider"]?.GetValue<string>() ?? "unknown";
            var region = ep?["region"]?.GetValue<string?>();
            // region lives inside the per-provider block, not at endpoint top-level;
            // try both so the summary matches whatever shape is present.
            if (string.IsNullOrEmpty(region) && ep?[provider] is JsonObject block)
            {
                region = block["region"]?.GetValue<string?>();
            }
            parts.Add(string.IsNullOrEmpty(region) ? provider : $"{provider} {region}");
        }

        return parts.Count == 0 ? null : string.Join(" + ", parts);
    }

    // ── EndpointRef (Pending) parsing ────────────────────────────────────────

    /// <summary>Parse a JSONB <c>endpoint_ref</c> text column into a
    /// <see cref="PendingEndpoint"/>, or null if it isn't a Pending endpoint.
    /// The tagged-union shape is <c>{"kind":"pending", cloud_account_id, region,
    /// vm_size, os, proxy_stack, topology, language?}</c>. A malformed ref
    /// (undecodable JSON / missing <c>cloud_account_id</c>) also yields null,
    /// but is WARN-logged when a logger is supplied — callers otherwise can't
    /// distinguish "not a pending endpoint" from "corrupt pending endpoint".</summary>
    internal static PendingEndpoint? ParsePending(string endpointRef, ILogger? logger = null)
    {
        try
        {
            using var doc = JsonDocument.Parse(endpointRef);
            var root = doc.RootElement;
            if (root.ValueKind != JsonValueKind.Object
                || !root.TryGetProperty("kind", out var kind)
                || kind.GetString() != EndpointKindPending)
            {
                return null;
            }

            var cloudAccountId = root.GetProperty("cloud_account_id").GetGuid();
            var region = root.TryGetProperty("region", out var r) ? r.GetString() ?? "" : "";
            var vmSize = root.TryGetProperty("vm_size", out var v) ? v.GetString() ?? "" : "";
            var os = root.TryGetProperty("os", out var o) ? o.GetString() ?? "" : "";
            var proxyStack = root.TryGetProperty("proxy_stack", out var ps) ? ps.GetString() ?? "nginx" : "nginx";
            var language = root.TryGetProperty("language", out var l) && l.ValueKind == JsonValueKind.String
                ? l.GetString()
                : null;

            return new PendingEndpoint(cloudAccountId, region, vmSize, os, proxyStack, language);
        }
        catch (Exception ex)
        {
            // Behavior preserved (null → caller skips the run), but no longer a
            // silent swallow: a pending-kind ref that fails to parse means a
            // stuck run, so leave a trace of WHY.
            logger?.LogWarning(
                ex,
                "endpoint_ref failed to parse as a Pending endpoint ({Length} chars) — treating as non-pending",
                endpointRef.Length);
            return null;
        }
    }

    /// <summary>First usable host (FQDN preferred, bare IP otherwise) from the
    /// deployment's JSON <c>endpoint_ips</c> array text. Mirrors Rust
    /// <c>first_endpoint_host</c>.</summary>
    internal static string? FirstEndpointHost(string? endpointIps)
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

    // ── Proxy port + label helpers (port of test_config.rs) ───────────────────

    /// <summary>HTTPS listener port for a proxy stack after a standard deploy.
    /// Ported verbatim from <c>networker_common::test_config::proxy_https_port</c>.</summary>
    internal static int ProxyHttpsPort(string stack) => stack switch
    {
        "nginx" => 8444,
        "caddy" => 8454,
        "traefik" => 8455,
        "haproxy" => 8456,
        "apache" => 8457,
        "iis" => 443,
        _ => 443,
    };

    private static string ShortId(Guid id) => id.ToString("N")[..8];

    private static string ShortIdFromName(string name)
    {
        var chars = new List<char>();
        foreach (var c in name)
        {
            if (char.IsAsciiLetterOrDigit(c))
            {
                chars.Add(char.ToLowerInvariant(c));
            }
            else if (c is ' ' or '-' or '_')
            {
                chars.Add('-');
            }
            if (chars.Count >= 8)
            {
                break;
            }
        }
        return new string(chars.ToArray());
    }

    // Windows NetBIOS constraint (install.sh's strictest): ≤15 chars, alnum + '-'.
    private static string SanitizeVmLabel(string raw)
    {
        var chars = new List<char>();
        foreach (var c in raw)
        {
            if (char.IsAsciiLetterOrDigit(c) || c == '-')
            {
                chars.Add(c);
            }
            if (chars.Count >= 15)
            {
                break;
            }
        }
        return new string(chars.ToArray());
    }

    /// <summary>Parsed <c>EndpointRef::Pending</c> payload.</summary>
    internal sealed record PendingEndpoint(
        Guid CloudAccountId,
        string Region,
        string VmSize,
        string Os,
        string ProxyStack,
        string? Language);
}
