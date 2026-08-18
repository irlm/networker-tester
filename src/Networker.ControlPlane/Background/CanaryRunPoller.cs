using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Endpoints;
using Networker.Data;

namespace Networker.ControlPlane.Background;

/// <summary>
/// Backfills GitHub run details onto <c>canary_dispatch</c> history rows.
///
/// <para>GitHub's <c>workflow_dispatch</c> API answers 204 with NO run id, so
/// <see cref="CanaryEndpoints"/> can only record who/when/ref/inputs at trigger
/// time. This loop closes the gap in two steps per tick:</para>
///
/// <list type="number">
///   <item><b>Resolve</b> — for rows with no <c>run_id</c> yet, list recent
///     <c>soak-canary.yml</c> runs and link the oldest unclaimed
///     <c>workflow_dispatch</c> run created at/after the dispatch time (small
///     clock-skew grace). Rows older than <see cref="ResolveWindow"/> that never
///     matched are marked <c>conclusion = "unresolved"</c> so they stop being
///     scanned.</item>
///   <item><b>Refresh</b> — for linked rows whose run is not <c>completed</c>,
///     fetch the run and copy status/conclusion. Completed rows are final and
///     never touched again — which is what keeps the history readable when
///     GitHub goes down later.</item>
/// </list>
///
/// <para>Ticks every 2 minutes but exits the tick immediately when there is
/// nothing pending (the common case), so the steady-state GitHub API cost is
/// zero. Requires <c>CANARY_GITHUB_TOKEN</c>; without it the loop idles.
/// Registered behind <see cref="BackgroundServicesGate"/> like every other
/// DB-writing loop (API-only replicas must not double-write).</para>
/// </summary>
public sealed class CanaryRunPoller : BackgroundService
{
    private static readonly TimeSpan TickInterval = TimeSpan.FromMinutes(2);

    /// <summary>Give up linking a dispatch to a run after this long.</summary>
    internal static readonly TimeSpan ResolveWindow = TimeSpan.FromHours(6);

    /// <summary>Clock-skew grace when matching run.created_at >= requested_at.</summary>
    internal static readonly TimeSpan SkewGrace = TimeSpan.FromMinutes(2);

    private readonly IServiceScopeFactory _scopeFactory;
    private readonly IHttpClientFactory _httpFactory;
    private readonly ILogger<CanaryRunPoller> _logger;

    public CanaryRunPoller(
        IServiceScopeFactory scopeFactory,
        IHttpClientFactory httpFactory,
        ILogger<CanaryRunPoller> logger)
    {
        _scopeFactory = scopeFactory;
        _httpFactory = httpFactory;
        _logger = logger;
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        _logger.LogInformation("canary run poller started (tick {Minutes}min)", TickInterval.TotalMinutes);
        using var timer = new PeriodicTimer(TickInterval);
        while (await WaitNextTickSafeAsync(timer, stoppingToken))
        {
            try
            {
                await TickAsync(stoppingToken);
            }
            catch (OperationCanceledException) when (stoppingToken.IsCancellationRequested)
            {
                break;
            }
            catch (Exception ex)
            {
                // Never let one bad tick kill the loop.
                _logger.LogWarning(ex, "canary run poller tick failed");
            }
        }
    }

    internal async Task TickAsync(CancellationToken ct)
    {
        var token = CanaryEndpoints.GitHubToken();
        if (token is null)
        {
            return; // Not configured (local lab): idle.
        }

        using var scope = _scopeFactory.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
        var now = DateTime.UtcNow;
        var resolveFloor = now - ResolveWindow;

        // Anything to do at all? (steady state: no — skip the GitHub calls)
        var pending = await db.CanaryDispatches
            .Where(d => (d.RunId == null && d.Conclusion == null)
                        || (d.RunId != null && d.RunStatus != "completed"))
            .OrderBy(d => d.RequestedAt)
            .ToListAsync(ct);
        if (pending.Count == 0)
        {
            return;
        }

        var (owner, repo) = CanaryEndpoints.RepoSlug();

        // 1. Resolve run ids for unlinked rows.
        var unlinked = pending.Where(d => d.RunId == null).ToList();
        if (unlinked.Count > 0)
        {
            var runs = await CanaryEndpoints.FetchRunsAsync(_httpFactory, owner, repo, token, 50, ct);
            var claimed = await db.CanaryDispatches
                .Where(d => d.RunId != null)
                .Select(d => d.RunId!.Value)
                .ToHashSetAsync(ct);

            foreach (var row in unlinked)
            {
                // Oldest unclaimed workflow_dispatch run created at/after the
                // dispatch time (grace for clock skew between us and GitHub).
                var match = runs
                    .Where(r => r.Event == "workflow_dispatch"
                                && !claimed.Contains(r.Id)
                                && DateTime.TryParse(r.CreatedAt, null,
                                       System.Globalization.DateTimeStyles.AdjustToUniversal
                                       | System.Globalization.DateTimeStyles.AssumeUniversal,
                                       out var created)
                                && created >= row.RequestedAt - SkewGrace)
                    .OrderBy(r => r.CreatedAt, StringComparer.Ordinal)
                    .FirstOrDefault();

                if (match is not null)
                {
                    row.RunId = match.Id;
                    row.RunUrl = match.HtmlUrl;
                    row.RunStatus = match.Status;
                    row.Conclusion = match.Status == "completed" ? match.Conclusion : null;
                    row.UpdatedAt = now;
                    claimed.Add(match.Id);
                    _logger.LogInformation(
                        "canary dispatch {Id} linked to run {RunId} ({Status})",
                        row.Id, match.Id, match.Status);
                }
                else if (row.RequestedAt < resolveFloor)
                {
                    // Never showed up (GitHub purge, mismatch, manual cancel
                    // before the run was created): stop scanning it.
                    row.Conclusion = "unresolved";
                    row.UpdatedAt = now;
                    _logger.LogWarning(
                        "canary dispatch {Id} ({At:u}) never matched a run; marking unresolved",
                        row.Id, row.RequestedAt);
                }
            }
        }

        // 2. Refresh linked-but-not-completed rows.
        foreach (var row in pending.Where(d => d.RunId != null && d.RunStatus != "completed"))
        {
            var run = await CanaryEndpoints.FetchRunAsync(_httpFactory, owner, repo, token, row.RunId!.Value, ct);
            if (run is null)
            {
                continue; // transient GitHub error; retry next tick
            }
            if (run.Status != row.RunStatus || run.Conclusion != row.Conclusion)
            {
                row.RunStatus = run.Status;
                row.Conclusion = run.Conclusion;
                row.UpdatedAt = now;
                if (run.Status == "completed")
                {
                    _logger.LogInformation(
                        "canary run {RunId} completed: {Conclusion}", row.RunId, run.Conclusion);
                }
            }
        }

        await db.SaveChangesAsync(ct);
    }

    private static async Task<bool> WaitNextTickSafeAsync(PeriodicTimer timer, CancellationToken ct)
    {
        try
        {
            return await timer.WaitForNextTickAsync(ct);
        }
        catch (OperationCanceledException)
        {
            return false;
        }
    }
}

/// <summary>DI wiring: <c>builder.Services.AddCanaryRunPoller();</c> — skipped
/// on API-only replicas (the loop writes to <c>canary_dispatch</c>).</summary>
public static class CanaryRunPollerExtensions
{
    public static IServiceCollection AddCanaryRunPoller(this IServiceCollection services)
    {
        if (!BackgroundServicesGate.IsEnabled("canary-run-poller"))
        {
            return services;
        }
        services.AddHostedService<CanaryRunPoller>();
        return services;
    }
}
