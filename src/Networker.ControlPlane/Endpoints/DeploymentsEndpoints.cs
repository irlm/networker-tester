using System.Text.Json;
using System.Text.Json.Nodes;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Realtime;
using Networker.Data;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Read-only deployment + cloud-status endpoints, mirroring the Rust
/// dashboard's <c>api/deployments.rs</c> and <c>api/cloud.rs</c> project-scoped
/// GET handlers. Field names are snake_case to match the existing REST
/// contract. Cloud credential material is NEVER serialized.
/// </summary>
public static class DeploymentsEndpoints
{
    private const int DefaultLimit = 50;
    private const int MaxLimit = 200;

    public static IEndpointRouteBuilder MapDeploymentsEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/projects/{projectId}/deployments — paginated list.
        // Mirrors DeploymentRow from crates/networker-dashboard/src/db/deployments.rs.
        app.MapGet("/api/projects/{projectId}/deployments", async (
            string projectId, int? limit, int? offset, NetworkerDbContext db) =>
        {
            var take = Math.Clamp(limit ?? DefaultLimit, 1, MaxLimit);
            var skip = Math.Max(offset ?? 0, 0);

            var rows = await db.Deployments
                .AsNoTracking()
                .Where(d => d.ProjectId == projectId)
                .OrderByDescending(d => d.CreatedAt)
                .Skip(skip)
                .Take(take)
                .ToListAsync();

            return Results.Ok(rows.Select(ShapeDeployment));
        })
        .RequireAuthorization(AuthPolicies.ProjectMember);

        // GET /api/projects/{projectId}/deployments/{deploymentId} — detail.
        app.MapGet("/api/projects/{projectId}/deployments/{deploymentId:guid}", async (
            string projectId, Guid deploymentId, NetworkerDbContext db) =>
        {
            var d = await db.Deployments
                .AsNoTracking()
                .FirstOrDefaultAsync(x => x.ProjectId == projectId && x.DeploymentId == deploymentId);

            return d is null ? Results.NotFound() : Results.Ok(ShapeDeployment(d));
        })
        .RequireAuthorization(AuthPolicies.ProjectMember);

        // GET /api/projects/{projectId}/deployments/{deploymentId}/events — the
        // per-deployment SSE stream the deploy-detail page's useDeployEvents
        // hook has connected to since the Rust dashboard (v0.27.16). The C#
        // control plane never implemented it, so the hook 404'd and — worse —
        // a FINISHED deployment had no replay at all (issue #816). Frames are
        // the flat SeqEvent shape ({"seq":N,"type":"deploy_log",...}) the hook
        // parses.
        //
        //   * terminal deployment: replay the persisted log (deploy_log per
        //     line, synthetic ascending seq) then deploy_complete — the
        //     durable post-mortem path;
        //   * active deployment: replay the EventBus ring for this deployment,
        //     then tail it by polling (the bus has no per-consumer
        //     subscription API; 1s polls against a 2048-event ring cannot
        //     miss). Bus seqs are globally monotonic, satisfying the client's
        //     seq-dedup contract.
        //
        // After deploy_complete the connection is held with keep-alives (the
        // hook treats stream-end as a drop and reconnect-loops otherwise).
        app.MapGet("/api/projects/{projectId}/deployments/{deploymentId:guid}/events", async (
            string projectId, Guid deploymentId, HttpContext ctx, NetworkerDbContext db, EventBus bus) =>
        {
            var d = await db.Deployments
                .AsNoTracking()
                .Where(x => x.ProjectId == projectId && x.DeploymentId == deploymentId)
                .Select(x => new { x.Status, x.Log, x.EndpointIps })
                .FirstOrDefaultAsync();
            if (d is null)
            {
                return Results.NotFound();
            }

            var response = ctx.Response;
            response.Headers.ContentType = "text/event-stream";
            response.Headers.CacheControl = "no-cache";
            // Disable proxy buffering so events flush immediately (nginx et al).
            response.Headers["X-Accel-Buffering"] = "no";

            var ct = ctx.RequestAborted;
            try
            {
                var terminal = d.Status is "completed" or "failed" or "cancelled" or "torn_down";
                if (terminal)
                {
                    // Post-mortem replay straight from the deployment row.
                    long seq = 0;
                    foreach (var line in (d.Log ?? string.Empty).Split('\n'))
                    {
                        if (line.Length == 0)
                        {
                            continue;
                        }
                        await WriteEventAsync(response,
                            new SeqEvent(++seq, new DeployLog(deploymentId, line, "stdout")), ct);
                    }
                    var ips = ParseIpList(d.EndpointIps);
                    await WriteEventAsync(response,
                        new SeqEvent(++seq, new DeployComplete(deploymentId, d.Status, ips)), ct);
                    await response.Body.FlushAsync(ct);
                }
                else
                {
                    // Live tail: replay the ring, then poll it. `since` tracks
                    // the last GLOBAL seq scanned (not just matching events) so
                    // each poll is incremental.
                    long since = 0;
                    var complete = false;
                    while (!ct.IsCancellationRequested && !complete)
                    {
                        var batch = bus.Replay(since);
                        var wrote = false;
                        foreach (var e in batch)
                        {
                            since = e.Seq;
                            switch (e.Event)
                            {
                                case DeployLog dl when dl.DeploymentId == deploymentId:
                                    await WriteEventAsync(response, e, ct);
                                    wrote = true;
                                    break;
                                case DeployComplete dc when dc.DeploymentId == deploymentId:
                                    await WriteEventAsync(response, e, ct);
                                    wrote = true;
                                    complete = true;
                                    break;
                            }
                        }
                        if (wrote)
                        {
                            await response.Body.FlushAsync(ct);
                        }
                        if (!complete)
                        {
                            await Task.Delay(TimeSpan.FromSeconds(1), ct);
                        }
                    }
                }

                // Hold the stream open — the hook reconnect-loops on close.
                while (!ct.IsCancellationRequested)
                {
                    await Task.Delay(TimeSpan.FromSeconds(15), ct);
                    await response.WriteAsync(ServerSentEvents.FormatComment("keep-alive"), ct);
                    await response.Body.FlushAsync(ct);
                }
            }
            catch (OperationCanceledException)
            {
                // Client went away — normal SSE termination.
            }

            return Results.Empty;
        })
        .RequireAuthorization(AuthPolicies.ProjectMember);

