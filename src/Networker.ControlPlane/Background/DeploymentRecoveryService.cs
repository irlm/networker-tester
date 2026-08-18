using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Provisioning;
using Networker.Data;

namespace Networker.ControlPlane.Background;

/// <summary>
/// One-shot startup reconciliation for deployments whose in-flight
/// <c>install.sh</c> was killed by a control-plane restart (issue #764).
///
/// <para><b>Why:</b> the deploy shell-out is a detached in-process
/// <c>Task.Run</c> child of this process, so every release restart SIGTERMs any
/// in-flight endpoint deployment (install.sh exit 143). Two shapes result:</para>
/// <list type="bullet">
///   <item><b>Graceful restart</b> — the dying process's
///     <see cref="DeployRunner"/> persists the row as <c>failed</c> with the
///     <see cref="ProvisioningFailureClassifier.InterruptedErrorPrefix"/>
///     message before exiting.</item>
///   <item><b>Crash</b> (SIGKILL/OOM/power) — the row wedges at
///     <c>pending</c>/<c>running</c> forever; only the watchdog's 30-minute
///     stale sweep used to fail it.</item>
/// </list>
///
/// <para>On startup — after migrations, before the orchestrator's first tick
/// (2s delay + 5s cadence) can terminally fail a linked run — this service
/// claims both shapes and re-runs each deployment's stored config through the
/// same <see cref="DeployRunner"/> path. install.sh is idempotent (the manual
/// Retry button, #766, relies on the same property), and flipping the row back
/// to <c>pending</c> also restores the orphan reaper's vm-name guard so the
/// half-created VM survives until the re-run completes. A linked run still in
/// <c>provisioning</c> simply keeps waiting and promotes when the re-run
/// completes.</para>
///
/// <para><b>Safety rails:</b> <c>deployment.recovery_attempts</c> (V052) caps the
/// automatic loop at <see cref="MaxRecoveryAttempts"/> (a deployment that keeps
/// getting interrupted stays failed; Retry remains the manual path); the
/// interrupted-failed shape is only recovered within
/// <see cref="InterruptedRetryWindow"/> of its failure (a restart follows the
/// interruption within seconds — old rows are history, not work); and a
/// deployment whose only linked run(s) are already terminal is left alone
/// (re-running it would provision a VM nothing consumes, which the teardown
/// phase would then race to delete). Every claim is an atomic status-guarded
/// update, so a concurrent writer loses cleanly.</para>
///
/// <para>Registered behind the same gate as the provisioning orchestrator —
/// API-only replicas neither run installs nor recover them.</para>
/// </summary>
public sealed class DeploymentRecoveryService : IHostedService
{
    /// <summary>Max automatic re-runs per deployment. Consecutive releases in a
    /// short window can interrupt the SAME deployment more than once, so one
    /// retry is not enough; past the cap the row stays failed and the UI Retry
    /// button (#766) is the manual path.</summary>
    internal const short MaxRecoveryAttempts = 3;

    /// <summary>How recent an interrupted-failed row must be to auto-retry. A
    /// restart follows the interruption within seconds (systemd swaps the unit),
    /// so anything older than this predates the previous shutdown and is
    /// history the user has already seen — not in-flight work. Static (not
    /// const) so tests can shrink it.</summary>
    internal static TimeSpan InterruptedRetryWindow = TimeSpan.FromMinutes(30);

    private const string StatusPending = "pending";
    private const string StatusRunning = "running";
    private const string StatusFailed = "failed";
    private const string RunProvisioning = "provisioning";

    private readonly IServiceScopeFactory _scopeFactory;
    private readonly DeployRunner _runner;
    private readonly ILogger<DeploymentRecoveryService> _logger;

    public DeploymentRecoveryService(
        IServiceScopeFactory scopeFactory,
        DeployRunner runner,
        ILogger<DeploymentRecoveryService> logger)
    {
        _scopeFactory = scopeFactory;
        _runner = runner;
        _logger = logger;
    }

    /// <summary>A deployment the recovery pass claimed for a re-run.</summary>
    internal sealed record RecoveredDeployment(
        Guid DeploymentId, string Config, string PriorStatus, short Attempt);

    public async Task StartAsync(CancellationToken cancellationToken)
    {
        // Awaited during host start: the claim pass must finish before the
        // provisioning orchestrator's first tick (2s startup delay + 5s cadence)
        // can see a still-`failed` deployment and terminally fail its run. The
        // pass is a handful of small queries — startup cost is negligible.
        List<RecoveredDeployment> recovered;
        try
        {
            using var scope = _scopeFactory.CreateScope();
            var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
            recovered = await RecoverAsync(db, _logger, cancellationToken).ConfigureAwait(false);
        }
        catch (Exception ex)
        {
            // Never block startup on recovery — the watchdog + manual Retry
            // remain the fallback exactly as before this service existed.
            _logger.LogError(ex, "Deployment recovery pass failed — interrupted deployments stay failed");
            return;
        }

        foreach (var r in recovered)
        {
            SpawnRerun(r);
        }
    }

