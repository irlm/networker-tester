using Networker.Monitoring.Data;

namespace Networker.Monitoring.Scheduling;

public sealed class MonitorScheduler(
    IServiceScopeFactory scopeFactory,
    TimeProvider timeProvider,
    IConfiguration configuration,
    ILogger<MonitorScheduler> logger) : BackgroundService
{
    private readonly string _workerId = $"{Environment.MachineName}:{Environment.ProcessId}:{Guid.NewGuid():N}";
    private readonly bool _enabled = configuration["MONITORING_BACKGROUND_SERVICES"] == "1";

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        if (!_enabled)
        {
            logger.LogInformation("Monitoring scheduler is disabled by configuration");
            return;
        }

        using var timer = new PeriodicTimer(TimeSpan.FromSeconds(2), timeProvider);
        do
        {
            try
            {
                await RunTickAsync(stoppingToken);
            }
            catch (OperationCanceledException) when (stoppingToken.IsCancellationRequested)
            {
                break;
            }
            catch (Exception exception)
            {
                logger.LogError(exception, "Monitoring scheduler tick failed");
            }
        }
        while (await timer.WaitForNextTickAsync(stoppingToken));
    }

    private async Task RunTickAsync(CancellationToken cancellationToken)
    {
        using var leaseScope = scopeFactory.CreateScope();
        var db = leaseScope.ServiceProvider.GetRequiredService<MonitoringDbContext>();
        var leases = await MonitorLeaseService.LeaseDueAsync(
            db,
            _workerId,
            timeProvider.GetUtcNow(),
            batchSize: 10,
            leaseDuration: TimeSpan.FromSeconds(45),
            cancellationToken);

        foreach (var lease in leases)
        {
            using var runScope = scopeFactory.CreateScope();
            var runner = runScope.ServiceProvider.GetRequiredService<MonitorCheckRunner>();
            await runner.RunAsync(lease, cancellationToken);
        }
    }
}
