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
    /// the first present wins. Region falls back to <c>zone</c> (gcp).</summary>
    internal sealed record EndpointSpec(string Label, string Provider, string? Region, string? VmSize, string? Os);

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

            string? Str(string key) =>
                ep[key] is JsonValue v && v.TryGetValue<string>(out var s) && !string.IsNullOrWhiteSpace(s) ? s : null;

            list.Add(new EndpointSpec(
                Label: Str("label") ?? $"endpoint {i}",
                Provider: Str("provider") ?? "unknown",
                Region: Str("region") ?? Str("zone"),
                VmSize: Str("vm_size") ?? Str("instance_type") ?? Str("machine_type"),
                Os: Str("os")));
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
