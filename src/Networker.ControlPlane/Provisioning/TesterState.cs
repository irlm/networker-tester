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
    /// The testers whose <c>provisioning</c> row is being driven by a create-path
    /// task IN THIS PROCESS. Registered for the lifetime of that task, so a
    /// heartbeat-driven reconcile can tell "install still mid-flight" (owner
    /// present) from "the owner is gone" — which is exactly what a control-plane
    /// restart produces: prod left a runner stuck in <c>provisioning</c> forever
    /// after a deploy restarted the process mid-wait, invisible to the UI and to
    /// auto-shutdown (so the VM billed on), even though its agent was online and
    /// heartbeating (prod mode sweep, v0.28.213).
    /// </summary>
    private static readonly System.Collections.Concurrent.ConcurrentDictionary<Guid, byte> ProvisioningOwners = new();

    /// <summary>Mark this process as the owner of a tester's provisioning flow.
    /// Dispose the returned scope when the flow ends (success, failure or throw).</summary>
    public static IDisposable OwnProvisioning(Guid testerId) => new ProvisioningOwnership(testerId);

    /// <summary>Whether a create-path task in this process is currently driving
    /// this tester's provisioning.</summary>
    public static bool IsProvisioningOwnedHere(Guid testerId) => ProvisioningOwners.ContainsKey(testerId);

    private sealed class ProvisioningOwnership : IDisposable
    {
        private readonly Guid _testerId;
        internal ProvisioningOwnership(Guid testerId)
        {
            _testerId = testerId;
            ProvisioningOwners[testerId] = 0;
        }
        public void Dispose() => ProvisioningOwners.TryRemove(_testerId, out _);
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
