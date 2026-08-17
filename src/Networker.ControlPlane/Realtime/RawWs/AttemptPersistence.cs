using System.Globalization;
using System.Text.Json;
using Npgsql;
using NpgsqlTypes;

namespace Networker.ControlPlane.Realtime.RawWs;

/// <summary>
/// Persists a streamed <c>attempt_event</c> into the tester-owned V001 probe
/// schema (RequestAttempt + the per-phase result tables) so
/// <c>GET /test-runs/{id}/attempts</c> and the reports (perf-per-cost,
/// app-network) have data. This port was missing after the cutover — the C#
/// agent runs the tester with <c>--json-stdout</c> and relays attempts over WS,
/// but nothing wrote them to the DB (E2E pass 2026-07-28 P0-2: a 60/60-success
/// run showed an empty attempts list).
///
/// <para>Two halves, split for testability: <see cref="AttemptExtract.Parse"/>
/// turns the tester's live attempt JSON into a typed <see cref="ParsedAttempt"/>
/// (pure — unit-tested), and <see cref="AttemptPersister.PersistAsync"/> writes
/// it with raw Npgsql matching the exact prod column set. A missing tester
/// schema (42P01) or a non-Postgres connection is skipped, not thrown.</para>
/// </summary>
public static class AttemptPersister
{
    /// <summary>
    /// The full tester probe schema (V001–V005, PostgreSQL) — embedded copy of
    /// <c>shared/tester-schema.postgres.sql</c>, which mirrors the
    /// <c>networker-tester</c> crate's own migrations (guarded by a Rust unit
    /// test). Applied lazily by the INGEST because on the streamed-attempt path
    /// the tester never touches the DB — its own <c>migrate()</c> (postgres.rs)
    /// only runs for DB-backed testers. Discovered live twice: v0.28.126
    /// shipped the V005 read/write sides with nothing creating the table on the
    /// streamed path, and lab/validate.sh showed a FRESH control-plane database
    /// (docker/dev/new install without the install.sh psql seed) persisting 0
    /// attempts forever because RequestAttempt did not exist (42P01 swallowed).
    /// Every statement is idempotent DDL, so re-running on an existing
    /// tester-created schema is a no-op.
    /// </summary>
    private const string TesterSchemaResource = "Networker.ControlPlane.shared.tester-schema.postgres.sql";

    /// <summary>Same advisory-lock key the tester's <c>PostgresBackend::migrate()</c>
    /// takes, so a DB-backed tester and this ingest never run the DDL
    /// concurrently (concurrent CREATE TABLE IF NOT EXISTS can still race on
    /// pg_type). Taken as a transaction-scoped lock.</summary>
    private const long TesterSchemaLockKey = 0x4E54505747524D31;

    private static readonly Lazy<string> TesterSchemaSql = new(() =>
    {
        var asm = typeof(AttemptPersister).Assembly;
        using var stream = asm.GetManifestResourceStream(TesterSchemaResource)
            ?? throw new InvalidOperationException(
                $"Embedded tester schema '{TesterSchemaResource}' missing — check the csproj EmbeddedResource entry.");
        using var reader = new StreamReader(stream);
        return reader.ReadToEnd();
    });

    // 0 = unknown, 1 = schema present (V001–V005 ensured), -1 = unavailable
    // (DDL denied / not Postgres) — probed once per process; the writes below
    // are gated on it so a locked-down DB degrades exactly as before (skip,
    // never abort the whole attempt insert or the live stream).
    private static int _schemaState;

    /// <summary>Test hook: forget the per-process probe result.</summary>
    internal static void ResetSchemaProbeForTests() => Volatile.Write(ref _schemaState, 0);

    /// <summary>The embedded schema text (for tests / diagnostics).</summary>
    internal static string EmbeddedTesterSchema => TesterSchemaSql.Value;

