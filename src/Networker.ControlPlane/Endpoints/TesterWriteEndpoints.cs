using System.Diagnostics;
using System.Text;
using System.Text.Json;
using Microsoft.AspNetCore.Mvc;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Provisioning;
using Networker.Data;
using Networker.Data.Entities;
using Networker.Security;
using Npgsql;
using NpgsqlTypes;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Persistent tester (VM) <b>lifecycle write</b> endpoints — the C# port of the
/// mutating handlers in the Rust dashboard's <c>api/testers.rs</c>
/// (create / start / stop / upgrade / probe / postpone / force-stop / schedule / delete).
/// The read surface (list / get / queue / cost / regions) lives in
/// <see cref="TestersEndpoints"/>; this file is additive and touches neither it
/// nor <c>Program.cs</c>.
///
/// <para><b>202-async pattern (preserved from Rust):</b> a mutating call first
/// validates the DB state transition, applies the authoritative
/// <c>power_state</c> / <c>allocation</c> change synchronously, returns
/// <c>202 Accepted</c> with the updated row, and drives the cloud CLI in the
/// background through <see cref="IComputeProvisioner"/>. The synchronous ops
/// (probe / postpone / schedule / force-stop) return <c>200 OK</c> like the Rust
/// side.</para>
///
/// <para><b>CI-safety:</b> cloud CLIs are absent in CI. The endpoints therefore
/// never fail the request when the CLI can't run — they do the DB transition and
/// return 202, and the provisioner call runs detached (failures are logged and
/// written to <c>status_message</c>, never surfaced to the caller). This keeps
/// every endpoint testable purely on (202 + DB change).</para>
///
/// <para><b>Auth</b> (matches the Rust <c>require_project_role</c> gates):
/// <c>ProjectOperator</c> for start / stop / probe / postpone / schedule;
/// <c>ProjectAdmin</c> for upgrade / force-stop / delete.</para>
/// </summary>
public static partial class TesterWriteEndpoints
{
    public static IEndpointRouteBuilder MapTesterWriteEndpoints(this IEndpointRouteBuilder app)
    {
        const string basePath = "/api/projects/{projectId}/testers/{testerId:guid}";

        // POST /testers — create + provision (Operator). The collection route;
        // the read-side GET lives in TestersEndpoints.
        app.MapPost("/api/projects/{projectId}/testers", CreateTester)
            .RequireAuthorization(AuthPolicies.ProjectOperator);

        app.MapPost($"{basePath}/start", StartTester)
            .RequireAuthorization(AuthPolicies.ProjectOperator);

        app.MapPost($"{basePath}/stop", StopTester)
            .RequireAuthorization(AuthPolicies.ProjectOperator);

        app.MapPost($"{basePath}/force-stop", ForceStopTester)
            .RequireAuthorization(AuthPolicies.ProjectAdmin);

        app.MapPost($"{basePath}/upgrade", UpgradeTester)
            .RequireAuthorization(AuthPolicies.ProjectAdmin);

        app.MapPost($"{basePath}/probe", ProbeTester)
            .RequireAuthorization(AuthPolicies.ProjectOperator);

        // Rotate the tester's agent api-key (Operator). Returns the new
        // plaintext key ONCE; the old key's hash is replaced so it dies instantly.
        app.MapPost($"{basePath}/rotate-key", RotateAgentKey)
            .RequireAuthorization(AuthPolicies.ProjectOperator);

        app.MapPost($"{basePath}/postpone", PostponeShutdown)
            .RequireAuthorization(AuthPolicies.ProjectOperator);

        app.MapPatch($"{basePath}/schedule", UpdateSchedule)
            .RequireAuthorization(AuthPolicies.ProjectOperator);

        app.MapDelete("/api/projects/{projectId}/testers/{testerId:guid}", DeleteTester)
            .RequireAuthorization(AuthPolicies.ProjectAdmin);

        return app;
    }

    // ── shared helpers ────────────────────────────────────────────────────────

    private static Task<ProjectTester?> LoadAsync(
        NetworkerDbContext db, string projectId, Guid testerId, CancellationToken ct) =>
        db.ProjectTesters.FirstOrDefaultAsync(t => t.ProjectId == projectId && t.TesterId == testerId, ct);

    private static async Task<int> InFlightRunCountAsync(NetworkerDbContext db, Guid testerId, CancellationToken ct) =>
        await db.TestRuns.CountAsync(
            r => r.TesterId == testerId
                 && (r.Status == "queued" || r.Status == "provisioning" || r.Status == "running"),
            ct);

    private static IResult Conflict(string message) =>
        ApiError.Status(StatusCodes.Status409Conflict, message);

    // ── DTO (snake_case, subset matching the Rust ProjectTesterRow response) ──

    private static object ToDto(ProjectTester t) => new
    {
        tester_id = t.TesterId,
        project_id = t.ProjectId,
        name = t.Name,
        cloud = t.Cloud,
        region = t.Region,
        vm_size = t.VmSize,
        vm_name = t.VmName,
        vm_resource_id = t.VmResourceId,
        public_ip = t.PublicIp?.ToString(),
        ssh_user = t.SshUser,
        power_state = t.PowerState,
        allocation = t.Allocation,
        status_message = t.StatusMessage,
        locked_by_config_id = t.LockedByConfigId,
        installer_version = t.InstallerVersion,
        last_installed_at = t.LastInstalledAt,
        auto_shutdown_enabled = t.AutoShutdownEnabled,
        auto_shutdown_local_hour = t.AutoShutdownLocalHour,
        next_shutdown_at = t.NextShutdownAt,
        shutdown_deferral_count = t.ShutdownDeferralCount,
        auto_probe_enabled = t.AutoProbeEnabled,
        last_used_at = t.LastUsedAt,
        created_at = t.CreatedAt,
        updated_at = t.UpdatedAt,
        cloud_connection_id = t.CloudConnectionId,
        cloud_account_id = t.CloudAccountId,
    };

