using System.Text.Json;
using System.Text.Json.Serialization;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;
using Npgsql;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// REST v2 read endpoints for test runs — the C# port of the Rust
/// <c>crates/networker-dashboard/src/api/test_runs.rs</c> list / get / artifact
/// handlers. JSON field names are snake_case to match the Rust
/// <c>networker_common::TestRun</c> and <c>BenchmarkArtifact</c> wire shapes so
/// the existing frontend consumes either backend unchanged.
///
/// Beyond the Rust shape, the list/detail responses add one computed field:
/// <c>result_status</c> — the shared completed-with-failures verdict
/// (<see cref="RunVerdict.ResultStatus"/>); <c>status</c> stays the raw stored
/// lifecycle value.
///
/// Mutating routes (cancel / compare) live elsewhere; <c>/attempts</c> is the
/// read route the run-detail page polls and is served here.
/// </summary>
public static class TestRunsEndpoints
{
    private const int DefaultLimit = 50;
    private const int MaxLimit = 200;

    public static IEndpointRouteBuilder MapTestRunsEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/v2/projects/{projectId}/test-runs — list with filters. Joins
        // TestRun→TestConfig for the config name and endpoint_kind. Mirrors the
        // Rust list_handler + db::test_runs::list, but folds in the config join
        // and the endpoint_kind / before-cursor filters the Rust DB layer had
        // left as TODOs.
        app.MapGet("/api/v2/projects/{projectId}/test-runs", async (
            string projectId,
            string? status,
            string? endpoint_kind,
            bool? has_artifact,
            Guid? comparison_group_id,
            int? limit,
            DateTime? before,
            NetworkerDbContext db) =>
        {
            var take = Math.Clamp(limit ?? DefaultLimit, 1, MaxLimit);

            var query = db.TestRuns
                .AsNoTracking()
                .Where(r => r.ProjectId == projectId);

            if (!string.IsNullOrEmpty(status))
            {
                query = query.Where(r => r.Status == status);
            }

            if (has_artifact is bool wantArtifact)
            {
                query = wantArtifact
                    ? query.Where(r => r.ArtifactId != null)
                    : query.Where(r => r.ArtifactId == null);
            }

            if (comparison_group_id is Guid cgid)
            {
                query = query.Where(r => r.ComparisonGroupId == cgid);
            }

            if (before is DateTime cursor)
            {
                // `before` is a keyset cursor over created_at DESC (exclusive).
                query = query.Where(r => r.CreatedAt < cursor);
            }

            // endpoint_kind lives on the config, so filter through the relation.
            if (!string.IsNullOrEmpty(endpoint_kind))
            {
                query = query.Where(r => r.TestConfig.EndpointKind == endpoint_kind);
            }

            var rows = await query
                .OrderByDescending(r => r.CreatedAt)
                .Take(take)
                .Select(r => new
                {
                    id = r.Id,
                    test_config_id = r.TestConfigId,
                    project_id = r.ProjectId,
                    status = r.Status,
                    started_at = r.StartedAt,
                    finished_at = r.FinishedAt,
                    success_count = r.SuccessCount,
                    failure_count = r.FailureCount,
                    error_message = r.ErrorMessage,
                    artifact_id = r.ArtifactId,
                    tester_id = r.TesterId,
                    worker_id = r.WorkerId,
                    last_heartbeat = r.LastHeartbeat,
                    created_at = r.CreatedAt,
                    comparison_group_id = r.ComparisonGroupId,
                    // Extra denormalized fields the Runs table needs; the join is
                    // why this endpoint is "fuller" than the base TestRun shape.
                    config_name = r.TestConfig.Name,
                    endpoint_kind = r.TestConfig.EndpointKind,
                })
                .ToListAsync();

            // result_status is computed in memory (RunVerdict is not
            // EF-translatable) — `status` stays the raw stored value.
            var shaped = rows.Select(r => new
            {
                r.id,
                r.test_config_id,
                r.project_id,
                r.status,
                result_status = RunVerdict.ResultStatus(r.status, r.success_count, r.failure_count),
                r.started_at,
                r.finished_at,
                r.success_count,
                r.failure_count,
                r.error_message,
                r.artifact_id,
                r.tester_id,
                r.worker_id,
                r.last_heartbeat,
                r.created_at,
                r.comparison_group_id,
                r.config_name,
                r.endpoint_kind,
            });

            return Results.Ok(shaped);
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        // GET /api/v2/test-runs/{id} — single run detail.
        // Flat route (no {projectId}), so the ProjectMember policy can't resolve a
        // project scope. Instead: load the row, then row-level authz via
        // ProjectAccessChecker against run.ProjectId. No access → 404 (identical
        // to not-found, so the route is not an existence oracle for other
        // projects' run ids).
        app.MapGet("/api/v2/test-runs/{id:guid}", async (
            Guid id,
            HttpContext ctx,
            ProjectAccessChecker access,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var run = await db.TestRuns
                .AsNoTracking()
                .Where(r => r.Id == id)
                .Select(r => new
                {
                    id = r.Id,
                    test_config_id = r.TestConfigId,
                    project_id = r.ProjectId,
                    status = r.Status,
                    started_at = r.StartedAt,
                    finished_at = r.FinishedAt,
                    success_count = r.SuccessCount,
                    failure_count = r.FailureCount,
                    error_message = r.ErrorMessage,
                    artifact_id = r.ArtifactId,
                    tester_id = r.TesterId,
                    worker_id = r.WorkerId,
                    last_heartbeat = r.LastHeartbeat,
                    created_at = r.CreatedAt,
                    comparison_group_id = r.ComparisonGroupId,
                })
                .FirstOrDefaultAsync(ct);

            if (run is null ||
                !await access.HasRoleAsync(ctx, run.project_id, ProjectRole.Viewer, ct))
            {
                return Results.NotFound();
            }

            return Results.Ok(new
            {
                run.id,
                run.test_config_id,
                run.project_id,
                run.status,
                result_status = RunVerdict.ResultStatus(
                    run.status, run.success_count, run.failure_count),
                run.started_at,
                run.finished_at,
                run.success_count,
                run.failure_count,
                run.error_message,
                run.artifact_id,
                run.tester_id,
                run.worker_id,
                run.last_heartbeat,
                run.created_at,
                run.comparison_group_id,
            });
        }).RequireAuthorization();

        // GET /api/v2/test-runs/{id}/attempts — per-attempt rows for a run.
        // The run-detail page polls this; it previously 404'd for EVERY run
        // because the route was never ported from the Rust dashboard (audit
        // F3). Semantics: 404 ONLY when the run does not exist or the caller
        // has no access (same non-oracle rule as the detail route); an
        // existing run always returns 200 with `{ "attempts": [...] }` — the
        // envelope the legacy Rust handler returned — empty when the tester
        // engine hasn't persisted probe rows for it (benchmark-style runs,
        // tester schema absent, or DB-less testers).
        //
        // Attempt rows live in the tester-owned V001 schema (RequestAttempt),
        // which is NOT part of the EF model — raw Npgsql, same pattern as
        // Alerting.RunMetricProvider / UrlTestsEndpoints.
        app.MapGet("/api/v2/test-runs/{id:guid}/attempts", async (
            Guid id,
            HttpContext ctx,
            ProjectAccessChecker access,
            NetworkerDbContext db,
            NpgsqlDataSource dataSource,
            CancellationToken ct) =>
        {
            var runProjectId = await db.TestRuns
                .AsNoTracking()
                .Where(r => r.Id == id)
                .Select(r => r.ProjectId)
                .FirstOrDefaultAsync(ct);

            if (runProjectId is null ||
                !await access.HasRoleAsync(ctx, runProjectId, ProjectRole.Viewer, ct))
            {
                return Results.NotFound();
            }

            var attempts = await LoadAttemptsAsync(dataSource, id, ct);
            return Results.Ok(new AttemptListResponse(attempts));
        }).RequireAuthorization();

        // GET /api/v2/test-runs/{id}/artifact — the BenchmarkArtifact for a run.
        // Mirrors Rust artifact_handler + db::benchmark_artifacts::get_for_run
        // (newest artifact for the run). The JSONB columns are stored as text in
        // the C# entity; we re-emit them as raw JSON (not escaped strings) so the
        // wire shape matches the Rust serde_json::Value fields.
        // Flat route: row-level authz via the parent run's ProjectId; no access
        // (or unknown run) → 404, same as a missing artifact.
        app.MapGet("/api/v2/test-runs/{id:guid}/artifact", async (
            Guid id,
            HttpContext ctx,
            ProjectAccessChecker access,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var runProjectId = await db.TestRuns
                .AsNoTracking()
                .Where(r => r.Id == id)
                .Select(r => r.ProjectId)
                .FirstOrDefaultAsync(ct);

            if (runProjectId is null ||
                !await access.HasRoleAsync(ctx, runProjectId, ProjectRole.Viewer, ct))
            {
                return Results.NotFound();
            }

            var art = await db.BenchmarkArtifacts
                .AsNoTracking()
                .Where(a => a.TestRunId == id)
                .OrderByDescending(a => a.CreatedAt)
                .FirstOrDefaultAsync(ct);

            if (art is null)
            {
                return Results.NotFound();
            }

            return Results.Ok(new
            {
                id = art.Id,
                test_run_id = art.TestRunId,
                environment = RawJson(art.Environment),
                methodology = RawJson(art.Methodology),
                launches = RawJson(art.Launches),
                cases = RawJson(art.Cases),
                samples = RawJsonOrNull(art.Samples),
                summaries = RawJson(art.Summaries),
                data_quality = RawJson(art.DataQuality),
                created_at = art.CreatedAt,
            });
        }).RequireAuthorization();

        return app;
    }

    // Parse a JSONB-as-text column into a JsonElement so it serializes as raw
    // JSON. Falls back to the original text as a JSON string if it isn't valid
    // JSON (defensive; the DB constraint should guarantee valid JSON).
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

    private static object? RawJsonOrNull(string? value)
        => value is null ? null : RawJson(value);

    /// <summary>
    /// Read the RequestAttempt rows the networker-tester engine persisted for
    /// a run (V001 tester-owned schema; unquoted identifiers fold to lowercase
    /// on both sides, matching how the tester creates the tables). A missing
    /// table (42P01 — no probe has ever written results to this database)
    /// yields an empty list, NOT an error: "no attempt data" is a valid state
    /// for an existing run. Capped defensively; ordered by sequence.
    ///
    /// Each attempt carries the per-phase detail rows the tester wrote to the
    /// V001 phase tables (Dns/Tcp/Tls/Http/Udp/ServerTimingResult), joined via
    /// LEFT JOIN LATERAL … LIMIT 1 so a stray duplicate phase row can never
    /// fan an attempt out into multiple list entries. Fields not persisted by
    /// the tester DB schema (TLS resumed/handshake_kind/tls_backend, HTTP
    /// goodput/cpu/csw, the server_ms/network_ms/split_anomaly split) reach
    /// the dashboard only via the live attempt_event → attempt_result stream,
    /// which forwards the tester's raw JSON verbatim.
    /// </summary>
    private static async Task<List<AttemptView>> LoadAttemptsAsync(
        NpgsqlDataSource dataSource, Guid runId, CancellationToken ct)
    {
        const string richSql = """
            SELECT a.AttemptId, a.Protocol, a.SequenceNum, a.StartedAt, a.FinishedAt,
                   a.Success, a.ErrorMessage, a.RetryCount,
                   d.DurationMs, d.Success, d.QueryName, d.ResolvedIPs,
                   t.ConnectDurationMs, t.RemoteAddr, t.MssBytesEstimate, t.RttEstimateMs,
                   t.Retransmits, t.TotalRetrans, t.SndCwnd, t.CongestionAlgorithm,
                   t.DeliveryRateBps, t.MinRttMs,
                   s.HandshakeDurationMs, s.ProtocolVersion, s.CipherSuite,
                   s.AlpnNegotiated, s.CertExpiry,
                   h.StatusCode, h.NegotiatedVersion, h.TtfbMs, h.TotalDurationMs,
                   h.BodySizeBytes, h.RedirectCount, h.PayloadBytes, h.ThroughputMbps,
                   u.RttAvgMs, u.RttMinMs, u.RttP95Ms, u.JitterMs, u.LossPercent,
                   u.ProbeCount, u.SuccessCount,
                   st.ProcessingMs, st.RecvBodyMs, st.TotalServerMs
            FROM RequestAttempt a
            LEFT JOIN LATERAL (SELECT * FROM DnsResult  x WHERE x.AttemptId = a.AttemptId LIMIT 1) d  ON TRUE
            LEFT JOIN LATERAL (SELECT * FROM TcpResult  x WHERE x.AttemptId = a.AttemptId LIMIT 1) t  ON TRUE
            LEFT JOIN LATERAL (SELECT * FROM TlsResult  x WHERE x.AttemptId = a.AttemptId LIMIT 1) s  ON TRUE
            LEFT JOIN LATERAL (SELECT * FROM HttpResult x WHERE x.AttemptId = a.AttemptId LIMIT 1) h  ON TRUE
            LEFT JOIN LATERAL (SELECT * FROM UdpResult  x WHERE x.AttemptId = a.AttemptId LIMIT 1) u  ON TRUE
            LEFT JOIN LATERAL (SELECT * FROM ServerTimingResult x WHERE x.AttemptId = a.AttemptId LIMIT 1) st ON TRUE
            WHERE a.RunId = $1
            ORDER BY a.SequenceNum, a.StartedAt
            LIMIT 10000
            """;

        // Pre-phase-table fallback shape — kept so a partially-created tester
        // schema (RequestAttempt present, a phase table missing) still serves
        // the flat rows it used to instead of degrading to an empty list.
        const string flatSql = """
            SELECT AttemptId, Protocol, SequenceNum, StartedAt, FinishedAt,
                   Success, ErrorMessage, RetryCount
            FROM RequestAttempt
            WHERE RunId = $1
            ORDER BY SequenceNum, StartedAt
            LIMIT 10000
            """;

        try
        {
            return await QueryAttemptsAsync(dataSource, richSql, runId, rich: true, ct);
        }
        catch (PostgresException ex) when (ex.SqlState == PostgresErrorCodes.UndefinedTable)
        {
            // A phase table is missing — retry flat; if RequestAttempt itself
            // is missing this throws 42P01 again and the outer catch applies.
        }

        try
        {
            return await QueryAttemptsAsync(dataSource, flatSql, runId, rich: false, ct);
        }
        catch (PostgresException ex) when (ex.SqlState == PostgresErrorCodes.UndefinedTable)
        {
            // Tester result schema not present — an existing run with no
            // recorded attempts, i.e. an empty (200) list.
            return new List<AttemptView>();
        }
    }

    private static async Task<List<AttemptView>> QueryAttemptsAsync(
        NpgsqlDataSource dataSource, string sql, Guid runId, bool rich, CancellationToken ct)
    {
        var attempts = new List<AttemptView>();
        await using var cmd = dataSource.CreateCommand(sql);
        cmd.Parameters.AddWithValue(runId);
        await using var reader = await cmd.ExecuteReaderAsync(ct);
        while (await reader.ReadAsync(ct))
        {
            attempts.Add(new AttemptView(
                AttemptId: reader.GetGuid(0),
                Protocol: reader.GetString(1),
                SequenceNum: reader.GetInt32(2),
                StartedAt: reader.GetDateTime(3),
                FinishedAt: reader.IsDBNull(4) ? null : reader.GetDateTime(4),
                Success: reader.GetBoolean(5),
                // Tester-written text can carry ANSI codes (the Rust side
                // owns that write path) — scrub on emit so API consumers
                // get clean data (audit F8).
                ErrorMessage: reader.IsDBNull(6) ? null : AnsiText.Strip(reader.GetString(6)),
                RetryCount: reader.GetInt32(7),
                Dns: rich ? ReadDns(reader) : null,
                Tcp: rich ? ReadTcp(reader) : null,
                Tls: rich ? ReadTls(reader) : null,
                Http: rich ? ReadHttp(reader) : null,
                Udp: rich ? ReadUdp(reader) : null,
                ServerTiming: rich ? ReadServerTiming(reader) : null));
        }
        return attempts;
    }

    // Per-phase readers for the rich query above. Ordinals are positional in
    // the SELECT list; each phase's first NOT NULL column doubles as the
    // "phase row exists" marker (LEFT JOIN yields all-NULL when absent).

    private static AttemptDnsView? ReadDns(NpgsqlDataReader r) =>
        r.IsDBNull(8) ? null : new AttemptDnsView(
            DurationMs: r.GetDouble(8),
            Success: r.GetBoolean(9),
            QueryName: r.GetString(10),
            // The tester persists resolved IPs comma-joined (postgres.rs
            // insert_dns); split back to the JSON array shape.
            ResolvedIps: r.GetString(11)
                .Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries));

