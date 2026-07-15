using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Background;
using Npgsql;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// M6 cutover-hardening ops endpoints — the observability surface for the
/// background loops plus the deploy readiness probe. Mapped by Program.cs via
/// <c>app.MapOpsEndpoints()</c> (requires <c>AddOpsInfrastructure()</c> in the
/// service registrations).
///
/// <list type="bullet">
///   <item><b>GET /api/health/background</b> (GlobalViewer) — one row per
///     background service that has reported into the <see cref="TickMonitor"/>:
///     last tick time, staleness, tick count, items, last error. A service is
///     <c>healthy</c> when it ticked within 3× its expected interval (a
///     never-ticked service gets the same 3× grace from its loop start, which
///     covers every loop's startup delay). <c>all_healthy</c> is the AND across
///     services — the primary soak signal during cutover.</item>
///   <item><b>GET /api/health/ready</b> (public) — readiness probe for the
///     deploy: 200 once the database answers <c>SELECT 1</c>, 503 otherwise.
///     Distinct from <c>/api/health</c> (liveness/info): a load balancer should
///     not route traffic to a replica that cannot reach the DB.</item>
/// </list>
/// </summary>
public static class OpsEndpoints
{
    /// <summary>
    /// Expected tick interval per service — kept in sync by hand with each
    /// service's <c>TickInterval</c> constant (they are private; duplicating the
    /// literal here is the deliberate, greppable trade-off). Health threshold is
    /// <see cref="HealthyIntervalMultiplier"/> × this value.
    /// </summary>
    public static readonly IReadOnlyDictionary<string, TimeSpan> ExpectedIntervals =
        new Dictionary<string, TimeSpan>(StringComparer.Ordinal)
        {
            [OpsServiceNames.Scheduler] = TimeSpan.FromSeconds(30),
            [OpsServiceNames.QueuedRedispatch] = TimeSpan.FromSeconds(30),
            [OpsServiceNames.Watchdog] = TimeSpan.FromSeconds(60),
            [OpsServiceNames.AgentReaper] = TimeSpan.FromSeconds(60),
            [OpsServiceNames.AutoShutdown] = TimeSpan.FromSeconds(60),
            [OpsServiceNames.OrphanReaper] = TimeSpan.FromMinutes(10),
            [OpsServiceNames.WorkspaceInactivity] = TimeSpan.FromHours(24),
            [OpsServiceNames.ProvisioningOrchestrator] = TimeSpan.FromSeconds(5),
        };

    /// <summary>Healthy = ticked within this many expected intervals. 3× rides
    /// out one slow tick + one lost-to-another-replica tick without flapping.</summary>
    public const int HealthyIntervalMultiplier = 3;

    /// <summary>Fallback expected interval for a service name missing from
    /// <see cref="ExpectedIntervals"/> (a new loop whose map entry was
    /// forgotten) — generous, so the omission surfaces as a code review fix,
    /// not a false alert.</summary>
    public static readonly TimeSpan DefaultExpectedInterval = TimeSpan.FromMinutes(10);

    public static IEndpointRouteBuilder MapOpsEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/health/background — per-service tick observability.
        app.MapGet("/api/health/background", (TickMonitor monitor) =>
        {
            var now = monitor.UtcNow;
            var snapshots = monitor.Snapshot();

            var services = snapshots.Select(s =>
            {
                var expected = ExpectedIntervalFor(s.Service);
                return new
                {
                    name = s.Service,
                    last_tick_at = s.LastTickAt,
                    seconds_since_tick = SecondsSinceTick(s, now),
                    ticks_total = s.TicksTotal,
                    last_items = s.LastItems,
                    last_note = s.LastNote,
                    last_error = s.LastError,
                    last_error_at = s.LastErrorAt,
                    expected_interval_secs = (long)expected.TotalSeconds,
                    healthy = IsHealthy(s, expected, now),
                };
            }).ToList();

            return Results.Ok(new
            {
                services,
                all_healthy = snapshots.All(s => IsHealthy(s, ExpectedIntervalFor(s.Service), now)),
                // Context for the empty-list case: an API-only replica
                // (DASHBOARD_BACKGROUND_SERVICES=0) hosts no loops, so an empty
                // `services` there is correct, not broken.
                background_services_enabled = BackgroundServicesGate.ParseEnabled(
                    Environment.GetEnvironmentVariable(BackgroundServicesGate.EnvVar)),
            });
        }).RequireAuthorization(AuthPolicies.GlobalViewer);

        // GET /api/health/ready — public readiness probe (DB reachable).
        app.MapGet("/api/health/ready", async (NpgsqlDataSource dataSource, CancellationToken ct) =>
        {
            try
            {
                await using var conn = await dataSource.OpenConnectionAsync(ct).ConfigureAwait(false);
                await using var cmd = conn.CreateCommand();
                cmd.CommandText = "SELECT 1";
                await cmd.ExecuteScalarAsync(ct).ConfigureAwait(false);
                return Results.Ok(new { status = "ready", db = "ok" });
            }
            catch (Exception) when (!ct.IsCancellationRequested)
            {
                // Deliberately no detail: this endpoint is unauthenticated.
                return Results.Json(
                    new { status = "unready", db = "error" },
                    statusCode: StatusCodes.Status503ServiceUnavailable);
            }
        });

        return app;
    }

    /// <summary>Expected interval for a service, with the defensive fallback.</summary>
    public static TimeSpan ExpectedIntervalFor(string service) =>
        ExpectedIntervals.TryGetValue(service, out var interval) ? interval : DefaultExpectedInterval;

    /// <summary>
    /// Seconds since the last successful tick, or since the loop start when it
    /// has never ticked (so a wedged-from-birth loop still ages into unhealthy).
    /// Clamped at 0 against clock skew between reporter and reader.
    /// </summary>
    public static double SecondsSinceTick(ServiceTickSnapshot s, DateTimeOffset now) =>
        Math.Max(0, (now - (s.LastTickAt ?? s.StartedAt)).TotalSeconds);

    /// <summary>
    /// Pure health predicate (unit-tested): a service is healthy when its last
    /// tick — or its loop start, if it never ticked — is within
    /// <see cref="HealthyIntervalMultiplier"/> × the expected interval.
    /// </summary>
    public static bool IsHealthy(ServiceTickSnapshot s, TimeSpan expectedInterval, DateTimeOffset now) =>
        SecondsSinceTick(s, now) <= HealthyIntervalMultiplier * expectedInterval.TotalSeconds;
}
