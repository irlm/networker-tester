using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.Json.Serialization;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Provisioning;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Benchmark VM catalog endpoints — the C# port of the Rust dashboard's
/// <c>api/benchmark_catalog.rs</c> (list / register / remove / detect-languages).
/// Response field names are snake_case, matching the Rust
/// <c>db::benchmark_vm_catalog::VmCatalogRow</c> serde output.
///
/// <para><b>Divergences (documented):</b></para>
/// <list type="bullet">
///   <item><b>DELETE requires ProjectAdmin</b> (per the M5 spec); the Rust
///   handler only required Operator. Registration stays Operator in both.</item>
///   <item><b>Detect</b> mirrors the Rust handler: SSH into the VM
///   synchronously (<see cref="Provisioning.SshLanguageDetector"/>), probe
///   /opt/bench/* for each language runtime, persist the detected list +
///   online/offline status (update_status also stamps last_health_check, as
///   the Rust SQL did), and return 200 <c>{languages, status}</c>.</item>
/// </list>
/// </summary>
public static class BenchmarkCatalogEndpoints
{
    public static IEndpointRouteBuilder MapBenchmarkCatalogEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/projects/{projectId}/benchmark-catalog — list the project's
        // registered benchmark VMs (project member). Rust list_catalog, ORDER BY name.
        app.MapGet("/api/projects/{projectId}/benchmark-catalog", async (
            string projectId, NetworkerDbContext db, CancellationToken ct) =>
        {
            var vms = await db.BenchmarkVmCatalogs
                .AsNoTracking()
                .Where(v => v.ProjectId == projectId)
                .OrderBy(v => v.Name)
                .ToListAsync(ct);

            return Results.Ok(vms.Select(ShapeVm));
        })
        .RequireAuthorization(AuthPolicies.ProjectMember);