    /// <summary>
    /// Ensure the tester probe schema exists (idempotent DDL under the tester's
    /// migration advisory lock, one transaction) and record V001–V005 in the
    /// tester's <c>_schema_versions</c> bookkeeping so a DB-backed tester that
    /// later points at this database skips them. Returns whether the schema
    /// (incl. V005) is available for the writes.
    /// </summary>
    private static async Task<bool> EnsureTesterSchemaAsync(NpgsqlConnection conn, CancellationToken ct)
    {
        var s = Volatile.Read(ref _schemaState);
        if (s != 0)
        {
            return s == 1;
        }

        try
        {
            await using (var tx = await conn.BeginTransactionAsync(ct))
            {
                await using (var lockCmd = new NpgsqlCommand("SELECT pg_advisory_xact_lock(@k)", conn, tx))
                {
                    lockCmd.Parameters.AddWithValue("k", TesterSchemaLockKey);
                    await lockCmd.ExecuteNonQueryAsync(ct);
                }
                await using (var ddl = new NpgsqlCommand(TesterSchemaSql.Value, conn, tx))
                {
                    await ddl.ExecuteNonQueryAsync(ct);
                }
                // Bookkeeping for the tester-side migrator (same table + rows
                // postgres.rs writes). Best-effort inside the same transaction.
                await using (var rec = new NpgsqlCommand(
                    "CREATE TABLE IF NOT EXISTS _schema_versions (version VARCHAR(20) NOT NULL PRIMARY KEY, applied_at TIMESTAMPTZ NOT NULL DEFAULT now()); "
                    + "INSERT INTO _schema_versions (version) VALUES ('V001'),('V002'),('V003'),('V004'),('V005') ON CONFLICT DO NOTHING",
                    conn, tx))
                {
                    await rec.ExecuteNonQueryAsync(ct);
                }
                await tx.CommitAsync(ct);
            }
            Volatile.Write(ref _schemaState, 1);
            return true;
        }
        catch (PostgresException)
        {
            // Insufficient privilege / read-only replica / not our database:
            // remember and degrade to the pre-existing behaviour (persist into
            // whatever exists; 42P01 is skipped in PersistAsync).
            Volatile.Write(ref _schemaState, -1);
            return false;
        }
    }

    public static async Task PersistAsync(
        NpgsqlConnection conn, ParsedAttempt a, CancellationToken ct)
    {
        if (conn.State != System.Data.ConnectionState.Open)
        {
            await conn.OpenAsync(ct);
        }

        var v005 = await EnsureTesterSchemaAsync(conn, ct);

        // RequestAttempt.RunId FKs to the tester-owned V001 `testrun` table
        // (runid PK) — a DIFFERENT table from the control plane's `test_run`.
        // DB-backed testers used to create it; the C# agent path never does,
        // so the FK would fail (E2E dry-run finding). Upsert a minimal row in
        // its own autocommit statement (idempotent, so it is safe outside the
        // attempt transaction) BEFORE the attempt insert.
        try
        {
            await EnsureTestRunRowAsync(conn, a, ct);
        }
        catch (PostgresException ex) when (ex.SqlState == PostgresErrorCodes.UndefinedTable)
        {
            // Tester probe schema absent and not creatable (DDL denied) —
            // nothing to persist into; same skip posture as the reads.
            return;
        }

        await using var tx = await conn.BeginTransactionAsync(ct);
        try
        {
            // Idempotent on AttemptId: a re-delivered attempt_event must not
            // duplicate rows. Only when the RequestAttempt is NEW do we write
            // its phase rows (their PKs are fresh uuids, so a conflict-skip on
            // the parent is the guard).
            var shape = await DetectShapeAsync(conn, ct);
            var extraCol = shape.ExtraJsonColumn is { } ec ? $", {ec}" : string.Empty;
            var extraVal = shape.ExtraJsonColumn is not null ? ", @extra" : string.Empty;
            var inserted = await ExecAsync(conn, tx, ct,
                "INSERT INTO RequestAttempt "
                + "(AttemptId, RunId, Protocol, SequenceNum, StartedAt, FinishedAt, "
                + $"Success, ErrorMessage, RetryCount{extraCol}) "
                + $"VALUES (@id, @run, @proto, @seq, @started, @finished, @ok, @err, @retry{extraVal}) "
                + "ON CONFLICT (AttemptId) DO NOTHING",
                p =>
                {
                    p.AddWithValue("id", a.AttemptId);
                    p.AddWithValue("run", a.RunId);
                    p.AddWithValue("proto", a.Protocol);
                    p.AddWithValue("seq", a.SequenceNum);
                    AddNullable(p, "started", a.StartedAt);
                    AddNullable(p, "finished", a.FinishedAt);
                    p.AddWithValue("ok", a.Success);
                    AddNullable(p, "err", a.ErrorMessage);
                    p.AddWithValue("retry", a.RetryCount);
                    if (shape.ExtraJsonColumn is not null)
                    {
                        p.Add(new NpgsqlParameter("extra", NpgsqlDbType.Jsonb)
                        { Value = (object?)a.ExtraJson ?? DBNull.Value });
                    }
                });

            if (inserted > 0)
            {
                await WritePhasesAsync(conn, tx, a, v005, ct);
            }

            await tx.CommitAsync(ct);
        }
        catch (PostgresException ex) when (ex.SqlState == PostgresErrorCodes.UndefinedTable
                                        || ex.SqlState == PostgresErrorCodes.UndefinedColumn)
        {
            // Tester probe schema absent (DB-less deployment) — nothing to
            // persist into; roll back and move on, same posture as the reads.
            await tx.RollbackAsync(ct);
        }
    }

