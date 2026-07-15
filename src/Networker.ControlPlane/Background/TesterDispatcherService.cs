using Networker.ControlPlane.Provisioning;
using Npgsql;

namespace Networker.ControlPlane.Background;

/// <summary>
/// Periodic queued-benchmark promotion loop — the C# port of the Rust
/// <c>tester_dispatcher::sweep_loop</c>
/// (<c>crates/networker-dashboard/src/services/tester_dispatcher.rs</c>). Every
/// ~30s it promotes the oldest queued benchmark on each running+idle tester to
/// <c>pending</c> (see <see cref="TesterDispatcher"/> for the SQL).
///
/// <para>Follows the established background-service pattern: <see cref="PeriodicTimer"/>,
/// per-tick leader election via <see cref="PgAdvisoryLeaderLock"/>, and
/// <see cref="TickMonitor"/> reporting. The Rust ticker uses missed-tick =
/// Delay + immediate first tick; <see cref="PeriodicTimer"/> also delays missed
/// ticks (it never bursts), and we run one sweep before the first wait to match
/// Rust's immediate first tick.</para>
///
/// <para><b>Schema caveat:</b> this loop reads/writes the legacy
/// <c>benchmark_config</c> table (queued-benchmark FIFO). The unified C# schema
/// folded Job+BenchmarkConfig into <c>TestConfig</c>/<c>TestRun</c>, so this
/// service is only meaningful where the legacy <c>benchmark_config</c> table is
/// present. It is therefore NOT auto-registered by the standard scheduler wiring;
/// register it explicitly via <see cref="TesterLoopsExtensions.AddTesterLoops"/>
/// only when that table exists.</para>
/// </summary>
public sealed class TesterDispatcherService : BackgroundService
{
    private const string ServiceName = "tester-dispatcher";

    private readonly NpgsqlDataSource _dataSource;
    private readonly ILogger<TesterDispatcherService> _logger;
    private readonly PgAdvisoryLeaderLock? _leader;
    private readonly TickMonitor _monitor;
    private readonly long _lockKey = LeaderLockKeys.KeyFor(ServiceName);

    public TesterDispatcherService(
        NpgsqlDataSource dataSource,
        ILogger<TesterDispatcherService> logger,
        PgAdvisoryLeaderLock? leaderLock = null,
        TickMonitor? tickMonitor = null)
    {
        _dataSource = dataSource;
        _logger = logger;
        _leader = leaderLock;
        _monitor = tickMonitor ?? new TickMonitor();
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        _logger.LogInformation(
            "tester dispatcher service started (tick every {Seconds}s)",
            TesterDispatcher.SweepInterval.TotalSeconds);
        _monitor.ReportStarted(ServiceName);

        // Immediate first sweep (Rust ticker's first tick fires immediately).
        await RunGuardedTickAsync(stoppingToken).ConfigureAwait(false);

        using var timer = new PeriodicTimer(TesterDispatcher.SweepInterval);
        while (await timer.WaitForNextTickAsync(stoppingToken).ConfigureAwait(false))
        {
            await RunGuardedTickAsync(stoppingToken).ConfigureAwait(false);
        }
    }

    private async Task RunGuardedTickAsync(CancellationToken ct)
    {
        try
        {
            await _leader.TryRunGuardedAsync(_lockKey, TickAsync, ct).ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            // shutting down
        }
        catch (Exception ex)
        {
            _monitor.ReportError(ServiceName, ex);
            _logger.LogWarning(ex, "tester dispatcher sweep failed");
        }
    }

    private async Task TickAsync(CancellationToken ct)
    {
        await using var conn = await _dataSource.OpenConnectionAsync(ct).ConfigureAwait(false);
        var promoted = await TesterDispatcher.SweepTickAsync(conn, _logger, ct).ConfigureAwait(false);
        _monitor.ReportTick(ServiceName, promoted);
    }
}