    private static AttemptTcpView? ReadTcp(NpgsqlDataReader r) =>
        r.IsDBNull(12) ? null : new AttemptTcpView(
            ConnectDurationMs: r.GetDouble(12),
            RemoteAddr: r.GetString(13),
            MssBytes: r.IsDBNull(14) ? null : r.GetInt32(14),
            RttEstimateMs: r.IsDBNull(15) ? null : r.GetDouble(15),
            Retransmits: r.IsDBNull(16) ? null : r.GetInt64(16),
            TotalRetrans: r.IsDBNull(17) ? null : r.GetInt64(17),
            SndCwnd: r.IsDBNull(18) ? null : r.GetInt64(18),
            CongestionAlgorithm: r.IsDBNull(19) ? null : r.GetString(19),
            DeliveryRateBps: r.IsDBNull(20) ? null : r.GetInt64(20),
            MinRttMs: r.IsDBNull(21) ? null : r.GetDouble(21));

    private static AttemptTlsView? ReadTls(NpgsqlDataReader r) =>
        r.IsDBNull(22) ? null : new AttemptTlsView(
            HandshakeDurationMs: r.GetDouble(22),
            ProtocolVersion: r.GetString(23),
            CipherSuite: r.GetString(24),
            AlpnNegotiated: r.IsDBNull(25) ? null : r.GetString(25),
            CertExpiry: r.IsDBNull(26) ? null : r.GetDateTime(26));

