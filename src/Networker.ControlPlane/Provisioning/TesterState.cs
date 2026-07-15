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
/// <para><b>Wiring:</b> this is a pure helper (static, connection-in). A later
/// pass calls it from the dispatch / recovery paths. It needs a live Postgres
/// connection with the legacy tester tables present.</para>
/// </summary>
public static class TesterState
{
    /// <summary>Outcome of a lock-acquire attempt — the C# port of Rust
    /// <c>AcquireOutcome</c>.</summary>
    public abstract record AcquireOutcome
    {
        /// <summary>Lock acquired (tester was running + idle + unlocked).</summary>
        public sealed record Acquired : AcquireOutcome;

        /// <summary>Tester is stopped — caller must start it first.</summary>
        public sealed record NeedsStart : AcquireOutcome;

        /// <summary>Tester is in a transient power state (the state string).</summary>
        public sealed record Transient(string PowerState) : AcquireOutcome;

        /// <summary>Tester is mid-upgrade.</summary>
        public sealed record Upgrading : AcquireOutcome;

        /// <summary>Already locked by another config (its id, or nil).</summary>
        public sealed record AlreadyLockedBy(Guid ConfigId) : AcquireOutcome;

        /// <summary>Tester is in the error power state.</summary>
        public sealed record Errored : AcquireOutcome;

        /// <summary>Some other non-idle state, formatted "{power}/{alloc}".</summary>
        public sealed record NotIdle(string Detail) : AcquireOutcome;

        /// <summary>Tester row was deleted mid-acquire — terminal failure.</summary>
        public sealed record Gone : AcquireOutcome;
    }

    /// <summary>
    /// Rust <c>try_acquire</c>: attempt the guarded single-row lock, else classify
    /// why it failed. Never throws for a normal "couldn't lock" case — returns the
    /// classified <see cref="AcquireOutcome"/>.
    /// </summary>
    public static async Task<AcquireOutcome> TryAcquireAsync(
        NpgsqlConnection conn, Guid testerId, Guid configId, CancellationToken ct = default)
    {
        // Step 1 — guarded lock (running + idle + unlocked).
        await using (var cmd = conn.CreateCommand())
        {
            cmd.CommandText = """
                UPDATE project_tester
                   SET allocation          = 'locked',
                       locked_by_config_id = @config,
                       last_used_at        = NOW(),
                       updated_at          = NOW()
                 WHERE tester_id           = @tester
                   AND power_state         = 'running'
                   AND allocation          = 'idle'
                   AND locked_by_config_id IS NULL
                 RETURNING tester_id
                """;
            cmd.Parameters.AddWithValue("config", configId);
            cmd.Parameters.AddWithValue("tester", testerId);
            var acquired = await cmd.ExecuteScalarAsync(ct).ConfigureAwait(false);
            if (acquired is not null)
            {
                return new AcquireOutcome.Acquired();
            }
        }

        // Step 2 — classify (query_opt: the row may have been deleted).
        await using (var cmd = conn.CreateCommand())
        {
            cmd.CommandText =
                "SELECT power_state, allocation, locked_by_config_id FROM project_tester WHERE tester_id = @tester";
            cmd.Parameters.AddWithValue("tester", testerId);
            await using var reader = await cmd.ExecuteReaderAsync(ct).ConfigureAwait(false);
            if (!await reader.ReadAsync(ct).ConfigureAwait(false))
            {
                return new AcquireOutcome.Gone();
            }

            var power = reader.GetString(0);
            var alloc = reader.GetString(1);
            var locker = reader.IsDBNull(2) ? (Guid?)null : reader.GetGuid(2);

            return (power, alloc) switch
            {
                ("stopped", _) => new AcquireOutcome.NeedsStart(),
                ("starting" or "stopping" or "provisioning", _) => new AcquireOutcome.Transient(power),
                ("running", "locked") => new AcquireOutcome.AlreadyLockedBy(locker ?? Guid.Empty),
                ("running", "upgrading") => new AcquireOutcome.Upgrading(),
                ("error", _) => new AcquireOutcome.Errored(),
                _ => new AcquireOutcome.NotIdle($"{power}/{alloc}"),
            };
        }
    }

    /// <summary>
    /// Rust <c>release</c>: the guarded unlock — clears the pair only when the
    /// holder matches <paramref name="configId"/>.
    /// </summary>
    public static async Task ReleaseAsync(
        NpgsqlConnection conn, Guid testerId, Guid configId, CancellationToken ct = default)
    {
        await using var cmd = conn.CreateCommand();
        cmd.CommandText = """
            UPDATE project_tester
               SET allocation          = 'idle',
                   locked_by_config_id = NULL,
                   updated_at          = NOW()
             WHERE tester_id           = @tester
               AND locked_by_config_id = @config
            """;
        cmd.Parameters.AddWithValue("tester", testerId);
        cmd.Parameters.AddWithValue("config", configId);
        await cmd.ExecuteNonQueryAsync(ct).ConfigureAwait(false);
    }

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
