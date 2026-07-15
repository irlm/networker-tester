using System.Text.Json;
using System.Text.Json.Serialization;
using Networker.ControlPlane.Auth;
using Npgsql;
using NpgsqlTypes;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// C# port of the Rust dashboard's <c>api/perf_log.rs</c> — ingest, list, and
/// aggregate stats for frontend performance telemetry. All three routes are
/// PLATFORM-ADMIN only (the Rust handlers reject non-<c>is_platform_admin</c>
/// callers with 403); mounted in <c>protected_flat</c>, so the C# routes use
/// <c>.RequireAuthorization()</c> plus an inline <c>IsPlatformAdmin</c> check.
///
/// <para>Routes:</para>
/// <list type="bullet">
///   <item><b>POST /api/perf-log</b> — ingest a batch. Body:
///     <c>{ session_id?, entries: [PerfLogInput...] }</c>. Rejects batches &gt;200,
///     or field lengths over the schema VARCHAR limits, with 400. Returns
///     <c>{ inserted: n }</c>.</item>
///   <item><b>GET /api/perf-log</b> — list with filters (kind, path, user_id,
///     limit default 100 clamp 1..500, offset ≥0). Returns <c>[PerfLogRow...]</c>.</item>
///   <item><b>GET /api/perf-log/stats</b> — 24h aggregate. Returns the stats object.</item>
/// </list>
///
/// <para>Raw-SQL divergence: <c>perf_log</c> is NOT in the EF model (it lives in
/// the logs DB in Rust; per the Rust <c>ensure_schema</c> note it is created in
/// whichever pool the perf-log endpoints hit). The C# ControlPlane has a single
/// core DB, so this reads/writes <c>perf_log</c> there via raw Npgsql
/// (<see cref="NpgsqlDataSource"/>), mirroring <c>db::perf_log</c> verbatim.</para>
/// </summary>
public static class PerfLogEndpoints
{
    private const long DefaultLimit = 100;
    private const long MaxLimit = 500;
    private const int MaxBatch = 200;

    // Field length limits matching the schema VARCHAR constraints.
    private const int MaxSessionId = 64;
    private const int MaxMethod = 10;
    private const int MaxPath = 500;
    private const int MaxSource = 20;
    private const int MaxComponent = 100;
    private const int MaxTrigger = 100;
    private const int MaxKind = 10;

    public static IEndpointRouteBuilder MapPerfLogEndpoints(this IEndpointRouteBuilder app)
    {
        // POST /api/perf-log — ingest a batch (admin only).
        app.MapPost("/api/perf-log", async (
            HttpContext ctx,
            IngestRequest? payload,
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
                return Results.StatusCode(StatusCodes.Status403Forbidden);
            }

            if (payload is null)
            {
                return Results.BadRequest();
            }

            var entries = payload.entries ?? new List<PerfLogInput>();
            if (entries.Count > MaxBatch)
            {
                return Results.BadRequest();
            }

            if (payload.session_id is { } sid && sid.Length > MaxSessionId)
            {
                return Results.BadRequest();
            }
            foreach (var e in entries)
            {
                if ((e.kind ?? string.Empty).Length > MaxKind
                    || (e.method is { } m && m.Length > MaxMethod)
                    || (e.path is { } p && p.Length > MaxPath)
                    || (e.source is { } s && s.Length > MaxSource)
                    || (e.component is { } c && c.Length > MaxComponent)
                    || (e.trigger is { } t && t.Length > MaxTrigger))
                {
                    return Results.BadRequest();
                }
            }

            var inserted = await InsertBatchAsync(dataSource, user.UserId, payload.session_id, entries, ct);
            return Results.Ok(new { inserted });
        }).RequireAuthorization();

        // GET /api/perf-log — list with filters (admin only).
        app.MapGet("/api/perf-log", async (
            HttpContext ctx,
            string? kind,
            string? path,
            Guid? user_id,
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
            if (!user.IsPlatformAdmin)
            {
                return Results.StatusCode(StatusCodes.Status403Forbidden);
            }

            var take = Math.Clamp(limit ?? DefaultLimit, 1, MaxLimit);
            var skip = Math.Max(offset ?? 0, 0);

            var rows = await ListAsync(dataSource, kind, path, user_id, take, skip, ct);
            return Results.Ok(rows);
        }).RequireAuthorization();

