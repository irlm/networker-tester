using Microsoft.Extensions.DependencyInjection.Extensions;

namespace Networker.ControlPlane.Alerting;

/// <summary>
/// DI wiring for the alerting module: the scoped evaluator + metric provider
/// (they share the request/frame scope's <c>NetworkerDbContext</c>), the
/// singleton notifier, and the named webhook HttpClient with the 10s
/// delivery timeout. <c>TryAdd</c> so a test host can register fakes first.
/// </summary>
public static class AlertingServiceCollectionExtensions
{
    public static IServiceCollection AddNetworkerAlerting(this IServiceCollection services)
    {
        services.AddHttpClient(AlertNotifier.WebhookClientName, c =>
        {
            c.Timeout = TimeSpan.FromSeconds(10);
        });

        services.TryAddSingleton<IAlertNotifier, AlertNotifier>();
        services.TryAddScoped<RunMetricProvider>();
        services.TryAddScoped<AlertEvaluator>();

        return services;
    }
}
