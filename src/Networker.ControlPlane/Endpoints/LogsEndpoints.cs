using System.Text.Json;
using Networker.ControlPlane.Auth;
using Npgsql;
using NpgsqlTypes;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// C# port of the Rust dashboard's <c>api/logs.rs</c> — structured log query,
/// per-service stats, and pipeline metrics. Mounted in the Rust
/// <c>protected_flat</c> group (valid JWT required, no project scope), so these
/// use <c>.RequireAuthorization()</c> with no named policy.
///
/// <para>Routes:</para>
/// <list type="bullet">
///   <item><b>GET /api/logs</b> — filtered listing (service, level, config_id,
///     project_id, search, from, to, limit, offset). Defaults: to=now,
///     from=to-1h, limit=200 (clamp 1..1000), offset=0 (clamp 0..10000). When a
///     <c>search</c> term is present but no narrowing filter (service / level /
///     config_id / project_id) → 400. Response: <c>{ entries, total, truncated }</c>.</item>
///   <item><b>GET /api/logs/stats</b> — per-service level-bucket counts over a
///     window. Response: <c>{ by_service: {svc: {error,warn,info,debug,trace}},
///     total }</c>.</item>
///   <item><b>GET /api/logs/pipeline-status</b> — live log-pipeline metrics.</item>
/// </list>
///
/// <para>Project scoping: the Rust handler forces <c>project_id</c> from the
/// injected <c>ProjectContext</c> for non-admins. On these FLAT routes the Rust
/// <c>require_project</c> layer never runs, so <c>ProjectContext</c> is absent and
/// a non-admin's <c>project_id</c> is forced to <c>None</c> (null). That exact
/// behavior is reproduced here.</para>
///
/// <para>Raw-SQL / stub divergences:
/// (1) <c>service_log</c> is NOT in the EF model; it is read with raw Npgsql via
///   <see cref="NpgsqlDataSource"/> against the core DB (the C# port has a single
///   DB, no split logs pool). SQL mirrors <c>networker_log::query</c> verbatim.
/// (2) <b>/api/logs/pipeline-status</b> has no live pipeline source in the C#
///   ControlPlane (there is no in-process log-batching pipeline). The endpoint +
///   response shape are ported; the metrics are a zeroed/"healthy" snapshot —
///   see the <c>// TODO(phase3)</c> note.</para>
/// </summary>
public static class LogsEndpoints
{
    private const long DefaultLimit = 200;
    private const long MaxLimit = 1000;

