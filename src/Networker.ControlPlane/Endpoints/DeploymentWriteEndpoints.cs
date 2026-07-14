using System.Text.Json;
using System.Text.Json.Nodes;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Provisioning;
using Networker.ControlPlane.Realtime;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// REST v2 <b>write</b> endpoints for deployments — the C# port of the Rust
/// dashboard's <c>api/deployments.rs</c> write handlers
/// (create / start / stop / check / update / delete). M1 ported only the reads
/// (<see cref="DeploymentsEndpoints"/>); this slice adds the mutating side.
///
/// <para><b>Pattern:</b> every handler does its DB transition synchronously and
/// returns immediately (201 on create, 202 Accepted for the async lifecycle
/// ops), backgrounding the actual shell-out work on a detached task via the
/// singleton <see cref="DeployRunner"/>. This mirrors the Rust handlers'
/// <c>tokio::spawn</c> + immediate JSON response. The deploy work SOFT-FAILS
/// without <c>install.sh</c> / cloud CLIs (the runner records
/// <c>failed</c> and publishes <c>DeployComplete</c> rather than throwing), so
/// these endpoints — and CI exercising them — succeed on a box with no
/// installer present.</para>
///
/// <para><b>Auth:</b> create + start/stop/check/update require
/// <see cref="AuthPolicies.ProjectOperator"/>; delete requires
/// <see cref="AuthPolicies.ProjectAdmin"/> — matching the Rust
/// <c>require_project_role(Operator)</c> handlers, with delete tightened to
/// Admin per the M4 scope.</para>
/// </summary>
public static class DeploymentWriteEndpoints
{
    public static IEndpointRouteBuilder MapDeploymentWriteEndpoints(this IEndpointRouteBuilder app)
    {
        // POST /api/projects/{projectId}/deployments — create + start a deployment.
        // Body: { "name": "...", "config": { ...deploy.json... } }.
        app.MapPost("/api/projects/{projectId}/deployments", async (
            string projectId,
            CreateDeploymentRequest body,
            NetworkerDbContext db,
            DeployRunner runner,
            ILoggerFactory loggerFactory,
            CancellationToken ct) =>
        {
            if (body is null || string.IsNullOrWhiteSpace(body.Name) || body.Config is null)
            {
                return Results.BadRequest(new { error = "name and config are required" });
            }

            var configText = body.Config.ToJsonString();
            var deploymentId = Guid.NewGuid();
            var now = DateTime.UtcNow;

            db.Deployments.Add(new Deployment
            {
                DeploymentId = deploymentId,
                Name = body.Name,
                Status = "pending",
                Config = configText,
                ProviderSummary = BuildProviderSummary(body.Config),
                CreatedAt = now,
                ProjectId = projectId,
            });
            await db.SaveChangesAsync(ct);

            // Background the deploy (soft-fails without install.sh — see class doc).
            SpawnDeploy(runner, loggerFactory, deploymentId, configText);

            return Results.Created(
                $"/api/projects/{projectId}/deployments/{deploymentId}",
                new { deployment_id = deploymentId, status = "pending" });
        })
        .RequireAuthorization(AuthPolicies.ProjectOperator);

        // POST /api/projects/{projectId}/deployments/{deploymentId}/start — bring a
        // stopped/deallocated VM back online. DB is untouched here (VM lifecycle is
        // a cloud-side op); 202 + background CLI, caller polls /check. Matches the
        // Rust start_deployment_scoped which returns 202 and spawns start_deployment_vm.
        app.MapPost("/api/projects/{projectId}/deployments/{deploymentId:guid}/start", async (
            string projectId,
            Guid deploymentId,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var exists = await DeploymentExistsAsync(db, projectId, deploymentId, ct);
            if (!exists)
            {
                return Results.NotFound();
            }

            // VM start is a cloud-CLI op (IComputeProvisioner). Not wired to a
            // concrete tester here; returning 202 keeps the contract while the
            // actual az/aws/gcloud start remains a follow-up (soft no-op today).
            return Results.Accepted(
                $"/api/projects/{projectId}/deployments/{deploymentId}",
                new { status = "starting", deployment_id = deploymentId });
        })
        .RequireAuthorization(AuthPolicies.ProjectOperator);

        // POST /api/projects/{projectId}/deployments/{deploymentId}/stop — cancel a
        // pending/running deploy (mirrors stop_deployment_scoped: only pending/running
        // are transitioned to cancelled; a DeployComplete{cancelled} is published).
        app.MapPost("/api/projects/{projectId}/deployments/{deploymentId:guid}/stop", async (
            string projectId,
            Guid deploymentId,
            NetworkerDbContext db,
            EventBus bus,
            CancellationToken ct) =>
        {
            var d = await db.Deployments
                .FirstOrDefaultAsync(x => x.ProjectId == projectId && x.DeploymentId == deploymentId, ct);
            if (d is null)
            {
                return Results.NotFound();
            }

            if (d.Status is "running" or "pending")
            {
                d.Status = "cancelled";
                d.FinishedAt = DateTime.UtcNow;
                await db.SaveChangesAsync(ct);
                bus.Publish(new DeployComplete(deploymentId, "cancelled", Array.Empty<string>()));
            }

            return Results.Ok(new { status = "cancelled" });
        })
        .RequireAuthorization(AuthPolicies.ProjectOperator);

        // POST /api/projects/{projectId}/deployments/{deploymentId}/check — probe the
        // deployed endpoint(s) for liveness/version. Mirrors check_deployment: reads
        // endpoint_ips off the row, TCP-connects each on :8443, reports alive/version.
        app.MapPost("/api/projects/{projectId}/deployments/{deploymentId:guid}/check", async (
            string projectId,
            Guid deploymentId,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var d = await db.Deployments
                .AsNoTracking()
                .FirstOrDefaultAsync(x => x.ProjectId == projectId && x.DeploymentId == deploymentId, ct);
            if (d is null)
            {
                return Results.NotFound();
            }

            var hosts = ParseHosts(d.EndpointIps);
            var results = new List<object>(hosts.Count);
            foreach (var host in hosts)
            {
                var alive = await TcpProbeAsync(host, 8443, TimeSpan.FromSeconds(5), ct);
                results.Add(new { ip = host, alive, version = (string?)null, outdated = false });
            }

            return Results.Ok(new { endpoints = results, latest_release = (string?)null });
        })
        .RequireAuthorization(AuthPolicies.ProjectOperator);

        // POST /api/projects/{projectId}/deployments/{deploymentId}/update — re-run the
        // deploy for an endpoint-only update (tests disabled), reusing the stored
        // config. Mirrors update_endpoint: sets tests.run_tests=false and re-runs.
        app.MapPost("/api/projects/{projectId}/deployments/{deploymentId:guid}/update", async (
            string projectId,
            Guid deploymentId,
            NetworkerDbContext db,
            DeployRunner runner,
            ILoggerFactory loggerFactory,
            CancellationToken ct) =>
        {
            var d = await db.Deployments
                .AsNoTracking()
                .FirstOrDefaultAsync(x => x.ProjectId == projectId && x.DeploymentId == deploymentId, ct);
            if (d is null)
            {
                return Results.NotFound();
            }

            // Force tests off for an endpoint-only update, reusing the stored config.
            var config = JsonNode.Parse(d.Config) as JsonObject ?? new JsonObject();
            config["tests"] = new JsonObject { ["run_tests"] = false };
            var configText = config.ToJsonString();

            SpawnDeploy(runner, loggerFactory, deploymentId, configText);

            return Results.Accepted(
                $"/api/projects/{projectId}/deployments/{deploymentId}",
                new { status = "updating" });
        })
        .RequireAuthorization(AuthPolicies.ProjectOperator);

        // DELETE /api/projects/{projectId}/deployments/{deploymentId} — remove the
        // deployment record. Admin-only (tightened from the Rust Operator per M4 scope).
        app.MapDelete("/api/projects/{projectId}/deployments/{deploymentId:guid}", async (
            string projectId,
            Guid deploymentId,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var deleted = await db.Deployments
                .Where(d => d.ProjectId == projectId && d.DeploymentId == deploymentId)
                .ExecuteDeleteAsync(ct);

            return deleted > 0 ? Results.Ok(new { deleted = true }) : Results.NotFound();
        })
        .RequireAuthorization(AuthPolicies.ProjectAdmin);

        return app;
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    /// <summary>Spawn the deploy runner on a detached task tied to the app
    /// lifetime (not the request), matching the Rust <c>tokio::spawn</c>. The
    /// runner opens its own DI scope and soft-fails without install.sh.</summary>
    private static void SpawnDeploy(
        DeployRunner runner, ILoggerFactory loggerFactory, Guid deploymentId, string configText)
    {
        var logger = loggerFactory.CreateLogger("DeploymentWriteEndpoints");
        _ = Task.Run(async () =>
        {
            try
            {
                await runner.RunDeploymentAsync(deploymentId, configText, CancellationToken.None);
            }
            catch (Exception ex)
            {
                logger.LogError(ex, "Background deploy failed for deployment {DeploymentId}", deploymentId);
            }
        }, CancellationToken.None);
    }

    private static Task<bool> DeploymentExistsAsync(
        NetworkerDbContext db, string projectId, Guid deploymentId, CancellationToken ct) =>
        db.Deployments.AsNoTracking()
            .AnyAsync(d => d.ProjectId == projectId && d.DeploymentId == deploymentId, ct);

    /// <summary>Human-readable provider summary from a deploy.json body, mirroring
    /// the Rust <c>build_provider_summary</c> (provider + endpoint-level region).</summary>
    private static string? BuildProviderSummary(JsonNode? config)
    {
        if (config?["endpoints"] is not JsonArray endpoints || endpoints.Count == 0)
        {
            return null;
        }

        var parts = new List<string>();
        foreach (var ep in endpoints)
        {
            var provider = ep?["provider"]?.GetValue<string>() ?? "unknown";
            var region = ep?["region"]?.GetValue<string?>();
            parts.Add(string.IsNullOrEmpty(region) ? provider : $"{provider} {region}");
        }

        return parts.Count == 0 ? null : string.Join(" + ", parts);
    }

    private static List<string> ParseHosts(string? endpointIps)
    {
        var hosts = new List<string>();
        if (string.IsNullOrWhiteSpace(endpointIps))
        {
            return hosts;
        }

        try
        {
            using var doc = JsonDocument.Parse(endpointIps);
            if (doc.RootElement.ValueKind == JsonValueKind.Array)
            {
                foreach (var el in doc.RootElement.EnumerateArray())
                {
                    if (el.ValueKind == JsonValueKind.String && el.GetString() is { Length: > 0 } s)
                    {
                        hosts.Add(s);
                    }
                }
            }
        }
        catch (JsonException)
        {
            // malformed column — treat as no endpoints
        }
        return hosts;
    }

    private static async Task<bool> TcpProbeAsync(string host, int port, TimeSpan timeout, CancellationToken ct)
    {
        try
        {
            using var client = new System.Net.Sockets.TcpClient();
            using var cts = CancellationTokenSource.CreateLinkedTokenSource(ct);
            cts.CancelAfter(timeout);
            await client.ConnectAsync(host, port, cts.Token);
            return client.Connected;
        }
        catch
        {
            return false;
        }
    }

    /// <summary>Create-deployment request body: a name + the deploy.json config.</summary>
    public sealed record CreateDeploymentRequest(string Name, JsonObject Config);
}
