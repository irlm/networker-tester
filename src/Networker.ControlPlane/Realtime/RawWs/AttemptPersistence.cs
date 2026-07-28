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
    public static async Task PersistAsync(
        NpgsqlConnection conn, ParsedAttempt a, CancellationToken ct)
    {
        if (conn.State != System.Data.ConnectionState.Open)
        {
            await conn.OpenAsync(ct);
        }

        await using var tx = await conn.BeginTransactionAsync(ct);
        try
        {
            // RequestAttempt.RunId FKs to the tester-owned V001 `testrun` table
            // (runid PK) — a DIFFERENT table from the control plane's `test_run`.
            // DB-backed testers used to create it; the C# agent path never does,
            // so the FK would fail (E2E dry-run finding). Upsert a minimal row
            // (only runid/targeturl/targethost are NOT NULL) — nothing reads its
            // target columns (the URL-test history joins on RunId only), so a
            // best-effort host/url from the attempt is sufficient.
            await ExecAsync(conn, tx, ct,
                "INSERT INTO testrun (runid, targeturl, targethost) VALUES (@run, @url, @host) "
                + "ON CONFLICT (runid) DO NOTHING",
                p =>
                {
                    p.AddWithValue("run", a.RunId);
                    p.AddWithValue("url", a.TargetUrl);
                    p.AddWithValue("host", a.TargetHost);
                });

            // Idempotent on AttemptId: a re-delivered attempt_event must not
            // duplicate rows. Only when the RequestAttempt is NEW do we write
            // its phase rows (their PKs are fresh uuids, so a conflict-skip on
            // the parent is the guard).
            var inserted = await ExecAsync(conn, tx, ct,
                """
                INSERT INTO RequestAttempt
                    (AttemptId, RunId, Protocol, SequenceNum, StartedAt, FinishedAt,
                     Success, ErrorMessage, RetryCount, extrajson)
                VALUES (@id, @run, @proto, @seq, @started, @finished, @ok, @err, @retry, @extra)
                ON CONFLICT (AttemptId) DO NOTHING
                """,
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
                    p.Add(new NpgsqlParameter("extra", NpgsqlDbType.Jsonb)
                    { Value = (object?)a.ExtraJson ?? DBNull.Value });
                });

            if (inserted > 0)
            {
                await WritePhasesAsync(conn, tx, a, ct);
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

    private static async Task WritePhasesAsync(
        NpgsqlConnection conn, NpgsqlTransaction tx, ParsedAttempt a, CancellationToken ct)
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
            await ExecAsync(conn, tx, ct,
                "INSERT INTO ServerTimingResult (ServerId, AttemptId, RecvBodyMs, ProcessingMs, TotalServerMs) "
                + "VALUES (@pk, @aid, @recv, @proc, @total)",
                p =>
                {
                    p.AddWithValue("pk", Guid.NewGuid());
                    p.AddWithValue("aid", a.AttemptId);
                    AddNullable(p, "recv", st.RecvBodyMs);
                    AddNullable(p, "proc", st.ProcessingMs);
                    AddNullable(p, "total", st.TotalServerMs);
                });
        }
    }

    private static async Task<int> ExecAsync(
        NpgsqlConnection conn, NpgsqlTransaction tx, CancellationToken ct,
        string sql, Action<NpgsqlParameterCollection> bind)
    {
        await using var cmd = new NpgsqlCommand(sql, conn, tx);
        bind(cmd.Parameters);
        return await cmd.ExecuteNonQueryAsync(ct);
    }

    private static void AddNullable(NpgsqlParameterCollection p, string name, object? value) =>
        p.AddWithValue(name, value ?? DBNull.Value);
}
