using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Provisioning;
using Networker.ControlPlane.Realtime;
using Networker.ControlPlane.Security;
using Networker.Data;

namespace Networker.ControlPlane.Endpoints;

// Agent api-key rotation (V044) for TesterWriteEndpoints — the route mapping
// lives in TesterWriteEndpoints.cs.
public static partial class TesterWriteEndpoints
{
    /// <summary>
    /// POST /api/projects/{projectId}/testers/{testerId}/rotate-key — mint a
    /// fresh agent api-key for the agent bound to this tester.
    ///
    /// <para><b>Project scope (mandatory):</b> the tester must belong to
    /// <paramref name="projectId"/> AND the agent row must also have
    /// <c>project_id == projectId</c>. An operator can only rotate agents in
    /// THEIR project; a foreign/unknown tester or an unlinked tester is a flat
    /// <c>404</c> (never a 403 existence oracle). The <c>ProjectOperator</c>
    /// policy already gates the route on the caller's role in the project.</para>
    ///
    /// <para><b>Behaviour:</b> generate a new 48-char CSPRNG key (the same
    /// <see cref="TesterCreateLogic.GenerateAgentApiKey"/> used at provision),
    /// replace both <c>api_key</c> and <c>api_key_hash</c> (so the OLD key's
    /// hash no longer matches and it is instantly dead), and clear
    /// <c>api_key_expires_at</c> (a rotated key starts with no expiry — never
    /// break the fleet). The agent's live WS connection, if any, is dropped so
    /// it must reconnect with the new key. The new plaintext key is returned
    /// <b>once</b> in the response and never stored/re-shown — exactly like an
    /// invite token.</para>
    /// </summary>
    private static async Task<IResult> RotateAgentKey(
        string projectId,
        Guid testerId,
        NetworkerDbContext db,
        AgentConnectionRegistry registry,
        ILoggerFactory loggerFactory,
        HttpContext http,
        CancellationToken ct)
    {
        var logger = loggerFactory.CreateLogger("TesterWrite.RotateKey");
        var user = http.GetAuthUser();

        // Tester must exist in this project (flat 404 for missing/foreign).
        var tester = await db.ProjectTesters.AsNoTracking()
            .FirstOrDefaultAsync(t => t.ProjectId == projectId && t.TesterId == testerId, ct);
        if (tester is null)
        {
            return ApiError.NotFound("tester not found in this project");
        }

        // The agent bound to this tester — scoped to the SAME project so an
        // operator can never rotate an agent outside their project.
        var agent = await db.Agents.AsTracking()
            .FirstOrDefaultAsync(a => a.TesterId == testerId && a.ProjectId == projectId, ct);
        if (agent is null)
        {
            return ApiError.NotFound("no agent is linked to this tester");
        }

        var newKey = TesterCreateLogic.GenerateAgentApiKey();
        // Only the hash is persisted (auth looks up api_key_hash; the plaintext
        // column was dropped in V045). The new key is returned once below.
        agent.ApiKeyHash = AgentApiKeys.HashHex(newKey);
        agent.ApiKeyExpiresAt = null; // rotated keys start with no expiry
        await db.SaveChangesAsync(ct);

        // Drop the agent's live connection so it re-authenticates with the new
        // key (best-effort: no-op when the agent is offline).
        var dropped = await registry.ShutdownAsync(agent.AgentId, ct);

        logger.LogInformation(
            "agent api-key rotated: project_id={ProjectId} tester_id={TesterId} agent_id={AgentId} "
            + "actor_user_id={ActorUserId} live_connection_dropped={Dropped}",
            projectId, testerId, agent.AgentId, user?.UserId, dropped);

        // Return the new plaintext key ONCE — never serialized again.
        return Results.Ok(new
        {
            agent_id = agent.AgentId,
            tester_id = testerId,
            api_key = newKey,
            api_key_expires_at = (DateTime?)null,
        });
    }
}