        // GET /api/perf-log/stats — 24h aggregate (admin only).
        app.MapGet("/api/perf-log/stats", async (
            HttpContext ctx,
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
                return Results.StatusCode(StatusCodes.Status403Forbidden);
            }

            var stats = await StatsAsync(dataSource, ct);
            return Results.Ok(stats);
        }).RequireAuthorization();

        return app;
    }

    // ── insert_batch (mirrors db::perf_log::insert_batch) ───────────────────
    private static async Task<long> InsertBatchAsync(
        NpgsqlDataSource dataSource, Guid userId, string? sessionId, List<PerfLogInput> entries,
        CancellationToken ct)
    {
        if (entries.Count == 0)
        {
            return 0;
        }

        await using var conn = await dataSource.OpenConnectionAsync(ct);
        await using var tx = await conn.BeginTransactionAsync(ct);

        const string sql =
            "INSERT INTO perf_log (logged_at, user_id, session_id, kind, method, path, status, " +
            "total_ms, server_ms, network_ms, source, component, \"trigger\", render_ms, item_count, meta) " +
            "VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)";

        long count = 0;
        foreach (var e in entries)
        {
            var loggedAt = e.timestamp is long ms
                ? DateTimeOffset.FromUnixTimeMilliseconds(ms).UtcDateTime
                : DateTime.UtcNow;

            await using var cmd = new NpgsqlCommand(sql, conn, (NpgsqlTransaction)tx);
            cmd.Parameters.AddWithValue(loggedAt);
            cmd.Parameters.AddWithValue(userId);
            cmd.Parameters.Add(NullableText(sessionId));
            cmd.Parameters.AddWithValue(e.kind ?? string.Empty);
            cmd.Parameters.Add(NullableText(e.method));
            cmd.Parameters.Add(NullableText(e.path));
            cmd.Parameters.Add(Nullable(NpgsqlDbType.Smallint, e.status));
            cmd.Parameters.Add(Nullable(NpgsqlDbType.Real, e.total_ms));
            cmd.Parameters.Add(Nullable(NpgsqlDbType.Real, e.server_ms));
            cmd.Parameters.Add(Nullable(NpgsqlDbType.Real, e.network_ms));
            cmd.Parameters.Add(NullableText(e.source));
            cmd.Parameters.Add(NullableText(e.component));
            cmd.Parameters.Add(NullableText(e.trigger));
            cmd.Parameters.Add(Nullable(NpgsqlDbType.Real, e.render_ms));
            cmd.Parameters.Add(Nullable(NpgsqlDbType.Integer, e.item_count));
            cmd.Parameters.Add(NullableJsonb(e.meta));
            await cmd.ExecuteNonQueryAsync(ct);
            count += 1;
        }

        await tx.CommitAsync(ct);
        return count;
    }

    // ── list (mirrors db::perf_log::list) ───────────────────────────────────
    private static async Task<List<PerfLogRow>> ListAsync(
        NpgsqlDataSource dataSource, string? kind, string? pathFilter, Guid? userIdFilter,
        long limit, long offset, CancellationToken ct)
    {
        const string baseSql =
            "SELECT id, logged_at, user_id, session_id, kind, method, path, status, " +
            "total_ms, server_ms, network_ms, source, component, \"trigger\", render_ms, item_count, meta " +
            "FROM perf_log WHERE 1=1";

        var clauses = new List<string>();
        var extra = new List<(NpgsqlDbType Type, object Value)>();
        var idx = 1;

        if (kind is not null)
        {
            clauses.Add($"kind = ${idx++}");
            extra.Add((NpgsqlDbType.Text, kind));
        }
        string? escapedPath = pathFilter is null ? null : EscapeIlike(pathFilter);
        if (escapedPath is not null)
        {
            clauses.Add($"path ILIKE '%' || ${idx++} || '%' ESCAPE '\\'");
            extra.Add((NpgsqlDbType.Text, escapedPath));
        }
        if (userIdFilter is Guid uid)
        {
            clauses.Add($"user_id = ${idx++}");
            extra.Add((NpgsqlDbType.Uuid, uid));
        }

        var order = $"ORDER BY logged_at DESC LIMIT ${idx} OFFSET ${idx + 1}";
        var sql = clauses.Count == 0
            ? $"{baseSql} {order}"
            : $"{baseSql} AND {string.Join(" AND ", clauses)} {order}";

        var rows = new List<PerfLogRow>();
        await using var cmd = dataSource.CreateCommand(sql);
        foreach (var (type, value) in extra)
        {
            cmd.Parameters.Add(new NpgsqlParameter { NpgsqlDbType = type, Value = value });
        }
        cmd.Parameters.AddWithValue(limit);
        cmd.Parameters.AddWithValue(offset);

        await using var reader = await cmd.ExecuteReaderAsync(ct);
        while (await reader.ReadAsync(ct))
        {
            rows.Add(new PerfLogRow
            {
                id = reader.GetInt64(0),
                logged_at = reader.GetDateTime(1),
                user_id = reader.IsDBNull(2) ? (Guid?)null : reader.GetGuid(2),
                session_id = reader.IsDBNull(3) ? null : reader.GetString(3),
                kind = reader.GetString(4),
                method = reader.IsDBNull(5) ? null : reader.GetString(5),
                path = reader.IsDBNull(6) ? null : reader.GetString(6),
                status = reader.IsDBNull(7) ? (short?)null : reader.GetInt16(7),
                total_ms = reader.IsDBNull(8) ? (float?)null : reader.GetFloat(8),
                server_ms = reader.IsDBNull(9) ? (float?)null : reader.GetFloat(9),
                network_ms = reader.IsDBNull(10) ? (float?)null : reader.GetFloat(10),
                source = reader.IsDBNull(11) ? null : reader.GetString(11),
                component = reader.IsDBNull(12) ? null : reader.GetString(12),
                trigger = reader.IsDBNull(13) ? null : reader.GetString(13),
                render_ms = reader.IsDBNull(14) ? (float?)null : reader.GetFloat(14),
                item_count = reader.IsDBNull(15) ? (int?)null : reader.GetInt32(15),
                meta = reader.IsDBNull(16) ? null : RawJson(reader.GetString(16)),
            });
        }

        return rows;
    }

    // ── stats (mirrors db::perf_log::stats — last 24h) ──────────────────────
    private static async Task<object> StatsAsync(NpgsqlDataSource dataSource, CancellationToken ct)
    {
        const string sql =
            "SELECT " +
            "COUNT(*) FILTER (WHERE kind = 'api') AS api_count, " +
            "COUNT(*) FILTER (WHERE kind = 'render') AS render_count, " +
            "AVG(total_ms) FILTER (WHERE kind = 'api') AS avg_total_ms, " +
            "AVG(server_ms) FILTER (WHERE kind = 'api') AS avg_server_ms, " +
            "AVG(render_ms) FILTER (WHERE kind = 'render') AS avg_render_ms, " +
            "PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY total_ms) FILTER (WHERE kind = 'api') AS p95_total_ms, " +
            "PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY render_ms) FILTER (WHERE kind = 'render') AS p95_render_ms, " +
            "COUNT(*) FILTER (WHERE kind = 'api' AND total_ms > 200) AS slow_api_count, " +
            "COUNT(*) FILTER (WHERE kind = 'render' AND render_ms > 16) AS janky_render_count " +
            "FROM perf_log WHERE logged_at >= NOW() - INTERVAL '24 hours'";

        await using var cmd = dataSource.CreateCommand(sql);
        await using var reader = await cmd.ExecuteReaderAsync(ct);
        await reader.ReadAsync(ct);

        long GetCount(int i) => reader.IsDBNull(i) ? 0 : reader.GetInt64(i);
        double? GetDbl(int i) => reader.IsDBNull(i) ? null : reader.GetDouble(i);

        return new
        {
            api_count = GetCount(0),
            render_count = GetCount(1),
            avg_total_ms = GetDbl(2),
            avg_server_ms = GetDbl(3),
            avg_render_ms = GetDbl(4),
            p95_total_ms = GetDbl(5),
            p95_render_ms = GetDbl(6),
            slow_api_count = GetCount(7),
            janky_render_count = GetCount(8),
        };
    }

    // ── helpers ─────────────────────────────────────────────────────────────
    public static string EscapeIlike(string input) =>
        input.Replace("\\", "\\\\").Replace("%", "\\%").Replace("_", "\\_");

    private static NpgsqlParameter NullableText(string? v) =>
        new() { NpgsqlDbType = NpgsqlDbType.Text, Value = (object?)v ?? DBNull.Value };

    private static NpgsqlParameter Nullable(NpgsqlDbType type, object? v) =>
        new() { NpgsqlDbType = type, Value = v ?? DBNull.Value };

    private static NpgsqlParameter NullableJsonb(JsonElement? v) =>
        new() { NpgsqlDbType = NpgsqlDbType.Jsonb, Value = v is { } je ? je.GetRawText() : DBNull.Value };

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

    // ── DTOs ─────────────────────────────────────────────────────────────────
    public sealed class IngestRequest
    {
        public string? session_id { get; set; }
        public List<PerfLogInput>? entries { get; set; }
    }

    public sealed class PerfLogInput
    {
        public string? kind { get; set; }
        public long? timestamp { get; set; }
        public string? method { get; set; }
        public string? path { get; set; }
        public short? status { get; set; }
        public float? total_ms { get; set; }
        public float? server_ms { get; set; }
        public float? network_ms { get; set; }
        public string? source { get; set; }
        public string? component { get; set; }
        public string? trigger { get; set; }
        public float? render_ms { get; set; }
        public int? item_count { get; set; }
        public JsonElement? meta { get; set; }
    }

    public sealed class PerfLogRow
    {
        public long id { get; set; }
        public DateTime logged_at { get; set; }
        public Guid? user_id { get; set; }
        public string? session_id { get; set; }
        public string kind { get; set; } = string.Empty;
        public string? method { get; set; }
        public string? path { get; set; }
        public short? status { get; set; }
        public float? total_ms { get; set; }
        public float? server_ms { get; set; }
        public float? network_ms { get; set; }
        public string? source { get; set; }
        public string? component { get; set; }
        public string? trigger { get; set; }
        public float? render_ms { get; set; }
        public int? item_count { get; set; }
        [JsonConverter(typeof(RawObjectJsonConverter))]
        public object? meta { get; set; }
    }
}

/// <summary>Passes through a JsonElement/object as raw JSON.</summary>
public sealed class RawObjectJsonConverter : JsonConverter<object?>
{
    public override object? Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
        => JsonElement.ParseValue(ref reader);

    public override void Write(Utf8JsonWriter writer, object? value, JsonSerializerOptions options)
    {
        if (value is JsonElement je)
        {
            je.WriteTo(writer);
        }
        else if (value is null)
        {
            writer.WriteNullValue();
        }
        else
        {
            JsonSerializer.Serialize(writer, value, value.GetType(), options);
        }
    }
}