        // GET /api/projects/{projectId}/deployments/{deploymentId}/capabilities
        // Live per-target test-support: probe each endpoint host's /health and
        // relay its `services` self-report mapped onto probe modes ("the
        // target must return the tests supported", 2026-08-13). Hosts that are
        // unreachable — or run a pre-0.28.202 endpoint without the report —
        // come back with supported_modes: null so the UI falls back to the
        // config-derived summary instead of trusting a fabricated list.
        app.MapGet("/api/projects/{projectId}/deployments/{deploymentId:guid}/capabilities", async (
            string projectId, Guid deploymentId, NetworkerDbContext db, LiveCapabilityCache liveCaps) =>
        {
            var d = await db.Deployments
                .AsNoTracking()
                .Where(x => x.ProjectId == projectId && x.DeploymentId == deploymentId)
                .Select(x => new { x.EndpointIps, x.Config })
                .FirstOrDefaultAsync();

            if (d is null)
            {
                return Results.NotFound();
            }

            var hosts = DeploymentWriteEndpoints.ParseHosts(d.EndpointIps);
            var probed = await Task.WhenAll(hosts.Select(TargetCapabilities.ProbeHostAsync));
            // Write-through: the config-create gate (rule 3, TestConfigWriteEndpoints)
            // reads this cache and never probes itself — a wizard that fetched
            // capabilities for its target has just informed the server too.
            foreach (var r in probed)
            {
                liveCaps.Store(r);
            }
            var reports = probed.Select(r => r.ToWire()).ToArray();
            // Static half of the answer: the proxy stack(s) this deployment
            // installed and whether each serves HTTP/3 (shared/http-stacks.json).
            // `stack` is http_stacks[0] — the listener a `proxy` config resolves
            // to (RunDispatcher) — so the UI can grey out h3 modes up front.
            var stacks = TargetCapabilities.StacksOf(d.Config);
            var primary = stacks.FirstOrDefault();
            return Results.Ok(new
            {
                endpoints = reports,
                stacks = stacks.Select(s => new { id = s, h3 = HttpStackCatalog.HasH3(s) }).ToArray(),
                stack = primary,
                stack_h3 = HttpStackCatalog.HasH3(primary),
                h3_modes = HttpStackCatalog.H3Modes.ToArray(),
                // This response IS a fresh probe (age 0); the server's create gate
                // will trust it for `live_capabilities_ttl_secs` and fail open
                // (kind / stack rules only) once it is older than that.
                live_capabilities_age_secs = 0,
                live_capabilities_ttl_secs = (int)liveCaps.Ttl.TotalSeconds,
            });
        })
        .RequireAuthorization(AuthPolicies.ProjectMember);