    public static IEndpointRouteBuilder MapLogsEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/logs — filtered listing.
        app.MapGet("/api/logs", async (
            HttpContext ctx,
            string? service,
            string? level,
            Guid? config_id,
            string? project_id,
            string? search,
            DateTime? from,
            DateTime? to,
            long? limit,
            long? offset,
            NpgsqlDataSource dataSource,
            CancellationToken ct) =>
        {
            var user = ctx.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }

            // Non-admins: force project_id from ProjectContext — which is never
            // present on this flat route → null (see class remarks).
            if (!user.IsPlatformAdmin)
            {
                project_id = null;
            }

            var toTs = (to ?? DateTime.UtcNow).ToUniversalTime();
            var fromTs = (from?.ToUniversalTime()) ?? toTs.AddHours(-1);

            // H4: a search term requires at least one narrowing filter.
            if (search is not null
                && service is null
                && level is null
                && config_id is null
                && project_id is null)
            {
                return Results.BadRequest();
            }

            var take = Math.Clamp(limit ?? DefaultLimit, 1, MaxLimit);
            var skip = Math.Clamp(offset ?? 0, 0, 10_000);

            short? minLevel = ParseLevelToDb(level);

            var response = await QueryLogsAsync(
                dataSource, service, minLevel, config_id, project_id, search, fromTs, toTs, take, skip, ct);

            return Results.Ok(response);
        }).RequireAuthorization();

        // GET /api/logs/stats — per-service level-bucket counts.
        app.MapGet("/api/logs/stats", async (
            HttpContext ctx,
            string? project_id,
            DateTime? from,
            DateTime? to,
            NpgsqlDataSource dataSource,
            CancellationToken ct) =>
        {
            var user = ctx.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }

            if (!user.IsPlatformAdmin)
            {
                project_id = null;
            }

            var toTs = (to ?? DateTime.UtcNow).ToUniversalTime();
            var fromTs = (from?.ToUniversalTime()) ?? toTs.AddHours(-1);

            var stats = await StatsAsync(dataSource, fromTs, toTs, project_id, ct);
            return Results.Ok(stats);
        }).RequireAuthorization();

        // GET /api/logs/pipeline-status — live pipeline metrics (STUBBED shape).
        app.MapGet("/api/logs/pipeline-status", (HttpContext ctx) =>
        {
            var user = ctx.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }

            // TODO(phase3): the C# ControlPlane has no in-process log-batching
            // pipeline (the Rust dashboard batches service_log writes and exposes
            // live counters). Return a zeroed, "healthy" snapshot so the shape
            // matches; wire real metrics when/if a C# log pipeline lands.
            return Results.Ok(new
            {
                entries_written = 0UL,
                entries_dropped = 0UL,
                flush_count = 0UL,
                flush_errors = 0UL,
                last_flush_ms = 0UL,
                queue_depth = 0U,
                status = "healthy",
            });
        }).RequireAuthorization();

        return app;
    }

    // ── Level parsing (mirrors networker_log::Level::from_str + as_db) ───────
    // Returns null on unrecognized input (Rust: .parse().ok() → None).
    public static short? ParseLevelToDb(string? level)
    {
        if (level is null)
        {
            return null;
        }

        return level.Trim().ToUpperInvariant() switch
        {
            "ERROR" or "ERR" or "FATAL" or "1" => (short)1,
            "WARN" or "WARNING" or "2" => (short)2,
            "INFO" or "INF" or "3" or "INFORMATION" => (short)3,
            "DEBUG" or "DBG" or "4" => (short)4,
            "TRACE" or "TRC" or "5" => (short)5,
            _ => null,
        };
    }

    // ── list (mirrors networker_log::query::list) ───────────────────────────
    private static async Task<object> QueryLogsAsync(
        NpgsqlDataSource dataSource,
        string? service, short? minLevel, Guid? configId, string? projectId, string? search,
        DateTime fromTs, DateTime toTs, long limit, long offset, CancellationToken ct)
    {
        // $1 = from, $2 = to always present.
        var conditions = new List<string> { "ts >= $1", "ts <= $2" };
        var extraParams = new List<(NpgsqlDbType Type, object Value)>();
        var idx = 3;

        if (service is not null)
        {
            conditions.Add($"service = ${idx++}");
            extraParams.Add((NpgsqlDbType.Text, service));
        }
        if (minLevel is short lvl)
        {
            conditions.Add($"level <= ${idx++}");
            extraParams.Add((NpgsqlDbType.Smallint, lvl));
        }
        if (configId is Guid cid)
        {
            conditions.Add($"config_id = ${idx++}");
            extraParams.Add((NpgsqlDbType.Uuid, cid));
        }
        if (projectId is not null)
        {
            conditions.Add($"project_id = ${idx++}");
            extraParams.Add((NpgsqlDbType.Text, projectId));
        }
        if (search is not null)
        {
            var escaped = search.Replace("\\", "\\\\").Replace("%", "\\%").Replace("_", "\\_");
            conditions.Add($"message ILIKE ${idx++}");
            extraParams.Add((NpgsqlDbType.Text, $"%{escaped}%"));
        }

        var whereClause = string.Join(" AND ", conditions);

        await using var conn = await dataSource.OpenConnectionAsync(ct);

        // COUNT capped at 10001 rows.
        var countSql =
            $"SELECT COUNT(*) FROM (SELECT 1 FROM service_log WHERE {whereClause} LIMIT 10001) sub";
        long total;
        await using (var cmd = new NpgsqlCommand(countSql, conn))
        {
            cmd.Parameters.AddWithValue(fromTs);
            cmd.Parameters.AddWithValue(toTs);
            foreach (var (type, value) in extraParams)
            {
                cmd.Parameters.Add(new NpgsqlParameter { NpgsqlDbType = type, Value = value });
            }
            total = Convert.ToInt64(await cmd.ExecuteScalarAsync(ct));
        }

        var selectSql =
            "SELECT ts, service, level, message, config_id, project_id, trace_id, fields " +
            $"FROM service_log WHERE {whereClause} " +
            "ORDER BY ts DESC " +
            $"LIMIT ${idx} OFFSET ${idx + 1}";

        var entries = new List<object>();
        await using (var cmd = new NpgsqlCommand(selectSql, conn))
        {
            cmd.Parameters.AddWithValue(fromTs);
            cmd.Parameters.AddWithValue(toTs);
            foreach (var (type, value) in extraParams)
            {
                cmd.Parameters.Add(new NpgsqlParameter { NpgsqlDbType = type, Value = value });
            }
            cmd.Parameters.AddWithValue(limit);
            cmd.Parameters.AddWithValue(offset);

            await using var reader = await cmd.ExecuteReaderAsync(ct);
            while (await reader.ReadAsync(ct))
            {
                entries.Add(new
                {
                    ts = reader.GetDateTime(0),
                    service = reader.GetString(1),
                    level = reader.GetInt16(2),
                    message = reader.GetString(3),
                    config_id = reader.IsDBNull(4) ? (Guid?)null : reader.GetGuid(4),
                    project_id = reader.IsDBNull(5) ? null : reader.GetString(5),
                    trace_id = reader.IsDBNull(6) ? (Guid?)null : reader.GetGuid(6),
                    fields = reader.IsDBNull(7) ? null : RawJson(reader.GetString(7)),
                });
            }
        }

        return new
        {
            entries,
            total,
            truncated = total > 10_000,
        };
    }

    // ── stats (mirrors networker_log::query::stats) ─────────────────────────
    private static async Task<object> StatsAsync(
        NpgsqlDataSource dataSource, DateTime fromTs, DateTime toTs, string? projectId, CancellationToken ct)
    {
        string sql;
        if (projectId is not null)
        {
            sql = "SELECT service, level, COUNT(*) FROM service_log " +
                  "WHERE ts >= $1 AND ts <= $2 AND project_id = $3 GROUP BY service, level";
        }
        else
        {
            sql = "SELECT service, level, COUNT(*) FROM service_log " +
                  "WHERE ts >= $1 AND ts <= $2 GROUP BY service, level";
        }

        var byService = new Dictionary<string, ServiceStats>();
        long grandTotal = 0;

        await using var cmd = dataSource.CreateCommand(sql);
        cmd.Parameters.AddWithValue(fromTs);
        cmd.Parameters.AddWithValue(toTs);
        if (projectId is not null)
        {
            cmd.Parameters.AddWithValue(projectId);
        }

        await using var reader = await cmd.ExecuteReaderAsync(ct);
        while (await reader.ReadAsync(ct))
        {
            var service = reader.GetString(0);
            var level = reader.GetInt16(1);
            var count = reader.GetInt64(2);
            grandTotal += count;

            if (!byService.TryGetValue(service, out var s))
            {
                s = new ServiceStats();
                byService[service] = s;
            }
            switch (level)
            {
                case 1: s.error += count; break;
                case 2: s.warn += count; break;
                case 3: s.info += count; break;
                case 4: s.debug += count; break;
                case 5: s.trace += count; break;
                default: break; // unknown level — skip
            }
        }

        return new
        {
            by_service = byService,
            total = grandTotal,
        };
    }

    private static object RawJson(string value)
    {
        try
        {
            using var doc = JsonDocument.Parse(value);
            return doc.RootElement.Clone();
        }
        catch (JsonException)
        {
            return value;
        }
    }

    public sealed class ServiceStats
    {
        public long error { get; set; }
        public long warn { get; set; }
        public long info { get; set; }
        public long debug { get; set; }
        public long trace { get; set; }
    }
}