    public Task StopAsync(CancellationToken cancellationToken) => Task.CompletedTask;

    /// <summary>
    /// The claim pass (separated from the spawn so tests can drive it against a
    /// seeded DbContext): find every recoverable deployment, atomically flip it
    /// back to <c>pending</c> with <c>recovery_attempts + 1</c>, and return the
    /// claimed set for the caller to re-run. <c>started_at</c> is stamped with
    /// the claim time so the watchdog's stale sweep measures THIS attempt, not
    /// the original one.
    /// </summary>
    internal static async Task<List<RecoveredDeployment>> RecoverAsync(
        NetworkerDbContext db, ILogger logger, CancellationToken ct)
    {
        var now = DateTime.UtcNow;
        var interruptedCutoff = now - InterruptedRetryWindow;

        // Two recoverable shapes, one query:
        //  * pending/running — crash leftovers. At process start no in-process
        //    deploy driver can exist yet (hosted services start before any
        //    request or orchestrator tick can spawn one), so every such row is
        //    driverless by construction.
        //  * failed + interrupted marker, recently — the graceful-SIGTERM path.
        var candidates = await db.Deployments
            .Where(d => d.RecoveryAttempts < MaxRecoveryAttempts
                        && (d.Status == StatusPending
                            || d.Status == StatusRunning
                            || (d.Status == StatusFailed
                                && d.ErrorMessage != null
                                && d.ErrorMessage.StartsWith(
                                    ProvisioningFailureClassifier.InterruptedErrorPrefix)
                                && d.FinishedAt != null
                                && d.FinishedAt >= interruptedCutoff)))
            .Select(d => new { d.DeploymentId, d.Status, d.Config, d.RecoveryAttempts })
            .ToListAsync(ct)
            .ConfigureAwait(false);

        var recovered = new List<RecoveredDeployment>();
        foreach (var c in candidates)
        {
            ct.ThrowIfCancellationRequested();

            // A deployment whose linked run(s) all reached a terminal state has
            // no consumer left: re-running it would build a VM the teardown
            // phase immediately races to delete. Leave it be — the row already
            // tells its story. (Wizard deployments have no linked run and are
            // always recoverable.)
            var linkedRunStatuses = await db.TestRuns
                .Where(r => r.ProvisioningDeploymentId == c.DeploymentId)
                .Select(r => r.Status)
                .ToListAsync(ct)
                .ConfigureAwait(false);
            if (linkedRunStatuses.Count > 0 && !linkedRunStatuses.Contains(RunProvisioning))
            {
                logger.LogInformation(
                    "Deployment {DeploymentId} is recoverable ({Status}) but its linked run is no longer provisioning — skipping re-run",
                    c.DeploymentId, c.Status);
                continue;
            }

            var attempt = (short)(c.RecoveryAttempts + 1);
            var claimed = await db.Deployments
                .Where(d => d.DeploymentId == c.DeploymentId
                            && d.Status == c.Status
                            && d.RecoveryAttempts == c.RecoveryAttempts)
                .ExecuteUpdateAsync(s => s
                    .SetProperty(d => d.Status, StatusPending)
                    .SetProperty(d => d.RecoveryAttempts, attempt)
                    .SetProperty(d => d.ErrorMessage, (string?)null)
                    .SetProperty(d => d.FinishedAt, (DateTime?)null)
                    .SetProperty(d => d.StartedAt, now), ct)
                .ConfigureAwait(false);
            if (claimed == 0)
            {
                continue; // a concurrent writer got there first
            }

            logger.LogWarning(
                "Recovering deployment {DeploymentId} interrupted by a control-plane restart (was {PriorStatus}) — re-running install (attempt {Attempt}/{Max})",
                c.DeploymentId, c.Status, attempt, MaxRecoveryAttempts);
            recovered.Add(new RecoveredDeployment(c.DeploymentId, c.Config, c.Status, attempt));
        }

        return recovered;
    }

    /// <summary>Re-run one recovered deployment on a detached task tied to the
    /// app lifetime — the same contract as the wizard's SpawnDeploy and the
    /// orchestrator's kick (the runner opens its own DI scope and soft-fails
    /// without install.sh).</summary>
    private void SpawnRerun(RecoveredDeployment r)
    {
        _ = Task.Run(async () =>
        {
            try
            {
                await _runner.RunDeploymentAsync(r.DeploymentId, r.Config, CancellationToken.None)
                    .ConfigureAwait(false);
            }
            catch (Exception ex)
            {
                _logger.LogError(ex,
                    "Recovered deployment {DeploymentId} re-run failed (attempt {Attempt}/{Max})",
                    r.DeploymentId, r.Attempt, MaxRecoveryAttempts);
            }
        }, CancellationToken.None);
    }
}
