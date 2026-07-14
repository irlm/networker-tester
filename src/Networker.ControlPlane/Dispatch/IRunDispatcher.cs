using Networker.ControlPlane.Auth;

namespace Networker.ControlPlane.Dispatch;

/// <summary>
/// The write-path run dispatcher — the C# re-architecture of the Rust launch +
/// dispatch flow spread across
/// <c>crates/networker-dashboard/src/api/test_configs.rs</c> (<c>launch_handler</c>),
/// <c>crates/networker-dashboard/src/provisioning.rs</c>
/// (<c>dispatch_or_provision</c> / <c>try_dispatch_run</c>),
/// <c>crates/networker-dashboard/src/api/test_runs.rs</c> (<c>cancel_handler</c>),
/// and <c>crates/networker-dashboard/src/scheduler.rs</c>
/// (<c>redispatch_queued_runs</c>).
///
/// <para>This is the single seam the write endpoints (and, in M3 slice 2, the
/// scheduler <c>BackgroundService</c>) call to create runs, hand them to an
/// online agent, and cancel them. It owns the "create a queued run, then
/// best-effort assign it to an agent" contract; if no compatible agent is
/// online the run simply stays <c>queued</c> and <see cref="RedispatchQueuedAsync"/>
/// retries it later.</para>
///
/// <para><b>Deferred to M4:</b> the <c>Pending</c>-endpoint provisioning path.
/// The Rust <c>kick_provisioning</c> starts a deployment and flips the run to
/// <c>provisioning</c>. Here we recognise a Pending endpoint, leave the run
/// <c>queued</c> (annotating the log), and return — the provisioning
/// orchestrator lands in M4.</para>
/// </summary>
public interface IRunDispatcher
{
    /// <summary>
    /// Create a <c>test_run</c> (status <c>queued</c>, project taken from the
    /// config) for <paramref name="testConfigId"/>, then best-effort dispatch it.
    /// Mirrors the Rust <c>launch_handler</c>. Returns the new run id.
    /// </summary>
    /// <param name="comparisonGroupId">Optional comparison-group id to stamp on
    /// the run (used when launching a set of runs to compare).</param>
    /// <param name="testerId">Optional executing-agent affinity: seeds
    /// <c>test_run.tester_id</c> (which semantically holds an AGENT id — see the
    /// Rust agent_hub's <c>WHERE tester_id=$1</c> binding) so dispatch prefers
    /// that agent. Threaded from <c>LaunchRequest.tester_id</c>.</param>
    Task<Guid> LaunchAsync(
        Guid testConfigId,
        Guid? comparisonGroupId,
        Guid? testerId,
        AuthUser caller,
        CancellationToken ct);

    /// <summary>
    /// Load the run + its config and, unless the config's endpoint is
    /// <c>Pending</c> (deferred to M4), assign it to a target agent. Target
    /// selection mirrors the Rust preference order: the run's tester's own agent
    /// if it is online, otherwise any online agent. If no agent is online the run
    /// is left <c>queued</c> for <see cref="RedispatchQueuedAsync"/> to retry.
    /// </summary>
    Task DispatchAsync(Guid runId, CancellationToken ct);

    /// <summary>
    /// Re-dispatch runs still stuck in <c>queued</c> that now have an online
    /// agent — the C# port of the Rust <c>redispatch_queued_runs</c> scheduler
    /// tick. Skips <c>Pending</c>-endpoint runs (M4 owns them). Returns the count
    /// of runs successfully handed to an agent this pass. The
    /// <c>BackgroundService</c> that periodically calls this is M3 slice 2.
    /// </summary>
    Task<int> RedispatchQueuedAsync(CancellationToken ct);

    /// <summary>
    /// Set the run's status to <c>cancelled</c>, send <c>CancelRun</c> to the
    /// owning/any online agent, and publish a <c>JobUpdate</c>. Mirrors the Rust
    /// <c>cancel_handler</c>.
    /// </summary>
    Task CancelAsync(Guid runId, CancellationToken ct);
}
