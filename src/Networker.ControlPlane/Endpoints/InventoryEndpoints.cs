using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// C# port of the Rust dashboard's <c>api/inventory.rs</c> cloud resource scan.
/// The route is project-scoped (mounted in the Rust <c>project_scoped</c> group →
/// ProjectMember / Viewer role) even though the underlying scan is global.
///
/// <para>Route: <b>GET /api/projects/{projectId}/inventory</b> → response
/// <c>{ vms: [CloudVm...], errors: [string...] }</c>.</para>
///
/// <para>Each <c>CloudVm</c> is snake_case: provider, name, region, status,
/// public_ip, fqdn, vm_size, os, resource_group, managed.</para>
///
/// <para><b>Cloud-scan stub (TODO(phase3)):</b> the Rust handler shells out to
/// <c>az</c> / <c>aws</c> / <c>gcloud</c> in parallel to enumerate networker VMs.
/// Those CLIs are not available in the C# ControlPlane / CI, so the live scan is
/// stubbed (like the M4 validate endpoints): <c>vms</c> is empty and <c>errors</c>
/// is empty (matching the Rust behavior when a provider CLI is "not installed" —
/// those errors are filtered out and not surfaced). The endpoint + response shape
/// are faithful, and no fabricated VM data is returned.</para>
///
/// <para>The managed-host cross-reference (from the <c>deployments</c> table,
/// newest 100) IS ported via EF, and <see cref="IsManaged"/> is unit-tested, so
/// the logic is exercised for when a real scan is wired up later.</para>
/// </summary>
public static class InventoryEndpoints
{
    public static IEndpointRouteBuilder MapInventoryEndpoints(this IEndpointRouteBuilder app)
    {
        app.MapGet("/api/projects/{projectId}/inventory", async (
            string projectId,
            NetworkerDbContext db,
            ILoggerFactory lf,
            CancellationToken ct) =>
        {
            // Managed deployment IPs for cross-referencing (list_all: newest 100).
            var managedHosts = await GetManagedHostsAsync(db, ct);

            // TODO(phase3): run the real az/aws/gcp scan. Stubbed — no CLIs in
            // this environment. Empty vms + empty errors mirrors the Rust output
            // when every provider CLI is absent (those errors are filtered out).
            lf.CreateLogger("Networker.Inventory").LogInformation(
                "inventory scan requested (project {ProjectId}) — cloud scan is a phase-3 stub; {Count} managed hosts loaded",
                projectId, managedHosts.Count);

            return Results.Ok(new
            {
                vms = Array.Empty<CloudVm>(),
                errors = Array.Empty<string>(),
            });
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        return app;
    }

    private static async Task<List<string>> GetManagedHostsAsync(NetworkerDbContext db, CancellationToken ct)
    {
        List<string?> endpointIpsJson;
        try
        {
            endpointIpsJson = await db.Deployments
                .AsNoTracking()
                .OrderByDescending(d => d.CreatedAt)
                .Take(100)
                .Select(d => d.EndpointIps)
                .ToListAsync(ct);
        }
        catch (Exception)
        {
            return new List<string>();
        }

        var hosts = new List<string>();
        foreach (var raw in endpointIpsJson)
        {
            if (string.IsNullOrEmpty(raw))
            {
                continue;
            }
            try
            {
                var arr = JsonSerializer.Deserialize<List<string>>(raw);
                if (arr is not null)
                {
                    hosts.AddRange(arr);
                }
            }
            catch (JsonException)
            {
                // Not a JSON string array — skip (matches Rust filter_map).
            }
        }
        return hosts;
    }

    /// <summary>
    /// True when the VM's fqdn or public_ip matches (exact or substring) any
    /// managed host. Mirrors the Rust <c>is_managed</c>.
    /// </summary>
    public static bool IsManaged(string? fqdn, string? publicIp, IReadOnlyList<string> managedHosts)
    {
        if (fqdn is { } dns && managedHosts.Any(h => h == dns || h.Contains(dns, StringComparison.Ordinal)))
        {
            return true;
        }
        if (publicIp is { } ip && managedHosts.Any(h => h == ip || h.Contains(ip, StringComparison.Ordinal)))
        {
            return true;
        }
        return false;
    }

    public sealed class CloudVm
    {
        public string provider { get; set; } = string.Empty;
        public string name { get; set; } = string.Empty;
        public string region { get; set; } = string.Empty;
        public string status { get; set; } = string.Empty;
        public string? public_ip { get; set; }
        public string? fqdn { get; set; }
        public string? vm_size { get; set; }
        public string? os { get; set; }
        public string? resource_group { get; set; }
        public bool managed { get; set; }
    }
}
