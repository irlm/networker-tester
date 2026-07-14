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
