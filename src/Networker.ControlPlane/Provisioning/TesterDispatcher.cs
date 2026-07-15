using Npgsql;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// Queued-benchmark promotion — the C# port of Rust
/// <c>crates/networker-dashboard/src/services/tester_dispatcher.rs</c>. Promotes
/// queued benchmarks to <c>pending</c> on running+idle testers.
///
/// <para>SQL copied verbatim (same FIFO ordering, same
/// <c>FOR UPDATE SKIP LOCKED</c>, same 100-tester candidate limit). Raw SQL on
/// <see cref="NpgsqlConnection"/> against the legacy <c>benchmark_config</c> /
/// <c>project_tester</c> tables (see <see cref="TesterState"/> remarks).</para>
///
/// <para><b>RR-005 invariant (preserved):</b> promotion must NOT clear
/// <c>queued_at</c> — re-queued rows keep FIFO position. The promote SQL below
/// contains no <c>queued_at = NULL</c>.</para>
/// </summary>
public static class TesterDispatcher
{
    /// <summary>Sweep cadence — Rust 30s.</summary>
    public static readonly TimeSpan SweepInterval = TimeSpan.FromSeconds(30);

    /// <summary>
    /// Rust <c>promote_next</c>: atomic per-tester promotion of the oldest queued
    /// benchmark to <c>pending</c>. Returns the promoted config id, or null (empty
    /// queue / lost race).
    /// </summary>
    public static async Task<Guid?> PromoteNextAsync(
        NpgsqlConnection conn, Guid testerId, CancellationToken ct = default)
    {
        await using var cmd = conn.CreateCommand();
        cmd.CommandText = """
            UPDATE benchmark_config
               SET status = 'pending'
             WHERE config_id = (
                 SELECT config_id FROM benchmark_config
                  WHERE tester_id = @tester AND status = 'queued'
                  ORDER BY queued_at ASC NULLS LAST
                  LIMIT 1
                  FOR UPDATE SKIP LOCKED
             )
             RETURNING config_id
            """;
        cmd.Parameters.AddWithValue("tester", testerId);
        var result = await cmd.ExecuteScalarAsync(ct).ConfigureAwait(false);
        return result is Guid g ? g : null;
    }

    /// <summary>
    /// Rust <c>sweep_tick</c>: find candidate running+idle testers with queued
    /// benchmarks, then promote one per tester. Returns the number of promotions.
    /// </summary>
    public static async Task<int> SweepTickAsync(
        NpgsqlConnection conn, ILogger? logger = null, CancellationToken ct = default)
    {
        var candidates = new List<Guid>();
        await using (var cmd = conn.CreateCommand())
        {
            cmd.CommandText = """
                SELECT DISTINCT t.tester_id
                  FROM project_tester t
                  JOIN benchmark_config b ON b.tester_id = t.tester_id
                 WHERE t.power_state = 'running'
                   AND t.allocation  = 'idle'
                   AND b.status      = 'queued'
                 LIMIT 100
                """;
            await using var reader = await cmd.ExecuteReaderAsync(ct).ConfigureAwait(false);
            while (await reader.ReadAsync(ct).ConfigureAwait(false))
            {
                candidates.Add(reader.GetGuid(0));
            }
        }

        logger?.LogDebug("tester dispatcher sweep tick candidates={Candidates}", candidates.Count);

        var promoted = 0;
        foreach (var testerId in candidates)
        {
            ct.ThrowIfCancellationRequested();
            try
            {
                var configId = await PromoteNextAsync(conn, testerId, ct).ConfigureAwait(false);
                if (configId is Guid cid)
                {
                    logger?.LogInformation(
                        "dispatcher promoted queued benchmark tester_id={TesterId} config_id={ConfigId}",
                        testerId, cid);
                    promoted++;
                }

                // Ok(None) => benign race, no log (Rust behavior).
            }
            catch (Exception ex)
            {
                logger?.LogWarning(ex, "promote_next failed tester_id={TesterId}", testerId);
            }
        }

        return promoted;
    }
}