    /// <summary>
    /// Full-row DTO for the create response — the complete
    /// <c>ProjectTesterRow</c> shape the Rust <c>create_tester</c> returns
    /// (same fields as <c>SELECT_COLUMNS</c> / the list_testers rows).
    /// </summary>
    private static object ToFullDto(ProjectTester t) => new
    {
        tester_id = t.TesterId,
        project_id = t.ProjectId,
        name = t.Name,
        cloud = t.Cloud,
        region = t.Region,
        vm_size = t.VmSize,
        vm_name = t.VmName,
        vm_resource_id = t.VmResourceId,
        public_ip = t.PublicIp?.ToString(),
        ssh_user = t.SshUser,
        power_state = t.PowerState,
        allocation = t.Allocation,
        status_message = t.StatusMessage,
        locked_by_config_id = t.LockedByConfigId,
        installer_version = t.InstallerVersion,
        last_installed_at = t.LastInstalledAt,
        auto_shutdown_enabled = t.AutoShutdownEnabled,
        auto_shutdown_local_hour = t.AutoShutdownLocalHour,
        next_shutdown_at = t.NextShutdownAt,
        shutdown_deferral_count = t.ShutdownDeferralCount,
        auto_probe_enabled = t.AutoProbeEnabled,
        last_used_at = t.LastUsedAt,
        avg_benchmark_duration_seconds = t.AvgBenchmarkDurationSeconds,
        benchmark_run_count = t.BenchmarkRunCount,
        created_by = t.CreatedBy,
        created_at = t.CreatedAt,
        updated_at = t.UpdatedAt,
        cloud_connection_id = t.CloudConnectionId,
        cloud_account_id = t.CloudAccountId,
        requested_os = t.RequestedOs,
        requested_variant = t.RequestedVariant,
        os_distro = t.OsDistro,
        os_version = t.OsVersion,
        os_variant = t.OsVariant,
        os_arch = t.OsArch,
        os_kernel = t.OsKernel,
    };

    // ── Request bodies (snake_case via [FromBody] + JSON property names) ──────

    /// <summary>
    /// Body for POST /testers — mirrors the Rust <c>CreateTesterBody</c>
    /// (dashboard/src/api/testers.ts <c>CreateTesterBody</c> on the wire).
    /// </summary>
    public sealed record CreateTesterBody
    {
        [System.Text.Json.Serialization.JsonPropertyName("name")]
        public string Name { get; init; } = string.Empty;

        [System.Text.Json.Serialization.JsonPropertyName("cloud")]
        public string Cloud { get; init; } = string.Empty;

        [System.Text.Json.Serialization.JsonPropertyName("region")]
        public string Region { get; init; } = string.Empty;

        [System.Text.Json.Serialization.JsonPropertyName("vm_size")]
        public string? VmSize { get; init; }

        [System.Text.Json.Serialization.JsonPropertyName("auto_shutdown_local_hour")]
        public short? AutoShutdownLocalHour { get; init; }

        [System.Text.Json.Serialization.JsonPropertyName("auto_probe_enabled")]
        public bool? AutoProbeEnabled { get; init; }

        [System.Text.Json.Serialization.JsonPropertyName("cloud_connection_id")]
        public Guid? CloudConnectionId { get; init; }

        [System.Text.Json.Serialization.JsonPropertyName("cloud_account_id")]
        public Guid? CloudAccountId { get; init; }

        [System.Text.Json.Serialization.JsonPropertyName("requested_os")]
        public string? RequestedOs { get; init; }

        [System.Text.Json.Serialization.JsonPropertyName("requested_variant")]
        public string? RequestedVariant { get; init; }
    }

    public sealed record UpgradeBody
    {
        [System.Text.Json.Serialization.JsonPropertyName("confirm")]
        public bool Confirm { get; init; }
    }

    public sealed record ForceStopBody
    {
        [System.Text.Json.Serialization.JsonPropertyName("confirm")]
        public bool Confirm { get; init; }

        [System.Text.Json.Serialization.JsonPropertyName("reason")]
        public string Reason { get; init; } = string.Empty;
    }

    public sealed record ScheduleBody
    {
        [System.Text.Json.Serialization.JsonPropertyName("auto_shutdown_enabled")]
        public bool? AutoShutdownEnabled { get; init; }

        [System.Text.Json.Serialization.JsonPropertyName("auto_shutdown_local_hour")]
        public short? AutoShutdownLocalHour { get; init; }
    }

    /// <summary>
    /// Postpone body — the three shapes from the Rust untagged enum
    /// (<c>{until}</c> | <c>{add_hours}</c> | <c>{skip_tonight}</c>). Deserialized
    /// as one flat record; exactly one field is expected to be present.
    /// </summary>
    public sealed record PostponeBody
    {
        [System.Text.Json.Serialization.JsonPropertyName("until")]
        public DateTime? Until { get; init; }

        [System.Text.Json.Serialization.JsonPropertyName("add_hours")]
        public long? AddHours { get; init; }

        [System.Text.Json.Serialization.JsonPropertyName("skip_tonight")]
        public bool? SkipTonight { get; init; }
    }
}
