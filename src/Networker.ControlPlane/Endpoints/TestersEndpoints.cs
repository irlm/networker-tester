using Microsoft.EntityFrameworkCore;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Read-only tester (VM) + VM-history endpoints — the C# port of the Rust
/// dashboard's <c>api/testers.rs</c> (list_testers, get_tester, get_queue,
/// get_cost_estimate, list_regions) and <c>api/vm_history.rs</c>.
///
/// Phase-2 M1 covers the GET surface only; the mutating lifecycle endpoints
/// (create/start/stop/upgrade/delete/schedule) land in a later milestone.
///
/// Every route is project-scoped under <c>/api/projects/{projectId}</c> and
/// gated by the <c>ProjectMember</c> policy (matches the Rust Viewer gate —
/// any project role can read). JSON is emitted in snake_case to match the
/// Rust <c>serde</c> field names the frontend already consumes.
/// </summary>
public static class TestersEndpoints
{
    /// <summary>
    /// Fallback region list when no cloud connection/account is registered for
    /// the project. Mirrors the Rust <c>FALLBACK_AZURE_REGIONS</c> constant.
    /// </summary>
    private static readonly string[] FallbackAzureRegions =
    {
        "eastus",
        "westus2",
        "japaneast",
        "uksouth",
        "westeurope",
        "southeastasia",
        "australiaeast",
    };

    public static IEndpointRouteBuilder MapTestersEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/projects/{projectId}/testers — list testers.
        app.MapGet("/api/projects/{projectId}/testers", ListTesters)
            .RequireAuthorization("ProjectMember");

        // GET /api/projects/{projectId}/testers/regions — available regions.
        // NOTE: registered before the `{testerId}` route so the literal
        // segment wins the match (ASP.NET routing prefers literals, but the
        // ordering keeps intent obvious).
        app.MapGet("/api/projects/{projectId}/testers/regions", ListRegions)
            .RequireAuthorization("ProjectMember");

        // GET /api/projects/{projectId}/testers/{testerId} — full detail.
        app.MapGet("/api/projects/{projectId}/testers/{testerId:guid}", GetTester)
            .RequireAuthorization("ProjectMember");

        // GET /api/projects/{projectId}/testers/{testerId}/queue — running + queued.
        app.MapGet("/api/projects/{projectId}/testers/{testerId:guid}/queue", GetQueue)
            .RequireAuthorization("ProjectMember");

        // GET /api/projects/{projectId}/testers/{testerId}/cost_estimate — monthly $.
        app.MapGet("/api/projects/{projectId}/testers/{testerId:guid}/cost_estimate", GetCostEstimate)
            .RequireAuthorization("ProjectMember");

        // GET /api/projects/{projectId}/vm-history — VM lifecycle events.
        app.MapGet("/api/projects/{projectId}/vm-history", ListVmHistory)
            .RequireAuthorization("ProjectMember");

