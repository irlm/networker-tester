using System.Text.Json;
using System.Text.Json.Nodes;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
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
    /// endpoint_ips) to real JSON nodes rather than escaped strings.</summary>
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
        agent_id = d.AgentId,
        error_message = d.ErrorMessage,
        log = d.Log,
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