    /// <summary>
    /// The two places the fielded probe schema is known to diverge from the
    /// tester's V001 DDL, detected once per process from
    /// <c>information_schema.columns</c> instead of hard-coding either shape:
    /// <list type="bullet">
    ///   <item><see cref="ExtraJsonColumn"/>: the raw-attempt JSON column on
    ///   <c>RequestAttempt</c> — <c>extrajson</c> on the live prod schema (dry-run
    ///   verified 2026-07-28), <c>extra_json</c> where install.sh's psql seed
    ///   created the tables, absent on a schema bootstrapped from the tester's
    ///   V001–V005 (which never adds it). Null = don't write it.</item>
    ///   <item><see cref="TestRunColumns"/>: which of the V001 NOT NULL
    ///   <c>testrun</c> columns (startedat/modes/clientos/clientversion) exist,
    ///   so the parent-row upsert satisfies them where they are declared (a fresh
    ///   bootstrap rejects the historical 3-column insert with 23502 — lab
    ///   finding) and stays minimal where they are not.</item>
    /// </list>
    /// </summary>
    internal sealed record ProbeSchemaShape(string? ExtraJsonColumn, IReadOnlySet<string> TestRunColumns);

    private static ProbeSchemaShape? _shape;

    /// <summary>Test hook: forget the cached shape.</summary>
    internal static void ResetShapeForTests() => Volatile.Write(ref _shape, null);

    /// <summary>Pure shape derivation from lower-cased (table, column) pairs — unit-tested.</summary>
    internal static ProbeSchemaShape DeriveShape(IEnumerable<(string Table, string Column)> columns)
    {
        var attempt = new HashSet<string>(StringComparer.Ordinal);
        var testRun = new HashSet<string>(StringComparer.Ordinal);
        foreach (var (table, column) in columns)
        {
            var t = table.ToLowerInvariant();
            var c = column.ToLowerInvariant();
            if (t == "requestattempt") attempt.Add(c);
            else if (t == "testrun") testRun.Add(c);
        }

        var extra = attempt.Contains("extrajson") ? "extrajson"
            : attempt.Contains("extra_json") ? "extra_json"
            : null;
        var wanted = new HashSet<string>(StringComparer.Ordinal) { "startedat", "modes", "clientos", "clientversion" };
        wanted.IntersectWith(testRun);
        return new ProbeSchemaShape(extra, wanted);
    }

    private static async Task<ProbeSchemaShape> DetectShapeAsync(NpgsqlConnection conn, CancellationToken ct)
    {
        if (Volatile.Read(ref _shape) is { } cached)
        {
            return cached;
        }

        var cols = new List<(string, string)>();
        await using (var cmd = new NpgsqlCommand(
            "SELECT table_name, column_name FROM information_schema.columns "
            + "WHERE table_schema = current_schema() AND lower(table_name) IN ('requestattempt', 'testrun')",
            conn))
        await using (var reader = await cmd.ExecuteReaderAsync(ct))
        {
            while (await reader.ReadAsync(ct))
            {
                cols.Add((reader.GetString(0), reader.GetString(1)));
            }
        }

        var shape = DeriveShape(cols);
        Volatile.Write(ref _shape, shape);
        return shape;
    }

