using Networker.ControlPlane.Provisioning;
using Npgsql;

namespace Networker.ControlPlane.Background;

/// <summary>
/// Periodic tester crash-recovery loop — the C# port of the Rust
/// <c>tester_recovery::recover_on_startup</c>
/// (<c>crates/networker-dashboard/src/services/tester_recovery.rs</c>). Sleeps a
/// startup grace (5min) once, then sweeps every 10min: force-releasing locks held
/// by finished benchmarks and resolving testers stuck in transient power states
/// (see <see cref="TesterRecovery"/> for the SQL + decisions).
///
/// <para>Follows the background-service pattern (<see cref="PeriodicTimer"/>,
/// leader election, <see cref="TickMonitor"/>). After the startup grace the Rust
/// ticker's first tick fires immediately, so we run one scan before the first
/// interval wait. The cloud probe is stubbed
/// (<see cref="TesterRecovery.NoCloudProbe"/>) — the control plane has no cloud
/// access, matching the Rust "failed to load cloud provider → mark error" arm;
/// a later pass can inject a real host-side probe.</para>
///
/// <para><b>Schema caveat:</b> reads the legacy <c>project_tester</c> /
/// <c>benchmark_config</c> tables — see <see cref="TesterDispatcherService"/>.
/// Registered only via <see cref="TesterLoopsExtensions.AddTesterLoops"/>.</para>
/// </summary>
public sealed class TesterRecoveryService : BackgroundService
{
    private const string ServiceName = "tester-recovery";

    private readonly NpgsqlDataSource _dataSource;
    private readonly ILogger<TesterRecoveryService> _logger;
    private readonly PgAdvisoryLeaderLock? _leader;
    private readonly TickMonitor _monitor;
    private readonly TesterRecovery.ProbeCloudState _probe;
    private readonly long _lockKey = LeaderLockKeys.KeyFor(ServiceName);

    public TesterRecoveryService(
        NpgsqlDataSource dataSource,
        ILogger<TesterRecoveryService> logger,
        PgAdvisoryLeaderLock? leaderLock = null,
        TickMonitor? tickMonitor = null,
        TesterRecovery.ProbeCloudState? probe = null)
    {
        _dataSource = dataSource;
        _logger = logger;
        _leader = leaderLock;
        _monitor = tickMonitor ?? new TickMonitor();
        _probe = probe ?? TesterRecovery.NoCloudProbe;
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        _logger.LogInformation(
            "tester crash-recovery service started (grace {GraceMin}min, sweep every {SweepMin}min)",
            TesterRecovery.StartupGrace.TotalMinutes, TesterRecovery.SweepInterval.TotalMinutes);
        _monitor.ReportStarted(ServiceName);

        try
        {
            await Task.Delay(TesterRecovery.StartupGrace, stoppingToken).ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            return;
        }

        // Immediate post-grace scan (Rust ticker's first tick fires immediately).
        await RunGuardedTickAsync(stoppingToken).ConfigureAwait(false);

        using var timer = new PeriodicTimer(TesterRecovery.SweepInterval);
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
            _logger.LogWarning(ex, "tester crash recovery scan failed");
        }
    }

    private async Task TickAsync(CancellationToken ct)
    {
        await using var conn = await _dataSource.OpenConnectionAsync(ct).ConfigureAwait(false);
        var (locks, stucks) = await TesterRecovery.ScanAsync(conn, _probe, _logger, ct).ConfigureAwait(false);
        _logger.LogInformation(
            "tester crash recovery scan complete locks_released={Locks} transients_handled={Stucks}",
            locks, stucks);
        _monitor.ReportTick(ServiceName, locks + stucks, $"locks={locks} transients={stucks}");
    }
}

/// <summary>
/// DI wiring for the tester dispatch + recovery loops. These read the legacy
/// <c>benchmark_config</c> / <c>project_tester</c> tables, so they are opt-in
/// (NOT part of the standard <c>AddNetworkerSchedulerServices</c> wiring). Add in
/// <c>Program.cs</c> only when those tables exist:
/// <code>builder.Services.AddTesterLoops();</code>
/// Both honor <see cref="BackgroundServicesGate"/>.
/// </summary>
public static class TesterLoopsExtensions
{
    public static IServiceCollection AddTesterLoops(this IServiceCollection services)
    {
        if (!BackgroundServicesGate.IsEnabled("tester-dispatcher/recovery"))
        {
            return services;
        }

        services.AddHostedService<TesterDispatcherService>();
        services.AddHostedService<TesterRecoveryService>();
        return services;
    }
}