        return app;
    }

    // ── list_testers ─────────────────────────────────────────────────────

    private static async Task<IResult> ListTesters(string projectId, NetworkerDbContext db)
    {
        // Materialize the testers first, THEN enrich with the linked agent's
        // key-lifecycle fields — AgentKeyInfoFor runs its own query, which EF
        // cannot translate inside a Select projection (throws on PostgreSQL).
        var testers = await db.ProjectTesters
            .AsNoTracking()
            .Where(t => t.ProjectId == projectId)
            .OrderByDescending(t => t.CreatedAt)
            .ToListAsync();

        var rows = testers
            .Select(t => ToListDto(t, AgentKeyInfoFor(db, projectId, t.TesterId)))
            .ToList();

        return Results.Ok(rows);
    }

    // ── get_tester ───────────────────────────────────────────────────────

    private static async Task<IResult> GetTester(string projectId, Guid testerId, NetworkerDbContext db)
    {
        var tester = await db.ProjectTesters
            .AsNoTracking()
            .Where(t => t.ProjectId == projectId && t.TesterId == testerId)
            .FirstOrDefaultAsync();

        // 404 on cross-project / missing — mirrors the Rust scoping so no
        // cross-project leakage even for platform admins. Agent key-lifecycle
        // fields are looked up after materialization (see ListTesters).
        return tester is null
            ? ApiError.NotFound("Tester not found")
            : Results.Ok(ToDetailDto(tester, AgentKeyInfoFor(db, projectId, tester.TesterId)));
    }

    // ── get_queue ────────────────────────────────────────────────────────

    private static async Task<IResult> GetQueue(string projectId, Guid testerId, NetworkerDbContext db)
    {
        // Confirm the tester belongs to this project (404 otherwise); also grab
        // the rolling average duration used to compute queued ETAs.
        var tester = await db.ProjectTesters
            .AsNoTracking()
            .Where(t => t.ProjectId == projectId && t.TesterId == testerId)
            .Select(t => new { t.AvgBenchmarkDurationSeconds })
            .FirstOrDefaultAsync();

        if (tester is null)
        {
            return ApiError.NotFound("Tester not found");
        }

        // Running + queued test runs for this tester, join to config for name.
        // Order: running first, then queued oldest-first.
        var rows = await db.TestRuns
            .AsNoTracking()
            .Where(r => r.TesterId == testerId
                        && (r.Status == "running" || r.Status == "queued"))
            .Join(db.TestConfigs, r => r.TestConfigId, c => c.Id, (r, c) => new
            {
                config_id = r.TestConfigId,
                name = c.Name,
                status = r.Status,
                queued_at = r.CreatedAt,
                started_at = r.StartedAt,
            })
            .OrderBy(x => x.status == "running" ? 0 : 1)
            .ThenBy(x => x.queued_at)
            .ToListAsync();

        RunningEntry? running = null;
        var queued = new List<QueueEntry>();

        foreach (var row in rows)
        {
            if (row.status == "running" && running is null)
            {
                running = new RunningEntry(row.config_id, row.name, row.started_at);
            }
            else if (row.status == "queued")
            {
                queued.Add(new QueueEntry(row.config_id, row.name, row.queued_at, 0, null));
            }
        }

        // Assign positions + ETAs using the tester's rolling average duration.
        var avgSecs = tester.AvgBenchmarkDurationSeconds;
        var now = DateTime.UtcNow;
        for (var i = 0; i < queued.Count; i++)
        {
            var position = i + 1;
            DateTime? eta = avgSecs.HasValue
                ? now.AddSeconds((long)(position - 1) * avgSecs.Value)
                : null;
            queued[i] = queued[i] with { position = position, eta = eta };
        }

        return Results.Ok(new QueueResponse(testerId, running, queued));
    }

    // ── get_cost_estimate ────────────────────────────────────────────────

    private static async Task<IResult> GetCostEstimate(string projectId, Guid testerId, NetworkerDbContext db)
    {
        var tester = await db.ProjectTesters
            .AsNoTracking()
            .Where(t => t.ProjectId == projectId && t.TesterId == testerId)
            .Select(t => new { t.Cloud, t.Region, t.VmSize, t.AutoShutdownEnabled })
            .FirstOrDefaultAsync();

        if (tester is null)
        {
            return ApiError.NotFound("Tester not found");
        }

        var hourly = await HourlyUsdAsync(db, tester.Cloud, tester.VmSize, tester.Region);
        var (alwaysOn, withSchedule) = CostEstimate(hourly, tester.AutoShutdownEnabled);

        return Results.Ok(new CostEstimateResponse(
            tester.VmSize,
            hourly,
            alwaysOn,
            withSchedule,
            tester.AutoShutdownEnabled));
    }

    // ── list_regions ─────────────────────────────────────────────────────

    private static async Task<IResult> ListRegions(string projectId, NetworkerDbContext db)
    {
        // If the project has active cloud_connections, derive the region list
        // from the connected provider(s). Otherwise fall back to the legacy
        // cloud_account + hardcoded Azure list.
        var providers = await db.CloudConnections
            .AsNoTracking()
            .Where(c => c.ProjectId == projectId && c.Status == "active")
            .OrderBy(c => c.CreatedAt)
            .Select(c => c.Provider)
            .ToListAsync();

        if (providers.Count > 0)
        {
            var seen = new HashSet<string>();
            var regions = new List<string>();
            foreach (var provider in providers)
            {
                if (seen.Add(provider))
                {
                    regions.AddRange(RegionsForCloud(provider));
                }
            }

            if (regions.Count > 0)
            {
                return Results.Ok(new RegionsResponse(regions));
            }
            // else: fall through to the hardcoded list below (graceful degrade).
        }

        // Legacy path: cloud_account default region + hardcoded Azure regions.
        var regionDefault = await db.CloudAccounts
            .AsNoTracking()
            .Where(a => a.ProjectId == projectId
                        && a.Provider == "azure"
                        && a.RegionDefault != null)
            .OrderBy(a => a.CreatedAt)
            .Select(a => a.RegionDefault)
            .FirstOrDefaultAsync();

        var fallback = new List<string>(FallbackAzureRegions);
        if (!string.IsNullOrEmpty(regionDefault) && !fallback.Contains(regionDefault))
        {
            fallback.Insert(0, regionDefault);
        }

        return Results.Ok(new RegionsResponse(fallback));
    }

    // ── list_vm_history ──────────────────────────────────────────────────

    private const long MaxLimit = 500;
    private const long DefaultLimit = 100;

    private static async Task<IResult> ListVmHistory(
        string projectId,
        string? resource_type,
        Guid? resource_id,
        DateTime? from,
        DateTime? to,
        long? limit,
        long? offset,
        NetworkerDbContext db)
    {
        var effLimit = Math.Clamp(limit ?? DefaultLimit, 1, MaxLimit);
        var effOffset = Math.Max(offset ?? 0, 0);

        // Resource-scoped drill-down: oldest-first, always complete.
        if (resource_id is { } rid)
        {
            if (string.IsNullOrEmpty(resource_type) || !IsValidResourceType(resource_type))
            {
                return ApiError.BadRequest("resource_id requires a valid resource_type");
            }

            var scoped = await db.VmLifecycles
                .AsNoTracking()
                .Where(e => e.ProjectId == projectId
                            && e.ResourceType == resource_type
                            && e.ResourceId == rid)
                .OrderBy(e => e.EventTime)
                .Select(e => ToVmHistoryDto(e))
                .ToListAsync();

            return Results.Ok(new VmHistoryResponse(scoped, false));
        }

        // Project-wide list: newest-first, paginated. `resource_type` / `from`
        // / `to` filters applied in SQL (the Rust version filters in-process
        // after a page fetch; here we push the predicates down so `has_more`
        // still reflects the returned page size against the requested limit).
        var query = db.VmLifecycles
            .AsNoTracking()
            .Where(e => e.ProjectId == projectId);

        if (!string.IsNullOrEmpty(resource_type))
        {
            query = query.Where(e => e.ResourceType == resource_type);
        }
        if (from is { } f)
        {
            query = query.Where(e => e.EventTime >= f);
        }
        if (to is { } t)
        {
            query = query.Where(e => e.EventTime <= t);
        }

        var events = await query
            .OrderByDescending(e => e.EventTime)
            .Skip((int)effOffset)
            .Take((int)effLimit)
            .Select(e => ToVmHistoryDto(e))
            .ToListAsync();

        var hasMore = events.Count == effLimit;

        return Results.Ok(new VmHistoryResponse(events, hasMore));
    }

    private static bool IsValidResourceType(string s) =>
        s is "tester" or "endpoint" or "benchmark";

    // ── Cost helpers ─────────────────────────────────────────────────────

    /// <summary>
    /// Resolve the hourly USD rate for a VM size. Prefers a matching
    /// <c>cost_rate</c> row (cloud + vm_size, effective now, region-specific
    /// beating region-agnostic); falls back to the Rust hardcoded Azure rates
    /// when no row applies.
    /// </summary>
    private static async Task<double> HourlyUsdAsync(
        NetworkerDbContext db, string cloud, string vmSize, string? region)
    {
        var now = DateTime.UtcNow;
        var rate = await db.CostRates
            .AsNoTracking()
            .Where(r => r.Cloud == cloud
                        && r.VmSize == vmSize
                        && r.EffectiveFrom <= now
                        && (r.EffectiveTo == null || r.EffectiveTo > now)
                        && (r.Region == null || r.Region == region))
            // Region-specific match wins over a region-agnostic one; newest
            // effective_from breaks further ties.
            .OrderByDescending(r => r.Region != null)
            .ThenByDescending(r => r.EffectiveFrom)
            .Select(r => (decimal?)r.RatePerHourUsd)
            .FirstOrDefaultAsync();

        return rate.HasValue ? (double)rate.Value : HardcodedHourlyUsd(vmSize);
    }

    /// <summary>
    /// Hardcoded hourly USD lookup — mirrors the Rust <c>hourly_usd</c>.
    /// Unknown sizes fall back to the Standard_D2s_v3 rate.
    /// </summary>
    private static double HardcodedHourlyUsd(string vmSize) => vmSize switch
    {
        "Standard_D2s_v3" => 0.096,
        "Standard_D4s_v3" => 0.192,
        "Standard_D8s_v3" => 0.384,
        _ => 0.096,
    };

    /// <summary>
    /// (always_on, with_schedule) monthly USD. Mirrors the Rust
    /// <c>cost_estimate</c>: 24h×30d always-on, 15h×30d when auto-shutdown is
    /// enabled (business-day approximation), else equal to always-on.
    /// </summary>
    private static (double AlwaysOn, double WithSchedule) CostEstimate(double hourly, bool autoShutdownEnabled)
    {
        var alwaysOn = 24.0 * 30.0 * hourly;
        var withSchedule = autoShutdownEnabled ? 15.0 * 30.0 * hourly : alwaysOn;
        return (alwaysOn, withSchedule);
    }

    /// <summary>
    /// Per-cloud region catalog — mirrors the Rust
    /// <c>azure_regions::regions_for_cloud</c>. Unknown providers yield empty.
    /// </summary>
    private static string[] RegionsForCloud(string provider) => provider switch
    {
        "azure" => new[]
        {
            "eastus", "eastus2", "westus2", "westus3", "centralus", "southcentralus",
            "northeurope", "westeurope", "uksouth", "francecentral", "germanywestcentral",
            "japaneast", "koreacentral", "southeastasia", "australiaeast", "brazilsouth",
            "canadacentral",
        },
        "aws" => new[]
        {
            "us-east-1", "us-east-2", "us-west-1", "us-west-2", "eu-west-1", "eu-west-2",
            "eu-central-1", "ap-northeast-1", "ap-southeast-1", "ap-southeast-2", "sa-east-1",
        },
        "gcp" => new[]
        {
            "us-central1", "us-east1", "us-east4", "us-west1", "us-west2", "europe-west1",
            "europe-west2", "europe-west3", "europe-west4", "asia-east1", "asia-northeast1",
            "asia-southeast1", "australia-southeast1",
        },
        _ => Array.Empty<string>(),
    };

    // ── DTO projections ──────────────────────────────────────────────────

    /// <summary>
    /// List row — the fields the testers table needs (id/name/cloud/region/
    /// vm_size/power_state/allocation/status + auto-shutdown info). Fuller than
    /// the minimal Program.cs version but lighter than full detail.
    /// </summary>
    /// <summary>
    /// The linked agent's api-key lifecycle fields (V044) — surfaced on the
    /// tester read DTOs so the UI can show "last seen" + expiry status. Never
    /// includes the key or its hash. Null when no agent is linked yet.
    /// </summary>
    private sealed record AgentKeyInfo(
        DateTime? ApiKeyLastUsedAt,
        string? ApiKeyLastUsedIp,
        DateTime? ApiKeyExpiresAt);

    /// <summary>
    /// EF-translatable correlated sub-query: the api-key lifecycle fields of the
    /// agent bound to this tester, scoped to the same project (never crosses
    /// projects). A left-join shape — null when the tester has no agent.
    /// </summary>
    private static AgentKeyInfo? AgentKeyInfoFor(NetworkerDbContext db, string projectId, Guid testerId) =>
        db.Agents
            .Where(a => a.TesterId == testerId && a.ProjectId == projectId)
            .Select(a => new AgentKeyInfo(a.ApiKeyLastUsedAt, a.ApiKeyLastUsedIp, a.ApiKeyExpiresAt))
            .FirstOrDefault();

    private static object ToListDto(ProjectTester t, AgentKeyInfo? agent) => new
    {
        tester_id = t.TesterId,
        name = t.Name,
        cloud = t.Cloud,
        region = t.Region,
        vm_size = t.VmSize,
        power_state = t.PowerState,
        allocation = t.Allocation,
        status_message = t.StatusMessage,
        auto_shutdown_enabled = t.AutoShutdownEnabled,
        auto_shutdown_local_hour = t.AutoShutdownLocalHour,
        next_shutdown_at = t.NextShutdownAt,
        shutdown_deferral_count = t.ShutdownDeferralCount,
        last_used_at = t.LastUsedAt,
        api_key_last_used_at = agent != null ? agent.ApiKeyLastUsedAt : null,
        api_key_last_used_ip = agent != null ? agent.ApiKeyLastUsedIp : null,
        api_key_expires_at = agent != null ? agent.ApiKeyExpiresAt : null,
        created_at = t.CreatedAt,
        updated_at = t.UpdatedAt,
    };

    /// <summary>
    /// Full detail row — the complete <c>ProjectTesterRow</c> shape incl. VM
    /// lifecycle / OS / cloud-binding fields the Rust <c>get_tester</c> returns.
    /// </summary>
    private static object ToDetailDto(ProjectTester t, AgentKeyInfo? agent) => new
    {
        tester_id = t.TesterId,
        project_id = t.ProjectId,
        name = t.Name,
        cloud = t.Cloud,
        region = t.Region,
        vm_size = t.VmSize,
        vm_name = t.VmName,
        vm_resource_id = t.VmResourceId,
        public_ip = t.PublicIp != null ? t.PublicIp.ToString() : null,
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
        api_key_last_used_at = agent != null ? agent.ApiKeyLastUsedAt : null,
        api_key_last_used_ip = agent != null ? agent.ApiKeyLastUsedIp : null,
        api_key_expires_at = agent != null ? agent.ApiKeyExpiresAt : null,
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

    private static object ToVmHistoryDto(VmLifecycle e) => new
    {
        event_id = e.EventId,
        project_id = e.ProjectId,
        resource_type = e.ResourceType,
        resource_id = e.ResourceId,
        resource_name = e.ResourceName,
        cloud = e.Cloud,
        region = e.Region,
        vm_size = e.VmSize,
        vm_name = e.VmName,
        vm_resource_id = e.VmResourceId,
        cloud_connection_id = e.CloudConnectionId,
        cloud_account_name_at_event = e.CloudAccountNameAtEvent,
        provider_account_id = e.ProviderAccountId,
        event_type = e.EventType,
        event_time = e.EventTime,
        triggered_by = e.TriggeredBy,
        metadata = e.Metadata,
        created_at = e.CreatedAt,
    };

    // ── Response shapes (snake_case via property names) ───────────────────

    private sealed record RegionsResponse(List<string> regions);

    private sealed record RunningEntry(Guid config_id, string name, DateTime? started_at);

    private sealed record QueueEntry(
        Guid config_id, string name, DateTime? queued_at, int position, DateTime? eta);

    private sealed record QueueResponse(
        Guid tester_id, RunningEntry? running, List<QueueEntry> queued);

    private sealed record CostEstimateResponse(
        string vm_size,
        double hourly_usd,
        double monthly_always_on_usd,
        double monthly_with_schedule_usd,
        bool auto_shutdown_enabled);

    private sealed record VmHistoryResponse(List<object> events, bool has_more);
}