    /// <summary>
    /// Upsert the tester-owned <c>testrun</c> parent row the RequestAttempt FK
    /// needs, filling whichever of the V001 NOT NULL columns the live table
    /// declares (see <see cref="ProbeSchemaShape"/>). Nothing reads these
    /// columns beyond the FK / RunId (URL-test history), so best-effort values
    /// from the attempt are sufficient. Autocommit + idempotent (ON CONFLICT
    /// DO NOTHING), so it is safe outside the attempt transaction.
    /// </summary>
    private static async Task EnsureTestRunRowAsync(NpgsqlConnection conn, ParsedAttempt a, CancellationToken ct)
    {
        var shape = await DetectShapeAsync(conn, ct);
        var cols = "runid, targeturl, targethost";
        var vals = "@run, @url, @host";
        if (shape.TestRunColumns.Contains("startedat")) { cols += ", startedat"; vals += ", @started"; }
        if (shape.TestRunColumns.Contains("modes")) { cols += ", modes"; vals += ", @modes"; }
        if (shape.TestRunColumns.Contains("clientos")) { cols += ", clientos"; vals += ", @os"; }
        if (shape.TestRunColumns.Contains("clientversion")) { cols += ", clientversion"; vals += ", @ver"; }

        await ExecAsync(conn, null, ct,
            $"INSERT INTO testrun ({cols}) VALUES ({vals}) ON CONFLICT (runid) DO NOTHING",
            p =>
            {
                p.AddWithValue("run", a.RunId);
                p.AddWithValue("url", a.TargetUrl);
                p.AddWithValue("host", a.TargetHost);
                if (shape.TestRunColumns.Contains("startedat")) p.AddWithValue("started", a.StartedAt ?? DateTime.UtcNow);
                if (shape.TestRunColumns.Contains("modes")) p.AddWithValue("modes", a.Protocol);
                if (shape.TestRunColumns.Contains("clientos")) p.AddWithValue("os", "unknown");
                if (shape.TestRunColumns.Contains("clientversion")) p.AddWithValue("ver", "unknown");
            });
    }