        // GET /api/projects/{projectId}/deployments/{deploymentId}/cost_estimate
        // Per-endpoint VM cost, priced by the same CostEstimation helpers the
        // tester cost endpoint uses so the two views can never disagree.
        // Deploy VMs have no auto-shutdown schedule → monthly is always-on.
        // Endpoints whose config carries no VM size (ssh/lan targets) are
        // listed with null cost rather than a made-up number.
        app.MapGet("/api/projects/{projectId}/deployments/{deploymentId:guid}/cost_estimate", async (
            string projectId, Guid deploymentId, NetworkerDbContext db) =>
        {
            var d = await db.Deployments
                .AsNoTracking()
                .Where(x => x.ProjectId == projectId && x.DeploymentId == deploymentId)
                .Select(x => new { x.Config })
                .FirstOrDefaultAsync();

            if (d is null)
            {
                return Results.NotFound();
            }

            var specs = ParseEndpointSpecs(d.Config);
            var shaped = new List<object>(specs.Count);
            var totalHourly = 0.0;
            var priced = 0;
            foreach (var s in specs)
            {
                double? hourly = null;
                if (s.VmSize is not null)
                {
                    hourly = await CostEstimation.HourlyUsdAsync(db, s.Provider, s.VmSize, s.Region);
                    totalHourly += hourly.Value;
                    priced++;
                }
                shaped.Add(new
                {
                    label = s.Label,
                    provider = s.Provider,
                    region = s.Region,
                    vm_size = s.VmSize,
                    os = s.Os,
                    vm_name = s.VmName,
                    hourly_usd = hourly,
                    monthly_usd = hourly.HasValue ? 24.0 * 30.0 * hourly.Value : (double?)null,
                });
            }

            return Results.Ok(new
            {
                endpoints = shaped,
                priced_endpoint_count = priced,
                total_hourly_usd = totalHourly,
                total_monthly_usd = 24.0 * 30.0 * totalHourly,
            });
        })
        .RequireAuthorization(AuthPolicies.ProjectMember);

        // GET /api/projects/{projectId}/cloud/status — aggregate cloud infra
        // status. Mirrors api/cloud.rs: reads cloud_account rows for the
        // project, grouped by provider. Never exposes credentials. SSH/LAN is
        // always available (no cloud account needed).
        app.MapGet("/api/projects/{projectId}/cloud/status", async (
            string projectId, NetworkerDbContext db) =>
        {
            var accounts = await db.CloudAccounts
                .AsNoTracking()
                .Where(c => c.ProjectId == projectId)
                .OrderBy(c => c.Provider)
                .ThenBy(c => c.Name)
                .Select(c => new { c.Provider, c.Name, c.Status })
                .ToListAsync();

            var azure = Unavailable();
            var aws = Unavailable();
            var gcp = Unavailable();

            foreach (var acc in accounts)
            {
                var ps = new
                {
                    available = true,
                    authenticated = acc.Status == "active",
                    account = (string?)acc.Name,
                };

                switch (acc.Provider.ToLowerInvariant())
                {
                    case "azure": azure = ps; break;
                    case "aws": aws = ps; break;
                    case "gcp": gcp = ps; break;
                }
            }

            return Results.Ok(new
            {
                azure,
                aws,
                gcp,
                ssh = new { available = true, authenticated = true, account = (string?)null },
            });
        })
        .RequireAuthorization(AuthPolicies.ProjectMember);