    private static AttemptHttpView? ReadHttp(NpgsqlDataReader r) =>
        r.IsDBNull(27) ? null : new AttemptHttpView(
            StatusCode: r.GetInt32(27),
            NegotiatedVersion: r.GetString(28),
            TtfbMs: r.GetDouble(29),
            TotalDurationMs: r.GetDouble(30),
            BodySizeBytes: r.GetInt32(31),
            RedirectCount: r.GetInt32(32),
            PayloadBytes: r.IsDBNull(33) ? null : r.GetInt64(33),
            ThroughputMbps: r.IsDBNull(34) ? null : r.GetDouble(34));

    private static AttemptUdpView? ReadUdp(NpgsqlDataReader r) =>
        r.IsDBNull(35) ? null : new AttemptUdpView(
            RttAvgMs: r.GetDouble(35),
            RttMinMs: r.GetDouble(36),
            RttP95Ms: r.GetDouble(37),
            JitterMs: r.GetDouble(38),
            LossPercent: r.GetDouble(39),
            ProbeCount: r.GetInt32(40),
            SuccessCount: r.GetInt32(41));

    private static AttemptServerTimingView? ReadServerTiming(NpgsqlDataReader r)
    {
        // Every ServerTimingResult column is nullable — treat an all-NULL row
        // (or no row) as "no server timing" rather than emitting an empty
        // object the frontend would render as a blank card.
        double? processing = r.IsDBNull(42) ? null : r.GetDouble(42);
        double? recvBody = r.IsDBNull(43) ? null : r.GetDouble(43);
        double? totalServer = r.IsDBNull(44) ? null : r.GetDouble(44);
        return processing is null && recvBody is null && totalServer is null
            ? null
            : new AttemptServerTimingView(processing, recvBody, totalServer);
    }
}

