using Npgsql;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// The single authoritative writer of the tester
/// <c>(allocation, locked_by_config_id)</c> pair — the C# port of Rust
/// <c>crates/networker-dashboard/src/services/tester_state.rs</c>.
///
/// <para>Every SQL statement is copied verbatim from the Rust source (same
/// columns, same WHERE clauses, same <c>NOW()</c> touches). Ported as raw SQL on
/// <see cref="NpgsqlConnection"/> — the Rust code is itself raw <c>tokio_postgres</c>
/// SQL, and these statements read/write the legacy <c>project_tester</c> /
/// <c>benchmark_config</c> tables (columns like <c>allocation</c>,
/// <c>locked_by_config_id</c>, <c>power_state</c>) that the EF entity model does
/// not fully expose. Keeping raw SQL preserves the exact locking semantics
/// (single-row guarded <c>UPDATE ... RETURNING</c>, compare-and-set transitions).</para>
///
/// <para><b>Wiring:</b> this is a pure helper (static, connection-in), called
/// from the create/recovery paths. It needs a live Postgres connection with the
/// legacy tester tables present. (The Rust <c>try_acquire</c>/<c>release</c>
/// lock pair was not carried over: the C# dispatcher gates on
/// <c>allocation = 'idle'</c> in its pick query and never takes the row lock.)</para>
/// </summary>
public static class TesterState
{
    /// <summary>
    /// Rust <c>try_power_transition</c>: compare-and-set on <c>power_state</c>.
    /// Returns true iff exactly one row updated.
    /// </summary>
    public static async Task<bool> TryPowerTransitionAsync(
        NpgsqlConnection conn, Guid testerId, string expected, string next, CancellationToken ct = default)
    {
        await using var cmd = conn.CreateCommand();
        cmd.CommandText = """
            UPDATE project_tester
               SET power_state = @next,
                   updated_at  = NOW()
             WHERE tester_id   = @tester
               AND power_state = @expected
            """;
        cmd.Parameters.AddWithValue("next", next);
        cmd.Parameters.AddWithValue("tester", testerId);
        cmd.Parameters.AddWithValue("expected", expected);
        var rows = await cmd.ExecuteNonQueryAsync(ct).ConfigureAwait(false);
        return rows == 1;
    }

    /// <summary>Rust <c>set_status_message</c>.</summary>
    public static async Task SetStatusMessageAsync(
        NpgsqlConnection conn, Guid testerId, string message, CancellationToken ct = default)
    {
        await using var cmd = conn.CreateCommand();
        cmd.CommandText =
            "UPDATE project_tester SET status_message = @msg, updated_at = NOW() WHERE tester_id = @tester";
        cmd.Parameters.AddWithValue("msg", message);
        cmd.Parameters.AddWithValue("tester", testerId);
        await cmd.ExecuteNonQueryAsync(ct).ConfigureAwait(false);
    }

    /// <summary>
    /// Rust <c>force_release</c>: unconditional unlock — recovery-loop use only.
    /// </summary>
    public static async Task ForceReleaseAsync(
        NpgsqlConnection conn, Guid testerId, CancellationToken ct = default)
    {
        await using var cmd = conn.CreateCommand();
        cmd.CommandText = """
            UPDATE project_tester
               SET allocation          = 'idle',
                   locked_by_config_id = NULL,
                   updated_at          = NOW()
             WHERE tester_id           = @tester
            """;
        cmd.Parameters.AddWithValue("tester", testerId);
        await cmd.ExecuteNonQueryAsync(ct).ConfigureAwait(false);
    }

    /// <summary>
    /// Rust <c>azure_power_to_row</c>: map an Azure power-state string to a
    /// <c>project_tester.power_state</c> value (case-insensitive, ordered checks).
    /// </summary>
    public static string AzurePowerToRow(string azureState)
    {
        var s = azureState.ToLowerInvariant();
        if (s.Contains("running"))
        {
            return "running";
        }

        if (s.Contains("deallocated") || s.Contains("stopped"))
        {
            return "stopped";
        }

        if (s.Contains("starting"))
        {
            return "starting";
        }

        if (s.Contains("stopping") || s.Contains("deallocating"))
        {
            return "stopping";
        }

        return "error";
    }
}
