using Networker.ControlPlane.Dispatch;

namespace Networker.ControlPlane.Background;

/// <summary>
/// The queued-run redispatcher — the C# port of the Rust scheduler's
/// <c>redispatch_queued_runs</c> sub-routine in
/// <c>crates/networker-dashboard/src/scheduler.rs</c>.
///
/// <para>Every ~30s it asks <see cref="IRunDispatcher.RedispatchQueuedAsync"/> to
/// retry runs still stuck in <c>queued</c> — runs created while no agent was
/// connected, or whose inline dispatch raced a transient WS send failure. The
/// dispatcher owns the candidate query, the min-age / batch-limit bounds, and
/// the <c>Pending</c>-endpoint skip; this service is just the periodic driver.</para>
///
/// <para>Kept as a separate <see cref="BackgroundService"/> (rather than folded
/// into <see cref="SchedulerService"/>) to mirror the Rust design where the
/// redispatch pass runs on its own ~30s cadence independent of the schedule
/// scan, and so a slow schedule fan-out can't delay stuck-run recovery.</para>
/// </summary>
public sealed class QueuedRunRedispatchService : BackgroundService
{
    private static readonly TimeSpan TickInterval = TimeSpan.FromSeconds(30);

    private readonly IServiceScopeFactory _scopeFactory;
    private readonly ILogger<QueuedRunRedispatchService> _logger;

    public QueuedRunRedispatchService(
        IServiceScopeFactory scopeFactory,
        ILogger<QueuedRunRedispatchService> logger)
    {
        _scopeFactory = scopeFactory;
        _logger = logger;
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        _logger.LogInformation(
            "Queued-run redispatch service started (tick every {Seconds}s)",
            TickInterval.TotalSeconds);

        using var timer = new PeriodicTimer(TickInterval);
        while (await timer.WaitForNextTickAsync(stoppingToken).ConfigureAwait(false))
        {
            try
            {
                await TickAsync(stoppingToken).ConfigureAwait(false);
            }
            catch (OperationCanceledException) when (stoppingToken.IsCancellationRequested)
            {
                break;
            }
            catch (Exception ex)
            {
                _logger.LogError(ex, "Queued-run redispatch tick failed");
            }
        }
    }

    private async Task TickAsync(CancellationToken ct)
    {
        // IRunDispatcher is scoped; open a fresh scope per tick.
        using var scope = _scopeFactory.CreateScope();
        var dispatcher = scope.ServiceProvider.GetRequiredService<IRunDispatcher>();

        var count = await dispatcher.RedispatchQueuedAsync(ct).ConfigureAwait(false);
        if (count > 0)
        {
            _logger.LogInformation("Redispatched {Count} previously-queued run(s)", count);
        }
    }
}
