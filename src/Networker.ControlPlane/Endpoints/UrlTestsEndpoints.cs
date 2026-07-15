using System.Text.Json;
using Networker.ControlPlane.Auth;
using Npgsql;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// C# port of the Rust dashboard's <c>api/url_tests.rs</c> project-scoped read
/// endpoints (URL Diagnostics results aggregated from browser page-load runs).
///
/// <para>Routes (all under <c>/api/projects/{projectId}</c>, ProjectMember /
/// Viewer role, matching the Rust <c>project_scoped</c> group):</para>
/// <list type="bullet">
///   <item><b>GET .../url-tests</b> — list summaries (limit/offset, default 50,
///     max 200; offset floored at 0). Project-scoped via the SQL EXISTS filter.</item>
///   <item><b>GET .../url-tests/{run_id}</b> — full detail. NOTE: the Rust
///     handler does NOT scope this by project (it loads by run id only), so this
///     is faithfully reproduced.</item>
///   <item><b>GET .../url-tests/{run_id}/sections</b> — the same detail folded
///     into dashboard sections (overview/timings/protocol/tls/artifacts + derived
///     origin &amp; connection summaries), mirroring <c>section_detail</c>.</item>
/// </list>
///
/// <para>Raw-SQL divergence: <c>UrlTestRun</c> / <c>UrlTestResource</c> /
/// <c>UrlTestProtocolRun</c> are NOT in the EF model (they only exist after
/// browser tests have run and use PascalCase-folded column names). These are read
/// with raw Npgsql via the registered <see cref="NpgsqlDataSource"/>, matching the
/// Rust <c>db::url_tests</c> SQL verbatim. A missing table (undefined_table,
/// 42P01) yields an empty list / 404, exactly as in Rust.</para>
/// </summary>
public static class UrlTestsEndpoints
{
    private const int DefaultLimit = 50;
    private const int MaxLimit = 200;