    private static async Task WritePhasesAsync(
        NpgsqlConnection conn, NpgsqlTransaction tx, ParsedAttempt a, bool v005, CancellationToken ct)
    {
        // Phase tables' StartedAt is NOT NULL; the attempt JSON doesn't carry a
        // per-phase start, so anchor them at the attempt's start (or now()).
        var started = a.StartedAt ?? DateTime.UtcNow;

        if (a.Dns is { } dns)
        {
            await ExecAsync(conn, tx, ct,
                "INSERT INTO DnsResult (DnsId, AttemptId, QueryName, ResolvedIPs, DurationMs, StartedAt, Success) "
                + "VALUES (@pk, @aid, @q, @ips, @dur, @st, @ok)",
                p =>
                {
                    p.AddWithValue("pk", Guid.NewGuid());
                    p.AddWithValue("aid", a.AttemptId);
                    AddNullable(p, "q", dns.QueryName);
                    AddNullable(p, "ips", dns.ResolvedIps);
                    AddNullable(p, "dur", dns.DurationMs);
                    p.AddWithValue("st", started);
                    p.AddWithValue("ok", dns.Success);
                });
        }
        if (a.Tcp is { } tcp)
        {
            await ExecAsync(conn, tx, ct,
                "INSERT INTO TcpResult (TcpId, AttemptId, RemoteAddr, ConnectDurationMs, MssBytesEstimate, "
                + "RttEstimateMs, Retransmits, TotalRetrans, SndCwnd, CongestionAlgorithm, DeliveryRateBps, MinRttMs, StartedAt, Success) "
                + "VALUES (@pk, @aid, @ra, @dur, @mss, @rtt, @rt, @trt, @cwnd, @ca, @dr, @minrtt, @st, @ok)",
                p =>
                {
                    p.AddWithValue("pk", Guid.NewGuid());
                    p.AddWithValue("aid", a.AttemptId);
                    AddNullable(p, "ra", tcp.RemoteAddr);
                    AddNullable(p, "dur", tcp.ConnectDurationMs);
                    AddNullable(p, "mss", tcp.MssBytes);
                    AddNullable(p, "rtt", tcp.RttEstimateMs);
                    AddNullable(p, "rt", tcp.Retransmits);
                    AddNullable(p, "trt", tcp.TotalRetrans);
                    AddNullable(p, "cwnd", tcp.SndCwnd);
                    AddNullable(p, "ca", tcp.CongestionAlgorithm);
                    AddNullable(p, "dr", tcp.DeliveryRateBps);
                    AddNullable(p, "minrtt", tcp.MinRttMs);
                    p.AddWithValue("st", started);
                    p.AddWithValue("ok", true);
                });
        }
        if (a.Tls is { } tls)
        {
            await ExecAsync(conn, tx, ct,
                "INSERT INTO TlsResult (TlsId, AttemptId, ProtocolVersion, CipherSuite, AlpnNegotiated, "
                + "CertExpiry, HandshakeDurationMs, StartedAt, Success) VALUES (@pk, @aid, @pv, @cs, @alpn, @exp, @dur, @st, @ok)",
                p =>
                {
                    p.AddWithValue("pk", Guid.NewGuid());
                    p.AddWithValue("aid", a.AttemptId);
                    AddNullable(p, "pv", tls.ProtocolVersion);
                    AddNullable(p, "cs", tls.CipherSuite);
                    AddNullable(p, "alpn", tls.AlpnNegotiated);
                    AddNullable(p, "exp", tls.CertExpiry);
                    AddNullable(p, "dur", tls.HandshakeDurationMs);
                    p.AddWithValue("st", started);
                    p.AddWithValue("ok", true);
                });
        }
        if (a.Http is { } http)
        {
            await ExecAsync(conn, tx, ct,
                "INSERT INTO HttpResult (HttpId, AttemptId, NegotiatedVersion, StatusCode, BodySizeBytes, "
                + "TtfbMs, TotalDurationMs, RedirectCount, PayloadBytes, ThroughputMbps, StartedAt) "
                + "VALUES (@pk, @aid, @nv, @sc, @body, @ttfb, @dur, @rc, @pb, @tp, @st)",
                p =>
                {
                    p.AddWithValue("pk", Guid.NewGuid());
                    p.AddWithValue("aid", a.AttemptId);
                    AddNullable(p, "nv", http.NegotiatedVersion);
                    AddNullable(p, "sc", http.StatusCode);
                    AddNullable(p, "body", http.BodySizeBytes);
                    AddNullable(p, "ttfb", http.TtfbMs);
                    AddNullable(p, "dur", http.TotalDurationMs);
                    AddNullable(p, "rc", http.RedirectCount);
                    AddNullable(p, "pb", http.PayloadBytes);
                    AddNullable(p, "tp", http.ThroughputMbps);
                    p.AddWithValue("st", started);
                });
        }
        if (a.Udp is { } udp)
        {
            await ExecAsync(conn, tx, ct,
                "INSERT INTO UdpResult (UdpId, AttemptId, RemoteAddr, ProbeCount, SuccessCount, LossPercent, "
                + "RttMinMs, RttAvgMs, RttP95Ms, JitterMs, StartedAt) VALUES (@pk, @aid, @ra, @pc, @sc, @loss, @min, @avg, @p95, @jit, @st)",
                p =>
                {
                    p.AddWithValue("pk", Guid.NewGuid());
                    p.AddWithValue("aid", a.AttemptId);
                    p.AddWithValue("ra", a.TargetHost);   // udpresult.remoteaddr is NOT NULL
                    AddNullable(p, "pc", udp.ProbeCount);
                    AddNullable(p, "sc", udp.SuccessCount);
                    AddNullable(p, "loss", udp.LossPercent);
                    AddNullable(p, "min", udp.RttMinMs);
                    AddNullable(p, "avg", udp.RttAvgMs);
                    AddNullable(p, "p95", udp.RttP95Ms);
                    AddNullable(p, "jit", udp.JitterMs);
                    p.AddWithValue("st", started);
                });
        }
        if (a.ServerTiming is { } st)
        {
            // SrvCpuMs only exists post-V005; use the narrow column set on a
            // pre-V005 schema so the whole attempt isn't rolled back.
            var sql = v005
                ? "INSERT INTO ServerTimingResult (ServerId, AttemptId, RecvBodyMs, ProcessingMs, TotalServerMs, SrvCpuMs) "
                  + "VALUES (@pk, @aid, @recv, @proc, @total, @cpu)"
                : "INSERT INTO ServerTimingResult (ServerId, AttemptId, RecvBodyMs, ProcessingMs, TotalServerMs) "
                  + "VALUES (@pk, @aid, @recv, @proc, @total)";
            await ExecAsync(conn, tx, ct, sql,
                p =>
                {
                    p.AddWithValue("pk", Guid.NewGuid());
                    p.AddWithValue("aid", a.AttemptId);
                    AddNullable(p, "recv", st.RecvBodyMs);
                    AddNullable(p, "proc", st.ProcessingMs);
                    AddNullable(p, "total", st.TotalServerMs);
                    if (v005)
                    {
                        AddNullable(p, "cpu", st.SrvCpuMs);
                    }
                });
        }
        if (v005 && a.Mthroughput is { } mt)
        {
            await ExecAsync(conn, tx, ct,
                "INSERT INTO MthroughputResult (ServerId, AttemptId, RemoteAddr, CapacityDownMbps, CapacityUpMbps, "
                + "ConnsDown, ConnsUp, FairShareSpreadDownPct, FairShareSpreadUpPct) "
                + "VALUES (@pk, @aid, @ra, @cd, @cu, @nd, @nu, @sd, @su)",
                p =>
                {
                    p.AddWithValue("pk", Guid.NewGuid());
                    p.AddWithValue("aid", a.AttemptId);
                    p.AddWithValue("ra", mt.RemoteAddr ?? a.TargetHost);
                    AddNullable(p, "cd", mt.CapacityDownMbps);
                    AddNullable(p, "cu", mt.CapacityUpMbps);
                    p.AddWithValue("nd", mt.ConnsDown);
                    AddNullable(p, "nu", mt.ConnsUp);
                    AddNullable(p, "sd", mt.FairShareSpreadDownPct);
                    AddNullable(p, "su", mt.FairShareSpreadUpPct);
                });
        }
    }

