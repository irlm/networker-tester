using Npgsql;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// Crash-recovery logic — the C# port of Rust
/// <c>crates/networker-dashboard/src/services/tester_recovery.rs</c>. After a
/// control-plane restart it force-releases locks held by finished benchmarks and
/// resolves testers wedged in a transient power state (RR-017 self-healing).
///
/// <para>SQL copied verbatim. Raw SQL on <see cref="NpgsqlConnection"/> against
/// the legacy <c>project_tester</c> / <c>benchmark_config</c> / <c>cloud_connection</c>
/// tables. The <b>cloud probe</b> (querying Azure for the real VM power state)
/// is a host-side side effect the control plane can't do in CI — it is stubbed
/// with <c>// TODO(phase3)</c> and the auto-probe branch treats a null probe
/// result the same as Rust's "probe failed" arm (marks the tester
/// <c>error</c>). The DB decisions (which rows to scan, which SQL to run) are
/// ported faithfully.</para>
/// </summary>
public static class TesterRecovery
{
    /// <summary>Rust <c>STARTUP_GRACE</c> = 5 minutes.</summary>
    public static readonly TimeSpan StartupGrace = TimeSpan.FromMinutes(5);

    /// <summary>Rust <c>SWEEP_INTERVAL</c> = 10 minutes.</summary>
    public static readonly TimeSpan SweepInterval = TimeSpan.FromMinutes(10);

    /// <summary>Rust <c>STUCK_THRESHOLD_MINUTES</c> = 30.</summary>
    public const int StuckThresholdMinutes = 30;

    /// <summary>
    /// A single cloud-state probe delegate. Returns the Azure power-state string
    /// for a tester, or null if the probe could not be performed (missing
    /// provider / cloud access — the control-plane CI case). Injected so the real
    /// probe (host-side) can be supplied later without changing the recovery
    /// logic. Rust's <c>probe_azure_state</c> lives behind this seam.
    /// </summary>
    public delegate Task<string?> ProbeCloudState(
        Guid? cloudConnectionId, string? vmResourceId, string? vmName, CancellationToken ct);

    /// <summary>
    /// The default probe used by the control plane: no cloud access, so it
    /// returns null (Rust's "failed to load cloud provider" / probe-error arm,
    /// which marks the tester <c>error</c>).
    /// </summary>
    // TODO(phase3): supply a real host-side Azure probe (get_vm_state) here.
    public static readonly ProbeCloudState NoCloudProbe =
        (_, _, _, _) => Task.FromResult<string?>(null);

    /// <summary>
    /// Rust <c>scan</c>: force-release stuck locks, then handle stuck transients.
    /// Returns (locks_released, transients_handled).
    /// </summary>
    public static async Task<(int LocksReleased, int TransientsHandled)> ScanAsync(
        NpgsqlConnection conn,
        ProbeCloudState probe,
        ILogger? logger = null,
        CancellationToken ct = default)
    {
        var locks = await ForceReleaseStuckLocksAsync(conn, logger, ct).ConfigureAwait(false);
        var stucks = await HandleStuckTransientsAsync(conn, probe, logger, ct).ConfigureAwait(false);
        return (locks, stucks);
    }

    /// <summary>
    /// Rust <c>force_release_stuck_locks</c>: release locks held by benchmarks in
    /// a terminal status, then attempt a promote on each freed tester.
    /// </summary>
    public static async Task<int> ForceReleaseStuckLocksAsync(
        NpgsqlConnection conn, ILogger? logger = null, CancellationToken ct = default)
    {
        var rows = new List<(Guid TesterId, string ProjectId, string Name, string PriorStatus)>();
        await using (var cmd = conn.CreateCommand())
        {
            cmd.CommandText = """
                SELECT t.tester_id, t.project_id, t.name, c.status
                  FROM project_tester t
                  JOIN benchmark_config c ON c.config_id = t.locked_by_config_id
                 WHERE t.allocation = 'locked'
                   AND c.status IN ('completed','completed_with_errors','failed','cancelled')
                """;
            await using var reader = await cmd.ExecuteReaderAsync(ct).ConfigureAwait(false);
            while (await reader.ReadAsync(ct).ConfigureAwait(false))
            {
                rows.Add((reader.GetGuid(0), reader.GetString(1), reader.GetString(2), reader.GetString(3)));
            }
        }

        var count = 0;
        foreach (var (testerId, projectId, name, priorStatus) in rows)
        {
            ct.ThrowIfCancellationRequested();
            try
            {
                await TesterState.ForceReleaseAsync(conn, testerId, ct).ConfigureAwait(false);
                logger?.LogInformation(
                    "force-released stuck lock tester_id={TesterId} project_id={ProjectId} name={Name} prior_holder_status={PriorStatus}",
                    testerId, projectId, name, priorStatus);
                count++;

                try
                {
                    await TesterDispatcher.PromoteNextAsync(conn, testerId, ct).ConfigureAwait(false);
                }
                catch (Exception ex)
                {
                    logger?.LogWarning(ex, "promote_next after force_release failed tester_id={TesterId}", testerId);
                }
            }
            catch (Exception ex)
            {
                logger?.LogWarning(ex, "force_release failed tester_id={TesterId}", testerId);
            }
        }

        return count;
    }