    public static IEndpointRouteBuilder MapUrlTestsEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/projects/{projectId}/url-tests — list summaries.
        app.MapGet("/api/projects/{projectId}/url-tests", async (
            string projectId,
            int? limit,
            int? offset,
            NpgsqlDataSource dataSource,
            CancellationToken ct) =>
        {
            var take = Math.Clamp(limit ?? DefaultLimit, 1, MaxLimit);
            var skip = Math.Max(offset ?? 0, 0);

            var rows = await ListSummariesAsync(dataSource, projectId, take, skip, ct);
            return Results.Ok(rows);
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        // GET /api/projects/{projectId}/url-tests/{run_id} — full detail (loaded
        // by run id only, matching the Rust handler which ignores project scope).
        app.MapGet("/api/projects/{projectId}/url-tests/{run_id:guid}", async (
            string projectId,
            Guid run_id,
            NpgsqlDataSource dataSource,
            CancellationToken ct) =>
        {
            var detail = await GetDetailAsync(dataSource, run_id, ct);
            return detail is null ? Results.NotFound() : Results.Ok(detail);
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        // GET /api/projects/{projectId}/url-tests/{run_id}/sections — sectioned view.
        app.MapGet("/api/projects/{projectId}/url-tests/{run_id:guid}/sections", async (
            string projectId,
            Guid run_id,
            NpgsqlDataSource dataSource,
            CancellationToken ct) =>
        {
            var detail = await GetDetailAsync(dataSource, run_id, ct);
            return detail is null ? Results.NotFound() : Results.Ok(SectionDetail(detail));
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        return app;
    }

    // ── Data access (raw Npgsql, mirrors db::url_tests) ─────────────────────

    private static bool IsUndefinedTable(PostgresException ex) => ex.SqlState == "42P01";

    private static async Task<List<UrlTestSummary>> ListSummariesAsync(
        NpgsqlDataSource dataSource, string projectId, int limit, int offset, CancellationToken ct)
    {
        const string sql =
            "SELECT u.Id, u.StartedAt, u.CompletedAt, u.RequestedUrl, u.FinalUrl, u.Status, " +
            "u.PageLoadStrategy, u.ObservedProtocolPrimaryLoad, u.TotalRequests, " +
            "u.TotalTransferBytes, u.FailureCount " +
            "FROM UrlTestRun u " +
            "WHERE EXISTS ( " +
            "  SELECT 1 FROM job j WHERE j.project_id = $1 " +
            "  AND j.run_id IN (SELECT RunId FROM TestRun WHERE RunId = u.Id) " +
            ") " +
            "OR NOT EXISTS ( " +
            "  SELECT 1 FROM job j2 WHERE j2.run_id IN (SELECT RunId FROM TestRun WHERE RunId = u.Id) " +
            ") " +
            "ORDER BY u.StartedAt DESC LIMIT $2 OFFSET $3";

        var result = new List<UrlTestSummary>();
        try
        {
            await using var cmd = dataSource.CreateCommand(sql);
            cmd.Parameters.AddWithValue(projectId);
            cmd.Parameters.AddWithValue((long)limit);
            cmd.Parameters.AddWithValue((long)offset);
            await using var reader = await cmd.ExecuteReaderAsync(ct);
            while (await reader.ReadAsync(ct))
            {
                result.Add(new UrlTestSummary
                {
                    id = reader.GetGuid(0),
                    started_at = reader.GetDateTime(1),
                    completed_at = reader.IsDBNull(2) ? null : reader.GetDateTime(2),
                    requested_url = reader.GetString(3),
                    final_url = reader.IsDBNull(4) ? null : reader.GetString(4),
                    status = reader.GetString(5),
                    page_load_strategy = reader.GetString(6),
                    observed_protocol_primary_load = reader.IsDBNull(7) ? null : reader.GetString(7),
                    total_requests = reader.GetInt32(8),
                    total_transfer_bytes = reader.GetInt64(9),
                    failure_count = reader.GetInt32(10),
                });
            }
        }
        catch (PostgresException ex) when (IsUndefinedTable(ex))
        {
            return new List<UrlTestSummary>();
        }

        return result;
    }

    private static async Task<UrlTestDetail?> GetDetailAsync(
        NpgsqlDataSource dataSource, Guid id, CancellationToken ct)
    {
        const string runSql =
            "SELECT Id, StartedAt, CompletedAt, RequestedUrl, FinalUrl, Status, PageLoadStrategy, " +
            "BrowserEngine, BrowserVersion, UserAgent, PrimaryOrigin, ObservedProtocolPrimaryLoad, " +
            "AdvertisedAltSvc, ValidatedHttpVersions, TlsVersion, CipherSuite, Alpn, " +
            "DnsMs, ConnectMs, HandshakeMs, TtfbMs, DomContentLoadedMs, LoadEventMs, " +
            "NetworkIdleMs, CaptureEndMs, TotalRequests, TotalTransferBytes, " +
            "PeakConcurrentConnections, RedirectCount, FailureCount, HarPath, PcapPath, " +
            "PcapSummaryJson, CaptureErrors, EnvironmentNotes " +
            "FROM UrlTestRun WHERE Id = $1";

        UrlTestDetail detail;
        try
        {
            await using var conn = await dataSource.OpenConnectionAsync(ct);
            await using (var cmd = new NpgsqlCommand(runSql, conn))
            {
                cmd.Parameters.AddWithValue(id);
                await using var reader = await cmd.ExecuteReaderAsync(ct);
                if (!await reader.ReadAsync(ct))
                {
                    return null;
                }

                var validatedRaw = reader.IsDBNull(13) ? string.Empty : reader.GetString(13);
                var captureErrRaw = reader.IsDBNull(33) ? string.Empty : reader.GetString(33);
                var pcapJson = reader.IsDBNull(32) ? null : reader.GetString(32);

                detail = new UrlTestDetail
                {
                    id = reader.GetGuid(0),
                    started_at = reader.GetDateTime(1),
                    completed_at = reader.IsDBNull(2) ? null : reader.GetDateTime(2),
                    requested_url = reader.GetString(3),
                    final_url = reader.IsDBNull(4) ? null : reader.GetString(4),
                    status = reader.GetString(5),
                    page_load_strategy = reader.GetString(6),
                    browser_engine = reader.IsDBNull(7) ? null : reader.GetString(7),
                    browser_version = reader.IsDBNull(8) ? null : reader.GetString(8),
                    user_agent = reader.IsDBNull(9) ? null : reader.GetString(9),
                    primary_origin = reader.IsDBNull(10) ? null : reader.GetString(10),
                    observed_protocol_primary_load = reader.IsDBNull(11) ? null : reader.GetString(11),
                    advertised_alt_svc = reader.IsDBNull(12) ? null : reader.GetString(12),
                    validated_http_versions = SplitCsvList(validatedRaw),
                    tls_version = reader.IsDBNull(14) ? null : reader.GetString(14),
                    cipher_suite = reader.IsDBNull(15) ? null : reader.GetString(15),
                    alpn = reader.IsDBNull(16) ? null : reader.GetString(16),
                    dns_ms = reader.IsDBNull(17) ? null : reader.GetDouble(17),
                    connect_ms = reader.IsDBNull(18) ? null : reader.GetDouble(18),
                    handshake_ms = reader.IsDBNull(19) ? null : reader.GetDouble(19),
                    ttfb_ms = reader.IsDBNull(20) ? null : reader.GetDouble(20),
                    dom_content_loaded_ms = reader.IsDBNull(21) ? null : reader.GetDouble(21),
                    load_event_ms = reader.IsDBNull(22) ? null : reader.GetDouble(22),
                    network_idle_ms = reader.IsDBNull(23) ? null : reader.GetDouble(23),
                    capture_end_ms = reader.IsDBNull(24) ? null : reader.GetDouble(24),
                    total_requests = reader.GetInt32(25),
                    total_transfer_bytes = reader.GetInt64(26),
                    peak_concurrent_connections = reader.IsDBNull(27) ? null : reader.GetInt32(27),
                    redirect_count = reader.GetInt32(28),
                    failure_count = reader.GetInt32(29),
                    har_path = RedactPath(reader.IsDBNull(30) ? null : reader.GetString(30)),
                    pcap_path = RedactPath(reader.IsDBNull(31) ? null : reader.GetString(31)),
                    pcap_summary = RedactPcapCapturePath(ParsePcap(pcapJson)),
                    capture_errors = SplitMultilineList(captureErrRaw),
                    environment_notes = reader.IsDBNull(34) ? null : reader.GetString(34),
                    origin_summaries = new List<UrlOriginSummaryView>(),
                    connection_summary = null,
                    resources = new List<UrlTestResourceRow>(),
                    protocol_runs = new List<UrlTestProtocolRunRow>(),
                };
            }

            const string resSql =
                "SELECT ResourceUrl, Origin, ResourceType, MimeType, StatusCode, Protocol, " +
                "TransferSize, EncodedBodySize, DecodedBodySize, DurationMs, ConnectionId, " +
                "ReusedConnection, InitiatorType, FromCache, Redirected, Failed " +
                "FROM UrlTestResource WHERE UrlTestRunId = $1 " +
                "ORDER BY DurationMs DESC NULLS LAST, ResourceUrl ASC";
            await using (var cmd = new NpgsqlCommand(resSql, conn))
            {
                cmd.Parameters.AddWithValue(id);
                await using var reader = await cmd.ExecuteReaderAsync(ct);
                while (await reader.ReadAsync(ct))
                {
                    detail.resources.Add(new UrlTestResourceRow
                    {
                        resource_url = reader.GetString(0),
                        origin = reader.GetString(1),
                        resource_type = reader.GetString(2),
                        mime_type = reader.IsDBNull(3) ? null : reader.GetString(3),
                        status_code = reader.IsDBNull(4) ? null : reader.GetInt32(4),
                        protocol = reader.IsDBNull(5) ? null : reader.GetString(5),
                        transfer_size = reader.IsDBNull(6) ? null : reader.GetInt64(6),
                        encoded_body_size = reader.IsDBNull(7) ? null : reader.GetInt64(7),
                        decoded_body_size = reader.IsDBNull(8) ? null : reader.GetInt64(8),
                        duration_ms = reader.IsDBNull(9) ? null : reader.GetDouble(9),
                        connection_id = reader.IsDBNull(10) ? null : reader.GetString(10),
                        reused_connection = reader.IsDBNull(11) ? null : reader.GetBoolean(11),
                        initiator_type = reader.IsDBNull(12) ? null : reader.GetString(12),
                        from_cache = reader.IsDBNull(13) ? null : reader.GetBoolean(13),
                        redirected = reader.IsDBNull(14) ? null : reader.GetBoolean(14),
                        failed = reader.GetBoolean(15),
                    });
                }
            }

            const string protoSql =
                "SELECT ProtocolMode, RunNumber, AttemptType, ObservedProtocol, FallbackOccurred, " +
                "Succeeded, StatusCode, TtfbMs, TotalMs, FailureReason, Error " +
                "FROM UrlTestProtocolRun WHERE UrlTestRunId = $1 " +
                "ORDER BY ProtocolMode ASC, RunNumber ASC";
            await using (var cmd = new NpgsqlCommand(protoSql, conn))
            {
                cmd.Parameters.AddWithValue(id);
                await using var reader = await cmd.ExecuteReaderAsync(ct);
                while (await reader.ReadAsync(ct))
                {
                    detail.protocol_runs.Add(new UrlTestProtocolRunRow
                    {
                        protocol_mode = reader.GetString(0),
                        run_number = reader.GetInt32(1),
                        attempt_type = reader.GetString(2),
                        observed_protocol = reader.IsDBNull(3) ? null : reader.GetString(3),
                        fallback_occurred = reader.IsDBNull(4) ? null : reader.GetBoolean(4),
                        succeeded = reader.GetBoolean(5),
                        status_code = reader.IsDBNull(6) ? null : reader.GetInt32(6),
                        ttfb_ms = reader.IsDBNull(7) ? null : reader.GetDouble(7),
                        total_ms = reader.IsDBNull(8) ? null : reader.GetDouble(8),
                        failure_reason = reader.IsDBNull(9) ? null : reader.GetString(9),
                        error = reader.IsDBNull(10) ? null : reader.GetString(10),
                    });
                }
            }
        }
        catch (PostgresException ex) when (IsUndefinedTable(ex))
        {
            return null;
        }

        return detail;
    }

    // ── Transforms (mirror db::url_tests helpers) ───────────────────────────

    public static List<string> SplitCsvList(string raw) =>
        raw.Split(',')
            .Select(s => s.Trim())
            .Where(s => s.Length > 0)
            .ToList();

    public static List<string> SplitMultilineList(string raw) =>
        raw.Replace("\r\n", "\n").Split('\n')
            .Select(s => s.Trim())
            .Where(s => s.Length > 0)
            .ToList();

    private static string? RedactPath(string? path)
    {
        if (path is null)
        {
            return null;
        }
        try
        {
            var name = Path.GetFileName(path);
            return string.IsNullOrEmpty(name) ? path : name;
        }
        catch (Exception)
        {
            return path;
        }
    }

    private static UrlPacketCaptureSummaryView? ParsePcap(string? json)
    {
        if (string.IsNullOrEmpty(json))
        {
            return null;
        }
        return JsonSerializer.Deserialize<UrlPacketCaptureSummaryView>(json, PcapJsonOptions);
    }

    private static readonly JsonSerializerOptions PcapJsonOptions = new()
    {
        PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower,
        PropertyNameCaseInsensitive = true,
    };

    private static UrlPacketCaptureSummaryView? RedactPcapCapturePath(UrlPacketCaptureSummaryView? s)
    {
        if (s is null)
        {
            return null;
        }
        s.capture_path = RedactPath(s.capture_path) ?? string.Empty;
        return s;
    }

    // section_detail: fold the flat detail into dashboard sections and derive the
    // origin & connection summaries from the resource rows.
    public static UrlTestSectionedDetail SectionDetail(UrlTestDetail d)
    {
        var (origins, connection) = SummarizeOriginsAndConnections(d.resources);
        return new UrlTestSectionedDetail
        {
            id = d.id,
            started_at = d.started_at,
            completed_at = d.completed_at,
            overview = new UrlTestOverview
            {
                status = d.status,
                requested_url = d.requested_url,
                final_url = d.final_url,
                primary_origin = d.primary_origin,
                observed_protocol_primary_load = d.observed_protocol_primary_load,
                browser_engine = d.browser_engine,
                browser_version = d.browser_version,
                total_requests = d.total_requests,
                total_transfer_bytes = d.total_transfer_bytes,
                failure_count = d.failure_count,
                redirect_count = d.redirect_count,
            },
            timings = new UrlTestTimingSummary
            {
                dns_ms = d.dns_ms,
                connect_ms = d.connect_ms,
                handshake_ms = d.handshake_ms,
                ttfb_ms = d.ttfb_ms,
                dom_content_loaded_ms = d.dom_content_loaded_ms,
                load_event_ms = d.load_event_ms,
                network_idle_ms = d.network_idle_ms,
                capture_end_ms = d.capture_end_ms,
            },
            protocol = new UrlTestProtocolSummary
            {
                page_load_strategy = d.page_load_strategy,
                observed_protocol_primary_load = d.observed_protocol_primary_load,
                validated_http_versions = d.validated_http_versions,
                advertised_alt_svc = d.advertised_alt_svc,
                alpn = d.alpn,
            },
            tls = new UrlTestTlsSummary
            {
                tls_version = d.tls_version,
                cipher_suite = d.cipher_suite,
                alpn = d.alpn,
            },
            artifacts = new UrlTestArtifactSummary
            {
                // section_detail re-applies redaction (idempotent for the paths,
                // and redacts the pcap capture_path), mirroring the Rust helper.
                har_path = RedactPath(d.har_path),
                pcap_path = RedactPath(d.pcap_path),
                pcap_summary = RedactPcapCapturePath(d.pcap_summary),
                capture_errors = d.capture_errors,
                environment_notes = d.environment_notes,
            },
            origin_summaries = origins,
            connection_summary = connection,
            resources = d.resources,
            protocol_runs = d.protocol_runs,
        };
    }

    private sealed class OriginAgg
    {
        public uint RequestCount;
        public uint FailureCount;
        public ulong TotalTransferBytes;
        public readonly SortedDictionary<string, uint> ProtocolCounts = new(StringComparer.Ordinal);
        public double DurationSum;
        public uint DurationCount;
        public uint CacheHitCount;
        public uint CacheKnownCount;
    }

    public static (List<UrlOriginSummaryView>, UrlConnectionSummaryView?) SummarizeOriginsAndConnections(
        List<UrlTestResourceRow> resources)
    {
        var byOrigin = new SortedDictionary<string, OriginAgg>(StringComparer.Ordinal);
        var connectionIds = new SortedSet<string>(StringComparer.Ordinal);
        uint reusedConnectionCount = 0;
        uint reusedResourceCount = 0;
        uint resourcesWithConnectionId = 0;

        foreach (var r in resources)
        {
            if (!byOrigin.TryGetValue(r.origin, out var agg))
            {
                agg = new OriginAgg();
                byOrigin[r.origin] = agg;
            }

            agg.RequestCount += 1;
            if (r.failed)
            {
                agg.FailureCount += 1;
            }
            agg.TotalTransferBytes += (ulong)Math.Max(r.transfer_size ?? 0, 0);
            if (r.protocol is { } proto)
            {
                agg.ProtocolCounts[proto] = agg.ProtocolCounts.GetValueOrDefault(proto) + 1;
            }
            if (r.duration_ms is { } dur)
            {
                agg.DurationSum += dur;
                agg.DurationCount += 1;
            }
            if (r.from_cache is { } fc)
            {
                agg.CacheKnownCount += 1;
                if (fc)
                {
                    agg.CacheHitCount += 1;
                }
            }
            if (r.connection_id is { } cid)
            {
                resourcesWithConnectionId += 1;
                connectionIds.Add(cid);
            }
            if (r.reused_connection == true)
            {
                reusedResourceCount += 1;
                if (r.connection_id is not null)
                {
                    reusedConnectionCount += 1;
                }
            }
        }

        uint? peakOriginRequestCount = byOrigin.Count == 0
            ? null
            : byOrigin.Values.Max(a => a.RequestCount);

        var originSummaries = byOrigin.Select(kv =>
        {
            var agg = kv.Value;
            // Sort by count desc, then protocol name asc (matches Rust sort_by).
            var pairs = agg.ProtocolCounts
                .OrderByDescending(p => p.Value)
                .ThenBy(p => p.Key, StringComparer.Ordinal)
                .ToList();
            var protocols = pairs.Select(p => p.Key).ToList();
            var dominant = pairs.Count > 0 ? pairs[0].Key : null;
            return new UrlOriginSummaryView
            {
                origin = kv.Key,
                request_count = agg.RequestCount,
                failure_count = agg.FailureCount,
                total_transfer_bytes = agg.TotalTransferBytes,
                protocols = protocols,
                dominant_protocol = dominant,
                average_duration_ms = agg.DurationCount > 0
                    ? agg.DurationSum / agg.DurationCount
                    : null,
                cache_hit_count = agg.CacheKnownCount > 0 ? agg.CacheHitCount : null,
            };
        }).ToList();

        UrlConnectionSummaryView? connectionSummary = resources.Count == 0
            ? null
            : new UrlConnectionSummaryView
            {
                total_connection_ids = (uint)connectionIds.Count,
                reused_connection_count = reusedConnectionCount,
                reused_resource_count = reusedResourceCount,
                resources_with_connection_id = resourcesWithConnectionId,
                peak_origin_request_count = peakOriginRequestCount,
            };

        return (originSummaries, connectionSummary);
    }
}

// ── Wire-shape DTOs (snake_case to match Rust serde output) ─────────────────

public sealed class UrlTestSummary
{
    public Guid id { get; set; }
    public DateTime started_at { get; set; }
    public DateTime? completed_at { get; set; }
    public string requested_url { get; set; } = string.Empty;
    public string? final_url { get; set; }
    public string status { get; set; } = string.Empty;
    public string page_load_strategy { get; set; } = string.Empty;
    public string? observed_protocol_primary_load { get; set; }
    public int total_requests { get; set; }
    public long total_transfer_bytes { get; set; }
    public int failure_count { get; set; }
}

public sealed class UrlTestResourceRow
{
    public string resource_url { get; set; } = string.Empty;
    public string origin { get; set; } = string.Empty;
    public string resource_type { get; set; } = string.Empty;
    public string? mime_type { get; set; }
    public int? status_code { get; set; }
    public string? protocol { get; set; }
    public long? transfer_size { get; set; }
    public long? encoded_body_size { get; set; }
    public long? decoded_body_size { get; set; }
    public double? duration_ms { get; set; }
    public string? connection_id { get; set; }
    public bool? reused_connection { get; set; }
    public string? initiator_type { get; set; }
    public bool? from_cache { get; set; }
    public bool? redirected { get; set; }
    public bool failed { get; set; }
}

public sealed class UrlTestProtocolRunRow
{
    public string protocol_mode { get; set; } = string.Empty;
    public int run_number { get; set; }
    public string attempt_type { get; set; } = string.Empty;
    public string? observed_protocol { get; set; }
    public bool? fallback_occurred { get; set; }
    public bool succeeded { get; set; }
    public int? status_code { get; set; }
    public double? ttfb_ms { get; set; }
    public double? total_ms { get; set; }
    public string? failure_reason { get; set; }
    public string? error { get; set; }
}

public sealed class UrlTestOverview
{
    public string status { get; set; } = string.Empty;
    public string requested_url { get; set; } = string.Empty;
    public string? final_url { get; set; }
    public string? primary_origin { get; set; }
    public string? observed_protocol_primary_load { get; set; }
    public string? browser_engine { get; set; }
    public string? browser_version { get; set; }
    public int total_requests { get; set; }
    public long total_transfer_bytes { get; set; }
    public int failure_count { get; set; }
    public int redirect_count { get; set; }
}

public sealed class UrlTestTimingSummary
{
    public double? dns_ms { get; set; }
    public double? connect_ms { get; set; }
    public double? handshake_ms { get; set; }
    public double? ttfb_ms { get; set; }
    public double? dom_content_loaded_ms { get; set; }
    public double? load_event_ms { get; set; }
    public double? network_idle_ms { get; set; }
    public double? capture_end_ms { get; set; }
}

public sealed class UrlTestProtocolSummary
{
    public string page_load_strategy { get; set; } = string.Empty;
    public string? observed_protocol_primary_load { get; set; }
    public List<string> validated_http_versions { get; set; } = new();
    public string? advertised_alt_svc { get; set; }
    public string? alpn { get; set; }
}

public sealed class UrlTestTlsSummary
{
    public string? tls_version { get; set; }
    public string? cipher_suite { get; set; }
    public string? alpn { get; set; }
}

public sealed class UrlPacketCaptureSummaryView
{
    public string mode { get; set; } = string.Empty;
    public string @interface { get; set; } = string.Empty;
    public string capture_path { get; set; } = string.Empty;
    public ulong total_packets { get; set; }
    public string capture_status { get; set; } = string.Empty;
    public string? note { get; set; }
    public List<string> warnings { get; set; } = new();
    public ulong tcp_packets { get; set; }
    public ulong udp_packets { get; set; }
    public ulong quic_packets { get; set; }
    public ulong http_packets { get; set; }
    public ulong dns_packets { get; set; }
    public ulong retransmissions { get; set; }
    public ulong duplicate_acks { get; set; }
    public ulong resets { get; set; }
    public bool observed_quic { get; set; }
    public bool observed_tcp_only { get; set; }
    public bool observed_mixed_transport { get; set; }
    public bool capture_may_be_ambiguous { get; set; }
}

public sealed class UrlOriginSummaryView
{
    public string origin { get; set; } = string.Empty;
    public uint request_count { get; set; }
    public uint failure_count { get; set; }
    public ulong total_transfer_bytes { get; set; }
    public List<string> protocols { get; set; } = new();
    public string? dominant_protocol { get; set; }
    public double? average_duration_ms { get; set; }
    public uint? cache_hit_count { get; set; }
}

public sealed class UrlConnectionSummaryView
{
    public uint total_connection_ids { get; set; }
    public uint reused_connection_count { get; set; }
    public uint reused_resource_count { get; set; }
    public uint resources_with_connection_id { get; set; }
    public uint? peak_origin_request_count { get; set; }
}

public sealed class UrlTestArtifactSummary
{
    public string? har_path { get; set; }
    public string? pcap_path { get; set; }
    public UrlPacketCaptureSummaryView? pcap_summary { get; set; }
    public List<string> capture_errors { get; set; } = new();
    public string? environment_notes { get; set; }
}

public sealed class UrlTestSectionedDetail
{
    public Guid id { get; set; }
    public DateTime started_at { get; set; }
    public DateTime? completed_at { get; set; }
    public UrlTestOverview overview { get; set; } = new();
    public UrlTestTimingSummary timings { get; set; } = new();
    public UrlTestProtocolSummary protocol { get; set; } = new();
    public UrlTestTlsSummary tls { get; set; } = new();
    public UrlTestArtifactSummary artifacts { get; set; } = new();
    public List<UrlOriginSummaryView> origin_summaries { get; set; } = new();
    public UrlConnectionSummaryView? connection_summary { get; set; }
    public List<UrlTestResourceRow> resources { get; set; } = new();
    public List<UrlTestProtocolRunRow> protocol_runs { get; set; } = new();
}

public sealed class UrlTestDetail
{
    public Guid id { get; set; }
    public DateTime started_at { get; set; }
    public DateTime? completed_at { get; set; }
    public string requested_url { get; set; } = string.Empty;
    public string? final_url { get; set; }
    public string status { get; set; } = string.Empty;
    public string page_load_strategy { get; set; } = string.Empty;
    public string? browser_engine { get; set; }
    public string? browser_version { get; set; }
    public string? user_agent { get; set; }
    public string? primary_origin { get; set; }
    public string? observed_protocol_primary_load { get; set; }
    public string? advertised_alt_svc { get; set; }
    public List<string> validated_http_versions { get; set; } = new();
    public string? tls_version { get; set; }
    public string? cipher_suite { get; set; }
    public string? alpn { get; set; }
    public double? dns_ms { get; set; }
    public double? connect_ms { get; set; }
    public double? handshake_ms { get; set; }
    public double? ttfb_ms { get; set; }
    public double? dom_content_loaded_ms { get; set; }
    public double? load_event_ms { get; set; }
    public double? network_idle_ms { get; set; }
    public double? capture_end_ms { get; set; }
    public int total_requests { get; set; }
    public long total_transfer_bytes { get; set; }
    public int? peak_concurrent_connections { get; set; }
    public int redirect_count { get; set; }
    public int failure_count { get; set; }
    public string? har_path { get; set; }
    public string? pcap_path { get; set; }
    public UrlPacketCaptureSummaryView? pcap_summary { get; set; }
    public List<string> capture_errors { get; set; } = new();
    public string? environment_notes { get; set; }
    public List<UrlOriginSummaryView> origin_summaries { get; set; } = new();
    public UrlConnectionSummaryView? connection_summary { get; set; }
    public List<UrlTestResourceRow> resources { get; set; } = new();
    public List<UrlTestProtocolRunRow> protocol_runs { get; set; } = new();
}