        // POST /api/projects/{projectId}/benchmark-catalog — register a VM
        // (project operator). Rust register_vm: name/ip required (400 when
        // blank), ssh_user defaults to "azureuser". Response: { vm_id }.
        app.MapPost("/api/projects/{projectId}/benchmark-catalog", async (
            string projectId,
            RegisterBenchmarkVmRequest body,
            HttpContext ctx,
            NetworkerDbContext db,
            ILoggerFactory loggerFactory,
            CancellationToken ct) =>
        {
            var user = ctx.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }

            // name/ip blank → 400 (explicit Rust check); cloud/region are
            // non-optional serde fields, so their absence is also a 400.
            if (string.IsNullOrWhiteSpace(body.Name)
                || string.IsNullOrWhiteSpace(body.Ip)
                || string.IsNullOrWhiteSpace(body.Cloud)
                || string.IsNullOrWhiteSpace(body.Region))
            {
                return ApiError.BadRequest("name, cloud, region and ip are required");
            }

            var vm = new BenchmarkVmCatalog
            {
                VmId = Guid.NewGuid(),
                ProjectId = projectId,
                Name = body.Name,
                Cloud = body.Cloud,
                Region = body.Region,
                Ip = body.Ip,
                SshUser = string.IsNullOrWhiteSpace(body.SshUser) ? "azureuser" : body.SshUser,
                VmSize = body.VmSize,
                Languages = "[]",
                Status = "unknown",
                CreatedBy = user.UserId,
                CreatedAt = DateTime.UtcNow,
            };
            db.BenchmarkVmCatalogs.Add(vm);
            await db.SaveChangesAsync(ct);

            loggerFactory.CreateLogger("Networker.ControlPlane.BenchmarkCatalog").LogInformation(
                "Benchmark VM registered: {VmId} name={Name} cloud={Cloud} ip={Ip} " +
                "(project {ProjectId}, by {UserId})",
                vm.VmId, vm.Name, vm.Cloud, vm.Ip, projectId, user.UserId);

            return Results.Ok(new { vm_id = vm.VmId });
        })
        .RequireAuthorization(AuthPolicies.ProjectOperator);

        // DELETE /api/projects/{projectId}/benchmark-catalog/{vmId} — remove a
        // VM (project ADMIN per the M5 spec; Rust required Operator). 404 when
        // the VM doesn't exist or belongs to another project. Response: { ok }.
        app.MapDelete("/api/projects/{projectId}/benchmark-catalog/{vmId:guid}", async (
            string projectId,
            Guid vmId,
            HttpContext ctx,
            NetworkerDbContext db,
            ILoggerFactory loggerFactory,
            CancellationToken ct) =>
        {
            var vm = await db.BenchmarkVmCatalogs
                .FirstOrDefaultAsync(v => v.VmId == vmId && v.ProjectId == projectId, ct);
            if (vm is null)
            {
                return Results.NotFound();
            }

            db.BenchmarkVmCatalogs.Remove(vm);
            await db.SaveChangesAsync(ct);

            loggerFactory.CreateLogger("Networker.ControlPlane.BenchmarkCatalog").LogInformation(
                "Benchmark VM removed: {VmId} (project {ProjectId}, by {UserId})",
                vmId, projectId, ctx.GetAuthUser()?.UserId);

            return Results.Ok(new { ok = true });
        })
        .RequireAuthorization(AuthPolicies.ProjectAdmin);

        // POST /api/projects/{projectId}/benchmark-catalog/{vmId}/detect —
        // language detection probe (project operator). Port of the Rust
        // detect_languages handler: SSH to the VM, probe /opt/bench/* installs,
        // persist languages + status, return 200 { languages, status }.
        app.MapPost("/api/projects/{projectId}/benchmark-catalog/{vmId:guid}/detect", async (
            string projectId,
            Guid vmId,
            NetworkerDbContext db,
            ISshLanguageDetector detector,
            ILoggerFactory loggerFactory,
            CancellationToken ct) =>
        {
            var vm = await db.BenchmarkVmCatalogs
                .FirstOrDefaultAsync(v => v.VmId == vmId && v.ProjectId == projectId, ct);
            if (vm is null)
            {
                return Results.NotFound();
            }

            var languages = await detector.DetectAsync(vm.Ip, vm.SshUser, ct);

            // Rust: update_languages sets the JSON list; update_status marks the
            // VM online when anything was detected (SSH succeeded), offline
            // otherwise, and stamps last_health_check = now().
            var status = languages.Count == 0 ? "offline" : "online";
            vm.Languages = JsonSerializer.Serialize(languages);
            vm.Status = status;
            vm.LastHealthCheck = DateTime.UtcNow;
            await db.SaveChangesAsync(ct);

            loggerFactory.CreateLogger("Networker.ControlPlane.BenchmarkCatalog").LogInformation(
                "Language detection complete for VM {VmId} ({Ip}): {Count} language(s) [{Languages}]",
                vmId, vm.Ip, languages.Count, string.Join(", ", languages));

            return Results.Ok(new { languages, status });
        })
        .RequireAuthorization(AuthPolicies.ProjectOperator);

        return app;
    }

    /// <summary>Snake_case row shape — Rust <c>VmCatalogRow</c> field-for-field.</summary>
    private static object ShapeVm(BenchmarkVmCatalog v) => new
    {
        vm_id = v.VmId,
        project_id = v.ProjectId,
        name = v.Name,
        cloud = v.Cloud,
        region = v.Region,
        ip = v.Ip,
        ssh_user = v.SshUser,
        languages = ParseJson(v.Languages),
        vm_size = v.VmSize,
        status = v.Status,
        last_health_check = v.LastHealthCheck,
        created_by = v.CreatedBy,
        created_at = v.CreatedAt,
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

/// <summary>
/// POST register body — Rust <c>RegisterVmRequest</c>: name/cloud/region/ip
/// required, ssh_user defaults to "azureuser", vm_size optional.
/// </summary>
public sealed record RegisterBenchmarkVmRequest(
    [property: JsonPropertyName("name")] string? Name,
    [property: JsonPropertyName("cloud")] string? Cloud,
    [property: JsonPropertyName("region")] string? Region,
    [property: JsonPropertyName("ip")] string? Ip,
    [property: JsonPropertyName("ssh_user")] string? SshUser,
    [property: JsonPropertyName("vm_size")] string? VmSize);