    /// <summary>
    /// Rust <c>handle_stuck_transients</c>: resolve testers stuck in a transient
    /// power state older than the stuck threshold. Auto-probe-enabled testers are
    /// probed against the cloud (stubbed here → error arm); disabled ones are
    /// marked <c>error</c> for manual recovery.
    /// </summary>
    public static async Task<int> HandleStuckTransientsAsync(
        NpgsqlConnection conn,
        ProbeCloudState probe,
        ILogger? logger = null,
        CancellationToken ct = default)
    {
        var rows = new List<StuckRow>();
        await using (var cmd = conn.CreateCommand())
        {
            cmd.CommandText = $"""
                SELECT tester_id, project_id, name, power_state, auto_probe_enabled,
                       vm_name, vm_resource_id, cloud_connection_id
                  FROM project_tester
                 WHERE power_state IN ('starting','stopping','upgrading','provisioning')
                   AND updated_at < NOW() - INTERVAL '{StuckThresholdMinutes} minutes'
                """;
            await using var reader = await cmd.ExecuteReaderAsync(ct).ConfigureAwait(false);
            while (await reader.ReadAsync(ct).ConfigureAwait(false))
            {
                rows.Add(new StuckRow(
                    reader.GetGuid(0),
                    reader.GetString(1),
                    reader.GetString(2),
                    reader.GetString(3),
                    reader.GetBoolean(4),
                    reader.IsDBNull(5) ? null : reader.GetString(5),
                    reader.IsDBNull(6) ? null : reader.GetString(6),
                    reader.IsDBNull(7) ? (Guid?)null : reader.GetGuid(7)));
            }
        }

        var count = 0;
        foreach (var row in rows)
        {
            ct.ThrowIfCancellationRequested();

            if (row.AutoProbeEnabled)
            {
                string? azureState;
                try
                {
                    azureState = await probe(row.CloudConnectionId, row.VmResourceId, row.VmName, ct)
                        .ConfigureAwait(false);
                }
                catch (Exception ex)
                {
                    // Rust: probe error → mark error "Auto-probe failed after restart: {e}".
                    await UpdateStateAsync(conn, row.TesterId, "error",
                        $"Auto-probe failed after restart: {ex.Message}", ct).ConfigureAwait(false);
                    logger?.LogWarning(ex,
                        "auto-probe failed; marked error tester_id={TesterId}", row.TesterId);
                    count++;
                    continue;
                }

                if (azureState is null)
                {
                    // Control-plane stub / no cloud provider: Rust's "failed to load
                    // cloud provider; marked error" arm.
                    await UpdateStateAsync(conn, row.TesterId, "error",
                        "Failed to load cloud provider: probe unavailable in control plane", ct)
                        .ConfigureAwait(false);
                    logger?.LogWarning(
                        "failed to load cloud provider; marked error tester_id={TesterId}", row.TesterId);
                    count++;
                    continue;
                }

                var resolved = TesterState.AzurePowerToRow(azureState);
                await UpdateStateAsync(conn, row.TesterId, resolved,
                    $"Auto-probed after restart: Azure reported {azureState}", ct).ConfigureAwait(false);
                logger?.LogInformation(
                    "stuck transient auto-probed tester_id={TesterId} previous={Previous} azure_state={AzureState} resolved={Resolved}",
                    row.TesterId, row.PowerState, azureState, resolved);
                count++;
            }
            else
            {
                // Rust: auto-probe disabled → mark error (note the em-dash).
                await UpdateStateAsync(conn, row.TesterId, "error",
                    $"Stuck in {row.PowerState} after dashboard restart — needs manual recovery (auto-probe disabled)",
                    ct).ConfigureAwait(false);
                logger?.LogWarning(
                    "stuck transient marked error (auto-probe disabled) tester_id={TesterId}", row.TesterId);
                count++;
            }
        }

        return count;
    }

    private static async Task UpdateStateAsync(
        NpgsqlConnection conn, Guid testerId, string powerState, string statusMessage, CancellationToken ct)
    {
        await using var cmd = conn.CreateCommand();
        cmd.CommandText =
            "UPDATE project_tester SET power_state = @state, status_message = @msg, updated_at = NOW() WHERE tester_id = @tester";
        cmd.Parameters.AddWithValue("state", powerState);
        cmd.Parameters.AddWithValue("msg", statusMessage);
        cmd.Parameters.AddWithValue("tester", testerId);
        await cmd.ExecuteNonQueryAsync(ct).ConfigureAwait(false);
    }

    private sealed record StuckRow(
        Guid TesterId,
        string ProjectId,
        string Name,
        string PowerState,
        bool AutoProbeEnabled,
        string? VmName,
        string? VmResourceId,
        Guid? CloudConnectionId);
}