        return app;
    }

    private static object Unavailable() =>
        new { available = false, authenticated = false, account = (string?)null };

    /// <summary>One SSE frame carrying a <see cref="SeqEvent"/> in the flat
    /// {"seq":N,"type":"...",...} shape the deploy hook parses.</summary>
    private static Task WriteEventAsync(HttpResponse response, SeqEvent evt, CancellationToken ct) =>
        response.WriteAsync(ServerSentEvents.FormatEvent(null, JsonSerializer.Serialize(evt)), ct);

    /// <summary>Decode the deployment's endpoint_ips JSON array (null/invalid ⇒ empty).</summary>
    private static IReadOnlyList<string> ParseIpList(string? raw)
    {
        if (string.IsNullOrWhiteSpace(raw))
        {
            return [];
        }
        try
        {
            return JsonSerializer.Deserialize<List<string>>(raw) ?? [];
        }
        catch (JsonException)
        {
            return [];
        }
    }

    /// <summary>One deployment-config endpoint resolved for costing/identity.
    /// The VM size field name is provider-specific in the config JSON:
    /// azure <c>vm_size</c>, aws <c>instance_type</c>, gcp <c>machine_type</c> —
    /// the first present wins. Region falls back to <c>zone</c> (gcp).
    /// Cloud deploys nest these inside the per-provider block
    /// (<c>ep.azure.region</c> …), not at the endpoint top level — top level is
    /// checked first, then the provider block.</summary>
    internal sealed record EndpointSpec(string Label, string Provider, string? Region, string? VmSize, string? Os, string? VmName);

    /// <summary>Parse a deployment's raw config JSON into per-endpoint specs.
    /// Tolerant by design: bad JSON, a missing <c>endpoints</c> array, or
    /// non-object entries yield an empty/partial list, never a throw.</summary>
    internal static List<EndpointSpec> ParseEndpointSpecs(string? rawConfig)
    {
        var list = new List<EndpointSpec>();
        if (string.IsNullOrWhiteSpace(rawConfig))
        {
            return list;
        }

        JsonNode? root;
        try
        {
            root = JsonNode.Parse(rawConfig);
        }
        catch (JsonException)
        {
            return list;
        }

        if (root?["endpoints"] is not JsonArray endpoints)
        {
            return list;
        }

        var i = 0;
        foreach (var node in endpoints)
        {
            i++;
            if (node is not JsonObject ep)
            {
                continue;
            }

            string? Top(string key) =>
                ep[key] is JsonValue v && v.TryGetValue<string>(out var s) && !string.IsNullOrWhiteSpace(s) ? s : null;

            var provider = Top("provider") ?? "unknown";

            // Cloud configs nest region/vm_size/os inside the provider block
            // (mirrors BuildProviderSummary's "try both" rule).
            var block = ep[provider] as JsonObject;
            string? Nested(string key) =>
                block?[key] is JsonValue v && v.TryGetValue<string>(out var s) && !string.IsNullOrWhiteSpace(s) ? s : null;
            string? Field(params string[] keys)
            {
                foreach (var k in keys) { if (Top(k) is { } t) return t; }
                foreach (var k in keys) { if (Nested(k) is { } n) return n; }
                return null;
            }

            list.Add(new EndpointSpec(
                Label: Top("label") ?? $"endpoint {i}",
                Provider: provider,
                Region: Field("region", "zone"),
                VmSize: Field("vm_size", "instance_type", "machine_type"),
                Os: Field("os"),
                VmName: Field("vm_name", "instance_name")));
        }

        return list;
    }

    /// <summary>Shape a <see cref="Data.Entities.Deployment"/> to the snake_case
    /// DeploymentRow JSON contract, decoding the JSON-text columns (config,
    /// endpoint_ips, endpoint_hosts) to real JSON nodes rather than escaped strings.</summary>
    private static object ShapeDeployment(Data.Entities.Deployment d) => new
    {
        deployment_id = d.DeploymentId,
        name = d.Name,
        status = d.Status,
        config = ParseJson(d.Config),
        provider_summary = d.ProviderSummary,
        created_by = d.CreatedBy,
        created_at = d.CreatedAt,
        started_at = d.StartedAt,
        finished_at = d.FinishedAt,
        endpoint_ips = ParseJson(d.EndpointIps),
        // V050: per-endpoint DNS names, parallel to endpoint_ips (null entries
        // where the provider gave none) — what a proxy run connects to.
        endpoint_hosts = ParseJson(d.EndpointHosts),
        agent_id = d.AgentId,
        error_message = d.ErrorMessage,
        log = d.Log,
        // V054 (issue #816): install.sh's raw exit code and the last "Step N: …"
        // header before a failure — post-mortem triage without reading the log.
        exit_code = d.ExitCode,
        failed_step = d.FailedStep,
    };

    private static JsonNode? ParseJson(string? raw)
    {
        if (string.IsNullOrWhiteSpace(raw))
        {
            return null;
        }

        try
        {
            return JsonNode.Parse(raw);
        }
        catch (JsonException)
        {
            return null;
        }
    }
}