    private static async Task<int> ExecAsync(
        NpgsqlConnection conn, NpgsqlTransaction? tx, CancellationToken ct,
        string sql, Action<NpgsqlParameterCollection> bind)
    {
        await using var cmd = new NpgsqlCommand(sql, conn, tx);
        bind(cmd.Parameters);
        return await cmd.ExecuteNonQueryAsync(ct);
    }

    /// <summary>Attempts actually stored for a run, or <c>null</c> when the probe
    /// schema is absent/unreadable (never throws). Used by
    /// <c>AgentMessageProcessor.OnRunFinished</c> to notice a run that finished
    /// with measured attempts none of which reached the database.</summary>
    public static async Task<int?> CountForRunAsync(
        NpgsqlConnection conn, Guid runId, CancellationToken ct)
    {
        try
        {
            if (conn.State != System.Data.ConnectionState.Open)
            {
                await conn.OpenAsync(ct);
            }

            await using var cmd = new NpgsqlCommand(
                "SELECT COUNT(*) FROM RequestAttempt WHERE RunId = @run", conn);
            cmd.Parameters.AddWithValue("run", runId);
            var scalar = await cmd.ExecuteScalarAsync(ct);
            return scalar is null or DBNull ? null : Convert.ToInt32(scalar, CultureInfo.InvariantCulture);
        }
        catch (Exception)
        {
            return null; // missing schema / no privilege / not Postgres - not a run failure
        }
    }

    private static void AddNullable(NpgsqlParameterCollection p, string name, object? value) =>
        p.AddWithValue(name, value ?? DBNull.Value);
}
