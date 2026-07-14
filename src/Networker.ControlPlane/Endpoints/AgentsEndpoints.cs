using System.Text.Json;
using System.Text.Json.Nodes;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Read-only agent endpoints, mirroring the Rust dashboard's
/// <c>api/agents.rs</c> project-scoped GET handlers. Field names are emitted in
/// snake_case to match the existing REST contract consumed by the React
/// frontend. The agent <c>api_key</c> is NEVER serialized.
/// </summary>
public static class AgentsEndpoints
{
    public static IEndpointRouteBuilder MapAgentsEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/projects/{projectId}/agents — list agents in the project.
        // Mirrors AgentRow from crates/networker-dashboard/src/db/agents.rs
        // (minus api_key, which is intentionally never exposed).
        app.MapGet("/api/projects/{projectId}/agents", async (string projectId, NetworkerDbContext db) =>
        {
            var agents = await db.Agents
                .AsNoTracking()
                .Where(a => a.ProjectId == projectId)
                .OrderBy(a => a.Name)
                .Select(a => new
                {
                    a.AgentId,
                    a.Name,
                    a.Region,
                    a.Provider,
                    a.Status,
                    a.Version,
                    a.Os,
                    a.Arch,
                    a.LastHeartbeat,
                    a.RegisteredAt,
                    a.Tags,
                    a.TesterId,
                })
                .ToListAsync();

            var shaped = agents.Select(a => new
            {
                agent_id = a.AgentId,
                name = a.Name,
                region = a.Region,
                provider = a.Provider,
                status = a.Status,
                version = a.Version,
                os = a.Os,
                arch = a.Arch,
                last_heartbeat = a.LastHeartbeat,
                registered_at = a.RegisteredAt,
                tags = ParseJson(a.Tags),
                tester_id = a.TesterId,
            });

            return Results.Ok(shaped);
        })
        .RequireAuthorization(AuthPolicies.ProjectMember);

        // GET /api/projects/{projectId}/agents/{agentId} — agent detail.
        // The Rust router only exposes list/create/delete for agents; a detail
        // GET is added here for read parity. Adds a computed `online` flag
        // (heartbeat within 90s, matching typical agent liveness windows) and a
        // `last_seen` alias so the UI can render presence.
        app.MapGet("/api/projects/{projectId}/agents/{agentId:guid}", async (
            string projectId, Guid agentId, NetworkerDbContext db) =>
        {
            var a = await db.Agents
                .AsNoTracking()
                .Where(x => x.ProjectId == projectId && x.AgentId == agentId)
                .Select(x => new
                {
                    x.AgentId,
                    x.Name,
                    x.Region,
                    x.Provider,
                    x.Status,
                    x.Version,
                    x.Os,
                    x.Arch,
                    x.LastHeartbeat,
                    x.RegisteredAt,
                    x.Tags,
                    x.TesterId,
                })
                .FirstOrDefaultAsync();

            if (a is null)
            {
                return Results.NotFound();
            }

            var online = a.LastHeartbeat is { } hb
                && DateTime.UtcNow - DateTime.SpecifyKind(hb, DateTimeKind.Utc) < TimeSpan.FromSeconds(90);

            return Results.Ok(new
            {
                agent_id = a.AgentId,
                name = a.Name,
                region = a.Region,
                provider = a.Provider,
                status = a.Status,
                version = a.Version,
                os = a.Os,
                arch = a.Arch,
                last_heartbeat = a.LastHeartbeat,
                last_seen = a.LastHeartbeat,
                registered_at = a.RegisteredAt,
                tags = ParseJson(a.Tags),
                tester_id = a.TesterId,
                online,
            });
        })
        .RequireAuthorization(AuthPolicies.ProjectMember);

        return app;
    }

    /// <summary>
    /// Parse a nullable JSON text column into a <see cref="JsonNode"/> so it
    /// serializes as a real object/array rather than an escaped string,
    /// matching the Rust <c>serde_json::Value</c> passthrough. Returns null on
    /// null/blank/invalid input.
    /// </summary>
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
