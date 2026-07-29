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
                return ApiError.BadRequest("name and config are required");
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

            // E2E P3-12: this used to TCP-probe only and stub version:null /
            // outdated:false — a VM that slept through releases (observed live:
            // an endpoint still on 0.28.66) showed as perfectly current. Reuse
            // the version-summary probe (HTTPS :8443 + HTTP :8080 /health,
            // self-signed accepted, 1.5s budget) and compare against this
            // control plane's own version — same release train, so any mismatch
            // means the binary is stale and /update will refresh it.
            var hosts = ParseHosts(d.EndpointIps);
            var current = VersionEndpoints.DashboardVersion;
            var probes = await Task.WhenAll(
                hosts.Select(h => VersionEndpoints.ProbeEndpointVersionAsync(h)));
            var results = probes.Select(p => (object)new
            {
                ip = p.host,
                alive = p.reachable,
                version = p.version,
                outdated = p.reachable && p.version is not null && p.version != current,
            }).ToList();

            return Results.Ok(new { endpoints = results, latest_release = current });
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
        // deployment record AND tear down its cloud VM(s). Admin-only (tightened
        // from the Rust Operator per M4 scope).
        //
        // P1-16 (E2E pass 2026-07-28): deleting a deployment used to drop only the
        // DB row, orphaning the endpoint VM to bill until the orphan reaper (or
        // forever, if it was never reaper-eligible). Deploy VMs are created by
        // install.sh, so — unlike a tester — the row never stored a resource id;
        // all it has is the endpoint IP/FQDN. We reverse-look-up the VM by that
        // endpoint and tear it down. The endpoint list + provider are captured
        // BEFORE the row is deleted (so deleting it can't lose the teardown
        // inputs); the actual cloud delete is backgrounded so the request returns
        // promptly and a cloud hiccup can't fail the DB delete (the reaper remains
        // the backstop).
        app.MapDelete("/api/projects/{projectId}/deployments/{deploymentId:guid}", async (
            string projectId,
            Guid deploymentId,
            NetworkerDbContext db,
            IServiceScopeFactory scopeFactory,
            ILoggerFactory loggerFactory,
            CancellationToken ct) =>
        {
            var deployment = await db.Deployments
                .AsNoTracking()
                .FirstOrDefaultAsync(d => d.ProjectId == projectId && d.DeploymentId == deploymentId, ct);
            if (deployment is null)
            {
                return Results.NotFound();
            }

            // Capture teardown inputs before the row goes away.
            var endpoints = ParseHosts(deployment.EndpointIps);
            string? provider = null;
            string? region = null;
            if (deployment.CloudAccountId is { } accountId)
            {
                var acct = await db.CloudAccounts.AsNoTracking()
                    .Where(a => a.AccountId == accountId)
                    .Select(a => new { a.Provider, a.RegionDefault })
                    .FirstOrDefaultAsync(ct);
                provider = acct?.Provider;
                region = acct?.RegionDefault;
            }
            provider ??= FirstProviderFromConfig(deployment.Config);

            var deleted = await db.Deployments
                .Where(d => d.ProjectId == projectId && d.DeploymentId == deploymentId)
                .ExecuteDeleteAsync(ct);
            if (deleted == 0)
            {
                return Results.NotFound();
            }

            if (!string.IsNullOrEmpty(provider) && endpoints.Count > 0)
            {
                SpawnVmTeardown(scopeFactory, loggerFactory, deploymentId, provider!, region, endpoints);
            }

            return Results.Ok(new { deleted = true });
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

    /// <summary>First endpoint provider from a deploy.json config
    /// (<c>endpoints[0].provider</c>) — the fallback when the deployment has no
    /// <c>cloud_account_id</c> to read the provider from. Returns null if absent.</summary>
    internal static string? FirstProviderFromConfig(string? config)
    {
        if (string.IsNullOrWhiteSpace(config))
        {
            return null;
        }
        try
        {
            using var doc = JsonDocument.Parse(config);
            if (doc.RootElement.ValueKind == JsonValueKind.Object
                && doc.RootElement.TryGetProperty("endpoints", out var eps)
                && eps.ValueKind == JsonValueKind.Array)
            {
                foreach (var ep in eps.EnumerateArray())
                {
                    if (ep.ValueKind == JsonValueKind.Object
                        && ep.TryGetProperty("provider", out var p)
                        && p.ValueKind == JsonValueKind.String
                        && p.GetString() is { Length: > 0 } provider)
                    {
                        return provider;
                    }
                }
            }
        }
        catch (JsonException)
        {
            // malformed config — no provider to derive
        }
        return null;
    }

    /// <summary>
    /// Background-tear-down the deployment's cloud VM(s) (P1-16). For each captured
    /// endpoint (IP/FQDN), reverse-resolve the owning VM and delete it via the
    /// provisioner (which cascades the per-VM Azure NSG/IP). Detached from the
    /// request and fully best-effort: every failure is logged and left to the
    /// orphan reaper, never surfaced. Runs in its own DI scope (the request's
    /// DbContext is already disposed with the response).
    /// </summary>
    private static void SpawnVmTeardown(
        IServiceScopeFactory scopeFactory,
        ILoggerFactory loggerFactory,
        Guid deploymentId,
        string provider,
        string? region,
        IReadOnlyList<string> endpoints)
    {
        var logger = loggerFactory.CreateLogger("DeploymentWriteEndpoints.teardown");
        _ = Task.Run(async () =>
        {
            try
            {
                using var scope = scopeFactory.CreateScope();
                var provisioner = scope.ServiceProvider.GetRequiredService<Provisioning.IComputeProvisioner>();
                // No stored per-connection credentials for a deploy row; the
                // control plane manages the endpoint RG via ambient auth (managed
                // identity), the same way install.sh created the VM. The delete's
                // NSG/IP cascade derives subscription+RG from the resolved VM's
                // resource id, so ambient creds (region only) are sufficient.
                var creds = new Provisioning.ProviderCredentials(provider, Region: region);

                foreach (var endpoint in endpoints)
                {
                    var vm = await provisioner
                        .ResolveByEndpointAsync(provider, creds, endpoint, CancellationToken.None)
                        .ConfigureAwait(false);
                    if (vm is null)
                    {
                        continue; // unsupported provider / already gone / no match — logged inside
                    }

                    // Synthesise the minimal tester the provisioner's DeleteAsync
                    // needs: it reads only Cloud + VmResourceId + VmName (and
                    // derives subscription/RG from the resource id for the cascade).
                    var synthetic = new Networker.Data.Entities.ProjectTester
                    {
                        Cloud = provider,
                        Region = region ?? string.Empty,
                        VmResourceId = vm.ResourceId,
                        VmName = vm.Name,
                    };

                    var res = await provisioner.DeleteAsync(synthetic, creds, CancellationToken.None).ConfigureAwait(false);
                    if (res.Success)
                    {
                        logger.LogInformation(
                            "Deployment {DeploymentId}: torn down VM {VmName} (endpoint {Endpoint})",
                            deploymentId, vm.Name, endpoint);
                    }
                    else
                    {
                        logger.LogWarning(
                            "Deployment {DeploymentId}: VM {VmName} teardown did not succeed ({Err}); leaving for the reaper",
                            deploymentId, vm.Name, res.Error ?? res.StdErr);
                    }
                }
            }
            catch (Exception ex)
            {
                logger.LogError(ex, "Deployment {DeploymentId} VM teardown threw", deploymentId);
            }
        });
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


    /// <summary>Create-deployment request body: a name + the deploy.json config.</summary>
    public sealed record CreateDeploymentRequest(string Name, JsonObject Config);
}
