using System.Diagnostics;
using System.Text.Json.Serialization;
using Microsoft.AspNetCore.Mvc;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Phase-2 M5: platform ADMIN endpoints — the C# port of the Rust
/// <c>crates/networker-dashboard/src/api/admin.rs</c> router (system metrics,
/// workspace usage, suspend / restore / protect / hard-delete, smoke test, and
/// the system-config KV).
///
/// <para>Everything here is platform-global and gated by
/// <see cref="AuthPolicies.GlobalAdmin"/> (the Rust side gates on
/// <c>is_platform_admin</c>; the C# global-admin policy is the module-level
/// equivalent per the M5 auth mapping).</para>
///
/// <para>Deviations from Rust, by design:</para>
/// <list type="bullet">
///   <item><b>metrics</b> — Rust shells sysinfo for host-wide CPU/memory; .NET
///   reports the process (Process/GC/DriveInfo) plus best-effort Postgres pool
///   stats. Field names match the Rust <c>SystemMetrics</c>/<c>DbMetrics</c>
///   serde shapes; a <c>counts</c> block is additive.</item>
///   <item><b>smoke-test</b> — Rust writes a marker to the logs-DB
///   <c>service_log</c>; there is no logs DB here, so the marker round-trips
///   through <c>system_config</c> instead (same insert → read → delete → ms).</item>
/// </list>
/// </summary>
public static class AdminEndpoints
{
    public static IEndpointRouteBuilder MapAdminEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/admin/metrics — process + DB health snapshot (Rust
        // system_metrics). DB stats are best-effort: any failure degrades to
        // zeros rather than 500ing the whole panel.
        app.MapGet("/api/admin/metrics", async (NetworkerDbContext db, CancellationToken ct) =>
        {
            var system = CollectSystemMetrics();
            var database = await CollectDbMetricsAsync(db, ct);

            var counts = new
            {
                users = await db.DashUsers.CountAsync(ct),
                projects = await db.Projects.CountAsync(p => p.DeletedAt == null, ct),
                agents_online = await db.Agents.CountAsync(a => a.Status == "online", ct),
                runs_24h = await db.TestRuns
                    .CountAsync(r => r.CreatedAt > DateTime.UtcNow.AddHours(-24), ct),
            };

            return Results.Ok(new { system, database, counts });
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        // GET /api/admin/workspaces — per-project usage rollup (Rust
        // workspace_usage). Bare array, snake_case, ordered by name.
        app.MapGet("/api/admin/workspaces", async (NetworkerDbContext db, CancellationToken ct) =>
        {
            var cutoff30d = DateTime.UtcNow.AddDays(-30);
            var rows = await db.Projects
                .AsNoTracking()
                .OrderBy(p => p.Name)
                .Select(p => new
                {
                    project_id = p.ProjectId,
                    name = p.Name,
                    slug = p.Slug,
                    member_count = db.ProjectMembers.Count(m => m.ProjectId == p.ProjectId),
                    tester_count = db.ProjectTesters.Count(t => t.ProjectId == p.ProjectId),
                    agent_count = db.Agents.Count(a => a.ProjectId == p.ProjectId),
                    runs_30d = db.TestRuns.Count(r =>
                        r.ProjectId == p.ProjectId && r.CreatedAt > cutoff30d),
                    last_activity = db.ProjectMembers
                        .Where(m => m.ProjectId == p.ProjectId)
                        .Join(db.DashUsers, m => m.UserId, u => u.UserId, (m, u) => u.LastLoginAt)
                        .Max(),
                    deleted_at = p.DeletedAt,
                    delete_protection = p.DeleteProtection,
                    status = p.DeletedAt == null ? "active" : "suspended",
                })
                .ToListAsync(ct);

            return Results.Ok(rows);
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        // POST /api/admin/workspaces/{projectId}/suspend — soft-delete (Rust
        // suspend_workspace → suspend_project: deleted_at = now() where null).
        app.MapPost("/api/admin/workspaces/{projectId}/suspend", async (
            string projectId,
            HttpContext ctx,
            NetworkerDbContext db,
            ILoggerFactory lf,
            CancellationToken ct) =>
        {
            await db.Projects
                .Where(p => p.ProjectId == projectId && p.DeletedAt == null)
                .ExecuteUpdateAsync(s => s.SetProperty(p => p.DeletedAt, DateTime.UtcNow), ct);

            lf.CreateLogger("AdminEndpoints").LogInformation(
                "Workspace {ProjectId} suspended by {Admin}",
                projectId, ctx.GetAuthUser()?.Email);
            return Results.Ok();
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        // POST /api/admin/workspaces/{projectId}/restore — clear deleted_at and
        // any lifecycle warnings (Rust restore_workspace → restore_project).
        app.MapPost("/api/admin/workspaces/{projectId}/restore", async (
            string projectId,
            HttpContext ctx,
            NetworkerDbContext db,
            ILoggerFactory lf,
            CancellationToken ct) =>
        {
            await db.Projects
                .Where(p => p.ProjectId == projectId)
                .ExecuteUpdateAsync(s => s.SetProperty(p => p.DeletedAt, (DateTime?)null), ct);
            await db.WorkspaceWarnings
                .Where(w => w.ProjectId == projectId)
                .ExecuteDeleteAsync(ct);

            lf.CreateLogger("AdminEndpoints").LogInformation(
                "Workspace {ProjectId} restored by {Admin}",
                projectId, ctx.GetAuthUser()?.Email);
            return Results.Ok();
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        // POST /api/admin/workspaces/{projectId}/protect — toggle
        // delete_protection, returning the new value (Rust protect_workspace).
        app.MapPost("/api/admin/workspaces/{projectId}/protect", async (
            string projectId,
            HttpContext ctx,
            NetworkerDbContext db,
            ILoggerFactory lf,
            CancellationToken ct) =>
        {
            var project = await db.Projects.FirstOrDefaultAsync(p => p.ProjectId == projectId, ct);
            if (project is null)
            {
                return Results.NotFound();
            }

            project.DeleteProtection = !project.DeleteProtection;
            await db.SaveChangesAsync(ct);

            lf.CreateLogger("AdminEndpoints").LogInformation(
                "Workspace {ProjectId} delete_protection toggled to {Value} by {Admin}",
                projectId, project.DeleteProtection, ctx.GetAuthUser()?.Email);
            return Results.Ok(new { delete_protection = project.DeleteProtection });
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        // DELETE /api/admin/workspaces/{projectId} — PERMANENT delete (Rust
        // hard_delete_workspace). Refused unless the project is already
        // soft-deleted AND unprotected; cascades dependent rows FK-safely.
        app.MapDelete("/api/admin/workspaces/{projectId}", async (
            string projectId,
            HttpContext ctx,
            NetworkerDbContext db,
            ILoggerFactory lf,
            CancellationToken ct) =>
        {
            var project = await db.Projects
                .AsNoTracking()
                .FirstOrDefaultAsync(p => p.ProjectId == projectId, ct);
            if (project is null)
            {
                return Results.NotFound();
            }
            if (project.DeletedAt is null)
            {
                return Results.BadRequest(new { error = "Workspace must be suspended before permanent deletion" });
            }
            if (project.DeleteProtection)
            {
                return Results.BadRequest(new { error = "Workspace has delete protection enabled" });
            }

            await WorkspaceCascade.HardDeleteAsync(db, projectId, ct);

            lf.CreateLogger("AdminEndpoints").LogWarning(
                "Workspace {ProjectId} ({Name}) PERMANENTLY deleted by {Admin}",
                projectId, project.Name, ctx.GetAuthUser()?.Email);
            return Results.Ok();
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        // POST /api/admin/smoke-test — write/read/delete a marker row and report
        // roundtrip latency (Rust smoke_test, retargeted at system_config since
        // the C# control plane has no logs-DB service_log table).
        app.MapPost("/api/admin/smoke-test", async (
            HttpContext ctx,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var marker = $"__smoke_test_{Guid.NewGuid()}__";
            var sw = Stopwatch.StartNew();
            try
            {
                db.SystemConfigs.Add(new Data.Entities.SystemConfig
                {
                    Key = marker,
                    Value = "ok",
                    UpdatedBy = ctx.GetAuthUser()?.UserId,
                    UpdatedAt = DateTime.UtcNow,
                });
                await db.SaveChangesAsync(ct);

                // Read back with an untracked query so this actually hits the DB.
                var found = await db.SystemConfigs
                    .AsNoTracking()
                    .AnyAsync(c => c.Key == marker, ct);

                // Always attempt cleanup.
                await db.SystemConfigs.Where(c => c.Key == marker).ExecuteDeleteAsync(ct);

                sw.Stop();
                return found
                    ? Results.Ok(new { ok = true, roundtrip_ms = (ulong)sw.ElapsedMilliseconds })
                    : Results.Ok(new { ok = false, error = "marker not found after insert" });
            }
            catch (Exception ex) when (ex is not OperationCanceledException)
            {
                return Results.Ok(new { ok = false, error = $"smoke test failed: {ex.Message}" });
            }
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        // GET /api/admin/system-config/{key} — read one KV (Rust
        // get_system_config). 404 when absent.
        app.MapGet("/api/admin/system-config/{key}", async (
            string key,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var value = await db.SystemConfigs
                .AsNoTracking()
                .Where(c => c.Key == key)
                .Select(c => c.Value)
                .FirstOrDefaultAsync(ct);

            return value is null
                ? Results.NotFound()
                : Results.Ok(new { key, value });
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        // PUT /api/admin/system-config/{key} — upsert one KV, stamping
        // updated_by/updated_at (Rust set_system_config).
        app.MapPut("/api/admin/system-config/{key}", async (
            string key,
            [FromBody] SystemConfigBody body,
            HttpContext ctx,
            NetworkerDbContext db,
            ILoggerFactory lf,
            CancellationToken ct) =>
        {
            if (body.Value is null)
            {
                return Results.BadRequest(new { error = "value is required" });
            }

            var admin = ctx.GetAuthUser();
            var existing = await db.SystemConfigs.FirstOrDefaultAsync(c => c.Key == key, ct);
            if (existing is null)
            {
                db.SystemConfigs.Add(new Data.Entities.SystemConfig
                {
                    Key = key,
                    Value = body.Value,
                    UpdatedBy = admin?.UserId,
                    UpdatedAt = DateTime.UtcNow,
                });
            }
            else
            {
                existing.Value = body.Value;
                existing.UpdatedBy = admin?.UserId;
                existing.UpdatedAt = DateTime.UtcNow;
            }

            await db.SaveChangesAsync(ct);
            lf.CreateLogger("AdminEndpoints").LogInformation(
                "system_config[{Key}] updated by {Admin}", key, admin?.Email);
            return Results.Ok();
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        return app;
    }

    /// <summary>PUT /api/admin/system-config/{key} body — Rust <c>SystemConfigBody</c>.</summary>
    public sealed record SystemConfigBody([property: JsonPropertyName("value")] string Value);

    // ── Metrics collection ────────────────────────────────────────────────────

    /// <summary>
    /// Process-level system snapshot. Field names match the Rust
    /// <c>SystemMetrics</c> serde shape; values are process/runtime-scoped
    /// (WorkingSet, container-aware available memory, base-dir disk) rather than
    /// sysinfo host-wide — good enough for the admin health panel.
    /// </summary>
    private static object CollectSystemMetrics()
    {
        var proc = Process.GetCurrentProcess();

        // Process CPU%: total processor time over process wall-clock, normalized
        // by core count (an average since start — no sampling delay per request).
        double cpuPercent = 0;
        var wall = (DateTime.UtcNow - proc.StartTime.ToUniversalTime()).TotalMilliseconds;
        if (wall > 0)
        {
            cpuPercent = proc.TotalProcessorTime.TotalMilliseconds
                         / (wall * Environment.ProcessorCount) * 100.0;
        }

        var gc = GC.GetGCMemoryInfo();

        long diskUsed = 0, diskTotal = 0;
        try
        {
            var root = Path.GetPathRoot(AppContext.BaseDirectory);
            if (!string.IsNullOrEmpty(root))
            {
                var drive = new DriveInfo(root);
                diskTotal = drive.TotalSize;
                diskUsed = drive.TotalSize - drive.AvailableFreeSpace;
            }
        }
        catch (Exception)
        {
            // best-effort; leave zeros
        }

        return new
        {
            cpu_usage_percent = (float)Math.Round(cpuPercent, 2),
            memory_used_bytes = (ulong)proc.WorkingSet64,
            memory_total_bytes = (ulong)Math.Max(gc.TotalAvailableMemoryBytes, 0),
            disk_used_bytes = (ulong)Math.Max(diskUsed, 0),
            disk_total_bytes = (ulong)Math.Max(diskTotal, 0),
            uptime_seconds = (ulong)(Environment.TickCount64 / 1000),
        };
    }

    /// <summary>
    /// Best-effort Postgres stats via the EF connection — same queries as the
    /// Rust <c>collect_db_metrics</c>. Any individual failure degrades that
    /// field instead of failing the endpoint.
    /// </summary>
    private static async Task<object> CollectDbMetricsAsync(NetworkerDbContext db, CancellationToken ct)
    {
        long active = 0, maxConn = 0, dbSize = 0;
        double? oldestTxn = null;
        double cacheHit = 0;

        var conn = db.Database.GetDbConnection();
        try
        {
            await db.Database.OpenConnectionAsync(ct);

            active = await ScalarAsync<long>(conn,
                "SELECT count(*) FROM pg_stat_activity WHERE state = 'active'", ct) ?? 0L;

            var maxStr = await ScalarStringAsync(conn, "SHOW max_connections", ct);
            maxConn = long.TryParse(maxStr, out var m) ? m : 100;

            dbSize = await ScalarAsync<long>(conn,
                "SELECT pg_database_size(current_database())", ct) ?? 0L;

            oldestTxn = await ScalarAsync<double>(conn,
                "SELECT EXTRACT(EPOCH FROM (now() - xact_start))::float8 FROM pg_stat_activity " +
                "WHERE state != 'idle' AND xact_start IS NOT NULL ORDER BY xact_start ASC LIMIT 1", ct);

            cacheHit = await ScalarAsync<double>(conn,
                "SELECT COALESCE(sum(blks_hit)::float8 / NULLIF(sum(blks_hit) + sum(blks_read), 0), 0)::float8 " +
                "FROM pg_stat_database WHERE datname = current_database()", ct) ?? 0d;
        }
        catch (Exception)
        {
            // pool stats are advisory — leave partial/zero values.
        }

        return new
        {
            active_connections = active,
            max_connections = maxConn,
            database_size_bytes = dbSize,
            oldest_transaction_age_seconds = oldestTxn,
            cache_hit_ratio = cacheHit,
        };
    }

    private static async Task<T?> ScalarAsync<T>(
        System.Data.Common.DbConnection conn, string sql, CancellationToken ct)
        where T : struct
    {
        var result = await ExecuteScalarAsync(conn, sql, ct);
        return result is null ? null : (T)Convert.ChangeType(result, typeof(T));
    }

    private static async Task<string?> ScalarStringAsync(
        System.Data.Common.DbConnection conn, string sql, CancellationToken ct)
        => (await ExecuteScalarAsync(conn, sql, ct))?.ToString();

    private static async Task<object?> ExecuteScalarAsync(
        System.Data.Common.DbConnection conn, string sql, CancellationToken ct)
    {
        await using var cmd = conn.CreateCommand();
        cmd.CommandText = sql;
        var result = await cmd.ExecuteScalarAsync(ct);
        return result is DBNull ? null : result;
    }
}

/// <summary>
/// The permanent-workspace-delete cascade, shared by the admin hard-delete
/// endpoint and the inactivity lifecycle loop (the C# port of the Rust
/// <c>db::projects::hard_delete_project</c>, extended to every table in the EF
/// model with a project FK). Deletes run via <c>ExecuteDelete</c> in FK-safe
/// order inside a single transaction — children before parents, so the DB-level
/// RESTRICT/NO ACTION constraints (agent, test_run, test_config, project_tester,
/// cloud_account, deployment) never fire.
/// </summary>
internal static class WorkspaceCascade
{
    public static async Task HardDeleteAsync(
        NetworkerDbContext db, string projectId, CancellationToken ct)
    {
        await using var txn = await db.Database.BeginTransactionAsync(ct);

        // Leaf tables with a direct project FK.
        await db.WorkspaceWarnings.Where(x => x.ProjectId == projectId).ExecuteDeleteAsync(ct);
        await db.WorkspaceInvites.Where(x => x.ProjectId == projectId).ExecuteDeleteAsync(ct);
        await db.TestVisibilityRules.Where(x => x.ProjectId == projectId).ExecuteDeleteAsync(ct);
        await db.ShareLinks.Where(x => x.ProjectId == projectId).ExecuteDeleteAsync(ct);
        await db.VmLifecycles.Where(x => x.ProjectId == projectId).ExecuteDeleteAsync(ct);
        await db.BenchmarkVmCatalogs.Where(x => x.ProjectId == projectId).ExecuteDeleteAsync(ct);

        // Children of agent / test_run, then approvals referencing both.
        await db.AgentCommands
            .Where(c => db.Agents.Any(a => a.AgentId == c.AgentId && a.ProjectId == projectId))
            .ExecuteDeleteAsync(ct);
        await db.CommandApprovals.Where(x => x.ProjectId == projectId).ExecuteDeleteAsync(ct);
        await db.BenchmarkArtifacts
            .Where(b => db.TestRuns.Any(r => r.Id == b.TestRunId && r.ProjectId == projectId))
            .ExecuteDeleteAsync(ct);

        // Schedules reference test_config + test_run; delete before both.
        await db.TestSchedules.Where(x => x.ProjectId == projectId).ExecuteDeleteAsync(ct);

        // Runs before configs/testers (test_run FKs both); groups after runs
        // (test_run.comparison_group_id would otherwise need SET NULL churn).
        await db.TestRuns.Where(x => x.ProjectId == projectId).ExecuteDeleteAsync(ct);
        await db.ComparisonGroups.Where(x => x.ProjectId == projectId).ExecuteDeleteAsync(ct);
        await db.TestConfigs.Where(x => x.ProjectId == projectId).ExecuteDeleteAsync(ct);

        // Agents reference project_tester (tester_id); testers reference
        // cloud_account — delete in that order.
        await db.Agents.Where(x => x.ProjectId == projectId).ExecuteDeleteAsync(ct);
        await db.ProjectTesters.Where(x => x.ProjectId == projectId).ExecuteDeleteAsync(ct);
        await db.CloudAccounts.Where(x => x.ProjectId == projectId).ExecuteDeleteAsync(ct);
        await db.Deployments.Where(x => x.ProjectId == projectId).ExecuteDeleteAsync(ct);

        await db.ProjectMembers.Where(x => x.ProjectId == projectId).ExecuteDeleteAsync(ct);
        await db.Projects.Where(x => x.ProjectId == projectId).ExecuteDeleteAsync(ct);

        await txn.CommitAsync(ct);
    }
}