/// <summary>
/// The pinned wire shape of <c>GET /api/v2/test-runs/{id}/attempts</c> — the
/// <c>{ "attempts": [...] }</c> envelope the legacy Rust handler returned and
/// the frontend client types. Pinned by <c>TestRunsContractTests</c>.
/// </summary>
public sealed record AttemptListResponse(
    [property: JsonPropertyName("attempts")] IReadOnlyList<AttemptView> Attempts);

/// <summary>
/// One attempt row — mirrors the tester's <c>RequestAttempt</c> table and the
/// frontend <c>Attempt</c> / <c>LiveAttempt</c> types
/// (<c>dashboard/src/api/types.ts</c>). The nested phase objects use the SAME
/// snake_case field names as the tester's live JSON (Rust <c>metrics.rs</c>),
/// so the frontend renders REST-loaded and live-streamed attempts through one
/// code path. All nested objects are omitted (not <c>null</c>) when the tester
/// persisted no phase row, keeping the pre-widening wire shape byte-identical
/// for old data (pinned by <c>TestRunsContractTests</c>).
/// </summary>
public sealed record AttemptView(
    [property: JsonPropertyName("attempt_id")] Guid AttemptId,
    [property: JsonPropertyName("protocol")] string Protocol,
    [property: JsonPropertyName("sequence_num")] int SequenceNum,
    [property: JsonPropertyName("started_at")] DateTime StartedAt,
    [property: JsonPropertyName("finished_at")] DateTime? FinishedAt,
    [property: JsonPropertyName("success")] bool Success,
    [property: JsonPropertyName("error_message")] string? ErrorMessage,
    [property: JsonPropertyName("retry_count")] int RetryCount,
    [property: JsonPropertyName("dns"),
     JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    AttemptDnsView? Dns = null,
    [property: JsonPropertyName("tcp"),
     JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    AttemptTcpView? Tcp = null,
    [property: JsonPropertyName("tls"),
     JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    AttemptTlsView? Tls = null,
    [property: JsonPropertyName("http"),
     JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    AttemptHttpView? Http = null,
    [property: JsonPropertyName("udp"),
     JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    AttemptUdpView? Udp = null,
    [property: JsonPropertyName("server_timing"),
     JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    AttemptServerTimingView? ServerTiming = null);

/// <summary>DNS phase of one attempt (tester <c>DnsResult</c> table).</summary>
public sealed record AttemptDnsView(
    [property: JsonPropertyName("duration_ms")] double DurationMs,
    [property: JsonPropertyName("success")] bool Success,
    [property: JsonPropertyName("query_name")] string QueryName,
    [property: JsonPropertyName("resolved_ips")] IReadOnlyList<string> ResolvedIps);

/// <summary>
/// TCP phase of one attempt (tester <c>TcpResult</c> table). The kernel-stat
/// columns are best-effort — null on Windows testers / old kernels. The DB
/// column <c>MssBytesEstimate</c> is re-emitted as <c>mss_bytes</c> to match
/// the tester's live JSON field name.
/// </summary>
public sealed record AttemptTcpView(
    [property: JsonPropertyName("connect_duration_ms")] double ConnectDurationMs,
    [property: JsonPropertyName("remote_addr")] string RemoteAddr,
    [property: JsonPropertyName("mss_bytes"),
     JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    int? MssBytes,
    [property: JsonPropertyName("rtt_estimate_ms"),
     JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    double? RttEstimateMs,
    [property: JsonPropertyName("retransmits"),
     JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    long? Retransmits,
    [property: JsonPropertyName("total_retrans"),
     JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    long? TotalRetrans,
    [property: JsonPropertyName("snd_cwnd"),
     JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    long? SndCwnd,
    [property: JsonPropertyName("congestion_algorithm"),
     JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    string? CongestionAlgorithm,
    [property: JsonPropertyName("delivery_rate_bps"),
     JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    long? DeliveryRateBps,
    [property: JsonPropertyName("min_rtt_ms"),
     JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    double? MinRttMs);

/// <summary>TLS phase of one attempt (tester <c>TlsResult</c> table).</summary>
public sealed record AttemptTlsView(
    [property: JsonPropertyName("handshake_duration_ms")] double HandshakeDurationMs,
    [property: JsonPropertyName("protocol_version")] string ProtocolVersion,
    [property: JsonPropertyName("cipher_suite")] string CipherSuite,
    [property: JsonPropertyName("alpn_negotiated"),
     JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    string? AlpnNegotiated,
    [property: JsonPropertyName("cert_expiry"),
     JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    DateTime? CertExpiry);

/// <summary>HTTP phase of one attempt (tester <c>HttpResult</c> table).</summary>
public sealed record AttemptHttpView(
    [property: JsonPropertyName("status_code")] int StatusCode,
    [property: JsonPropertyName("negotiated_version")] string NegotiatedVersion,
    [property: JsonPropertyName("ttfb_ms")] double TtfbMs,
    [property: JsonPropertyName("total_duration_ms")] double TotalDurationMs,
    [property: JsonPropertyName("body_size_bytes")] int BodySizeBytes,
    [property: JsonPropertyName("redirect_count")] int RedirectCount,
    [property: JsonPropertyName("payload_bytes"),
     JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    long? PayloadBytes,
    [property: JsonPropertyName("throughput_mbps"),
     JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    double? ThroughputMbps);

/// <summary>UDP phase of one attempt (tester <c>UdpResult</c> table).</summary>
public sealed record AttemptUdpView(
    [property: JsonPropertyName("rtt_avg_ms")] double RttAvgMs,
    [property: JsonPropertyName("rtt_min_ms")] double RttMinMs,
    [property: JsonPropertyName("rtt_p95_ms")] double RttP95Ms,
    [property: JsonPropertyName("jitter_ms")] double JitterMs,
    [property: JsonPropertyName("loss_percent")] double LossPercent,
    [property: JsonPropertyName("probe_count")] int ProbeCount,
    [property: JsonPropertyName("success_count")] int SuccessCount);

/// <summary>
/// Server-timing phase of one attempt (tester <c>ServerTimingResult</c> table).
/// Only the header-derived timings the tester persists; the computed
/// server/network split (<c>server_ms</c> / <c>network_ms</c> /
/// <c>split_anomaly</c>) is not in the V001 schema and reaches the dashboard
/// via the live attempt stream only.
/// </summary>
public sealed record AttemptServerTimingView(
    [property: JsonPropertyName("processing_ms"),
     JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    double? ProcessingMs,
    [property: JsonPropertyName("recv_body_ms"),
     JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    double? RecvBodyMs,
    [property: JsonPropertyName("total_server_ms"),
     JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    double? TotalServerMs);
