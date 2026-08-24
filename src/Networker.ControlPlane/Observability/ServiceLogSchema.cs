using Npgsql;

namespace Networker.ControlPlane.Observability;

/// <summary>
/// The <c>service_log</c> DDL, mirroring <c>networker-log/src/schema.rs</c>
/// statement for statement.
///
/// <para>The table is <b>shared</b>: the tester and the endpoint still write it
/// through the Rust crate, and the control plane's <c>/api/logs</c> reads it.
/// So this must produce a table those writers and that reader already agree on
/// — same columns, same types, same level encoding — rather than a C#-shaped
/// table that only this process understands. Every statement is
/// <c>IF NOT EXISTS</c>, so running it against a database the Rust crate
/// already provisioned is a no-op.</para>
/// </summary>
public static class ServiceLogSchema
{
    public const string CreateTable = """
        CREATE TABLE IF NOT EXISTS service_log (
            ts          TIMESTAMPTZ     NOT NULL DEFAULT clock_timestamp(),
            service     TEXT            NOT NULL,
            level       SMALLINT        NOT NULL,
            message     TEXT            NOT NULL,
            config_id   UUID,
            project_id  CHAR(14),
            trace_id    UUID,
            fields      JSONB
        );
        """;

    public static readonly string[] CreateIndexes =
    [
        "CREATE INDEX IF NOT EXISTS service_log_service_ts_idx ON service_log (service, ts DESC);",
        "CREATE INDEX IF NOT EXISTS service_log_level_ts_idx ON service_log (level, ts DESC);",
        """
        CREATE INDEX IF NOT EXISTS service_log_config_id_ts_idx
            ON service_log (config_id, ts DESC) WHERE config_id IS NOT NULL;
        """,
        """
        CREATE INDEX IF NOT EXISTS service_log_project_id_ts_idx
            ON service_log (project_id, ts DESC) WHERE project_id IS NOT NULL;
        """,
        """
        CREATE INDEX IF NOT EXISTS ix_service_log_trace
            ON service_log (trace_id, ts DESC) WHERE trace_id IS NOT NULL;
        """,
        // Best effort, exactly as the Rust crate does it: the search filter on
        // /api/logs is an ILIKE, which is a sequential scan without this.
        """
        DO $$ BEGIN
          IF EXISTS (SELECT 1 FROM pg_available_extensions WHERE name = 'pg_trgm') THEN
            CREATE EXTENSION IF NOT EXISTS pg_trgm;
            CREATE INDEX IF NOT EXISTS ix_service_log_message_trgm
                ON service_log USING GIN (message gin_trgm_ops);
          END IF;
        END $$;
        """,
    ];

    /// <summary>
    /// Create the table and its indexes if absent. Returns true when the table
    /// is present and writable afterwards.
    /// </summary>
    public static async Task EnsureTableAsync(NpgsqlDataSource dataSource, CancellationToken ct = default)
    {
        await using var conn = await dataSource.OpenConnectionAsync(ct);
        await using (var cmd = new NpgsqlCommand(CreateTable, conn))
        {
            await cmd.ExecuteNonQueryAsync(ct);
        }

        foreach (var ddl in CreateIndexes)
        {
            await using var cmd = new NpgsqlCommand(ddl, conn);
            await cmd.ExecuteNonQueryAsync(ct);
        }
    }

    /// <summary>
    /// Upgrade to a TimescaleDB hypertable with 1-day chunks and a retention
    /// policy, when the extension is available. A plain PostgreSQL database is
    /// fine — the table works either way, it just grows unbounded without a
    /// retention policy, which is what <paramref name="retentionDays"/> is for.
    ///
    /// <para>Best-effort by design, like the Rust crate: an already-converted
    /// table, or a Timescale version that refuses the call, must not stop the
    /// control plane from booting. Returns a human-readable outcome for the
    /// startup log rather than throwing.</para>
    /// </summary>
    public static async Task<string> TryEnsureHypertableAsync(
        NpgsqlDataSource dataSource, int retentionDays, CancellationToken ct = default)
    {
        try
        {
            await using var conn = await dataSource.OpenConnectionAsync(ct);

            await using (var probe = new NpgsqlCommand(
                "SELECT 1 FROM pg_extension WHERE extname = 'timescaledb'", conn))
            {
                if (await probe.ExecuteScalarAsync(ct) is null)
                {
                    return retentionDays > 0
                        ? "plain table (TimescaleDB not installed — no retention policy; prune service_log yourself)"
                        : "plain table (TimescaleDB not installed)";
                }
            }

            await using (var hyper = new NpgsqlCommand(
                "SELECT create_hypertable('service_log', 'ts', chunk_time_interval => INTERVAL '1 day', if_not_exists => TRUE)",
                conn))
            {
                await hyper.ExecuteScalarAsync(ct);
            }

            if (retentionDays <= 0)
            {
                return "hypertable (1-day chunks, no retention policy)";
            }

            await using var retention = new NpgsqlCommand(
                $"SELECT add_retention_policy('service_log', INTERVAL '{retentionDays} days', if_not_exists => TRUE)",
                conn);
            await retention.ExecuteScalarAsync(ct);
            return $"hypertable (1-day chunks, {retentionDays}-day retention)";
        }
        catch (PostgresException ex)
        {
            return $"plain table (Timescale upgrade skipped: {ex.SqlState} {ex.MessageText})";
        }
        catch (Exception ex) when (ex is not OperationCanceledException)
        {
            return $"plain table (Timescale upgrade skipped: {ex.GetType().Name})";
        }
    }
}
