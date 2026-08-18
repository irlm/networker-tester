using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.Monitoring.Data;
using Networker.Monitoring.Data.Entities;
using MonitorEntity = Networker.Monitoring.Data.Entities.Monitor;

namespace Networker.Monitoring.Endpoints;

public static class MonitorEndpoints
{
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);

    public static IEndpointRouteBuilder MapMonitorEndpoints(this IEndpointRouteBuilder app)
    {
        var api = app.MapGroup("/api/v1");

        api.MapPost("/projects/{projectId}/monitors", CreateAsync);
        api.MapGet("/projects/{projectId}/monitors", ListAsync);
        api.MapGet("/monitors/{monitorId:guid}", GetAsync);
        api.MapPatch("/monitors/{monitorId:guid}", UpdateAsync);
        api.MapDelete("/monitors/{monitorId:guid}", DeleteAsync);
        api.MapGet("/monitors/{monitorId:guid}/checks", ListChecksAsync);
        api.MapGet("/locations", ListLocationsAsync);
        return app;
    }

    private static async Task<IResult> CreateAsync(
        string projectId,
        CreateMonitorRequest request,
        MonitoringDbContext db,
        TimeProvider timeProvider,
        CancellationToken cancellationToken)
    {
        var error = Validate(projectId, request.Name, request.TargetUrl, request.Method,
            request.Assertions, request.IntervalSeconds, request.TimeoutMs, request.Criticality);
        if (error is not null)
        {
            return Results.BadRequest(new { error });
        }

        var location = request.LocationId is Guid requestedLocation
            ? await db.MonitorLocations.FirstOrDefaultAsync(
                row => row.LocationId == requestedLocation && row.Enabled, cancellationToken)
            : await db.MonitorLocations
                .Where(row => row.Enabled)
                .OrderBy(row => row.Name)
                .FirstOrDefaultAsync(cancellationToken);
        if (location is null)
        {
            return Results.BadRequest(new { error = "No enabled monitoring location is available." });
        }

        var now = timeProvider.GetUtcNow();
        var monitor = new MonitorEntity
        {
            MonitorId = Guid.NewGuid(),
            ProjectId = projectId.Trim(),
            Name = request.Name!.Trim(),
            TargetUrl = NormalizeUrl(request.TargetUrl!),
            Method = NormalizeMethod(request.Method),
            AssertionConfig = JsonSerializer.Serialize(request.Assertions ?? new MonitorAssertions(), JsonOptions),
            IntervalSeconds = request.IntervalSeconds ?? 60,
            TimeoutMs = request.TimeoutMs ?? 10_000,
            Enabled = true,
            Criticality = NormalizeCriticality(request.Criticality),
            RetentionPolicy = "{}",
            NextCheckAt = now,
            CreatedAt = now,
            UpdatedAt = now,
        };
        monitor.LocationAssignments.Add(new MonitorLocationAssignment
        {
            MonitorId = monitor.MonitorId,
            LocationId = location.LocationId,
            ParticipatesInIncidentQuorum = true,
            CreatedAt = now,
        });

        db.Monitors.Add(monitor);
        try
        {
            await db.SaveChangesAsync(cancellationToken);
        }
        catch (DbUpdateException)
        {
            return Results.Conflict(new { error = "A monitor with this name already exists in the project." });
        }

        var view = ToView(monitor, location.LocationId, latestOutcome: null, latestCheckAt: null);
        return Results.Created($"/api/v1/monitors/{monitor.MonitorId}", view);
    }

    private static async Task<IResult> ListAsync(
        string projectId,
        int? limit,
        DateTimeOffset? before,
        MonitoringDbContext db,
        CancellationToken cancellationToken)
    {
        var take = Math.Clamp(limit ?? 50, 1, 100);
        var query = db.Monitors
            .AsNoTracking()
            .Where(monitor => monitor.ProjectId == projectId && monitor.DeletedAt == null);
        if (before is DateTimeOffset cursor)
        {
            query = query.Where(monitor => monitor.CreatedAt < cursor);
        }

        var monitors = await query
            .OrderByDescending(monitor => monitor.CreatedAt)
            .Take(take + 1)
            .Select(monitor => new
            {
                Monitor = monitor,
                LocationId = monitor.LocationAssignments.Select(a => a.LocationId).First(),
                LatestOutcome = monitor.Checks
                    .OrderByDescending(check => check.ScheduledAt)
                    .Select(check => check.Outcome)
                    .FirstOrDefault(),
                LatestCheckAt = monitor.Checks
                    .OrderByDescending(check => check.ScheduledAt)
                    .Select(check => (DateTimeOffset?)check.FinishedAt)
                    .FirstOrDefault(),
            })
            .ToListAsync(cancellationToken);

        var page = monitors.Take(take).ToArray();
        var items = page
            .Select(row => ToView(
                row.Monitor,
                row.LocationId,
                row.LatestOutcome,
                row.LatestCheckAt))
            .ToArray();
        var nextCursor = monitors.Count > take ? page[^1].Monitor.CreatedAt.ToString("O") : null;
        return Results.Ok(new ListResponse<MonitorView>(items, nextCursor));
    }

    private static async Task<IResult> GetAsync(
        Guid monitorId,
        MonitoringDbContext db,
        CancellationToken cancellationToken)
    {
        var row = await db.Monitors
            .AsNoTracking()
            .Where(monitor => monitor.MonitorId == monitorId && monitor.DeletedAt == null)
            .Select(monitor => new
            {
                Monitor = monitor,
                LocationId = monitor.LocationAssignments.Select(a => a.LocationId).First(),
            })
            .FirstOrDefaultAsync(cancellationToken);
        if (row is null)
        {
            return Results.NotFound(new { error = "Monitor not found." });
        }

        var latest = await db.MonitorChecks
            .AsNoTracking()
            .Where(check => check.MonitorId == monitorId)
            .OrderByDescending(check => check.ScheduledAt)
            .FirstOrDefaultAsync(cancellationToken);
        return Results.Ok(ToView(row.Monitor, row.LocationId, latest?.Outcome, latest?.FinishedAt));
    }

    private static async Task<IResult> UpdateAsync(
        Guid monitorId,
        UpdateMonitorRequest request,
        MonitoringDbContext db,
        TimeProvider timeProvider,
        CancellationToken cancellationToken)
    {
        var monitor = await db.Monitors
            .Include(row => row.LocationAssignments)
            .FirstOrDefaultAsync(
                row => row.MonitorId == monitorId && row.DeletedAt == null,
                cancellationToken);
        if (monitor is null)
        {
            return Results.NotFound(new { error = "Monitor not found." });
        }

        var error = Validate(
            monitor.ProjectId,
            request.Name ?? monitor.Name,
            request.TargetUrl ?? monitor.TargetUrl,
            request.Method ?? monitor.Method,
            request.Assertions ?? DeserializeAssertions(monitor.AssertionConfig),
            request.IntervalSeconds ?? monitor.IntervalSeconds,
            request.TimeoutMs ?? monitor.TimeoutMs,
            request.Criticality ?? monitor.Criticality);
        if (error is not null)
        {
            return Results.BadRequest(new { error });
        }

        await using var transaction = await db.Database.BeginTransactionAsync(cancellationToken);
        if (request.LocationId is Guid locationId
            && monitor.LocationAssignments.Single().LocationId != locationId)
        {
            var locationExists = await db.MonitorLocations.AnyAsync(
                row => row.LocationId == locationId && row.Enabled,
                cancellationToken);
            if (!locationExists)
            {
                return Results.BadRequest(new { error = "The selected monitoring location is unavailable." });
            }

            var oldAssignment = monitor.LocationAssignments.Single();
            db.MonitorLocationAssignments.Remove(oldAssignment);
            await db.SaveChangesAsync(cancellationToken);
            db.Entry(oldAssignment).State = EntityState.Detached;
            monitor.LocationAssignments.Clear();
            monitor.LocationAssignments.Add(new MonitorLocationAssignment
            {
                MonitorId = monitorId,
                LocationId = locationId,
                ParticipatesInIncidentQuorum = true,
                CreatedAt = timeProvider.GetUtcNow(),
            });
        }

        if (request.Name is not null) monitor.Name = request.Name.Trim();
        if (request.TargetUrl is not null) monitor.TargetUrl = NormalizeUrl(request.TargetUrl);
        if (request.Method is not null) monitor.Method = NormalizeMethod(request.Method);
        if (request.Assertions is not null)
            monitor.AssertionConfig = JsonSerializer.Serialize(request.Assertions, JsonOptions);
        if (request.IntervalSeconds is int interval) monitor.IntervalSeconds = interval;
        if (request.TimeoutMs is int timeout) monitor.TimeoutMs = timeout;
        if (request.Criticality is not null) monitor.Criticality = NormalizeCriticality(request.Criticality);
        if (request.Enabled is bool enabled)
        {
            var wasEnabled = monitor.Enabled;
            monitor.Enabled = enabled;
            if (enabled && !wasEnabled)
            {
                monitor.NextCheckAt = timeProvider.GetUtcNow();
            }
        }
        monitor.UpdatedAt = timeProvider.GetUtcNow();

        try
        {
            await db.SaveChangesAsync(cancellationToken);
        }
        catch (DbUpdateException)
        {
            return Results.Conflict(new { error = "The monitor update conflicts with an existing record." });
        }

        await transaction.CommitAsync(cancellationToken);
        return Results.Ok(ToView(
            monitor,
            monitor.LocationAssignments.Single().LocationId,
            latestOutcome: null,
            latestCheckAt: null));
    }

    private static async Task<IResult> DeleteAsync(
        Guid monitorId,
        MonitoringDbContext db,
        TimeProvider timeProvider,
        CancellationToken cancellationToken)
    {
        var monitor = await db.Monitors.FirstOrDefaultAsync(
            row => row.MonitorId == monitorId && row.DeletedAt == null,
            cancellationToken);
        if (monitor is null)
        {
            return Results.NotFound(new { error = "Monitor not found." });
        }

        var now = timeProvider.GetUtcNow();
        monitor.Enabled = false;
        monitor.LeaseOwner = null;
        monitor.LeaseExpiresAt = null;
        monitor.DeletedAt = now;
        monitor.UpdatedAt = now;
        await db.SaveChangesAsync(cancellationToken);
        return Results.NoContent();
    }

    private static async Task<IResult> ListChecksAsync(
        Guid monitorId,
        int? limit,
        DateTimeOffset? before,
        MonitoringDbContext db,
        CancellationToken cancellationToken)
    {
        var exists = await db.Monitors.AsNoTracking().AnyAsync(
            row => row.MonitorId == monitorId && row.DeletedAt == null,
            cancellationToken);
        if (!exists)
        {
            return Results.NotFound(new { error = "Monitor not found." });
        }

        var take = Math.Clamp(limit ?? 50, 1, 100);
        var query = db.MonitorChecks.AsNoTracking().Where(check => check.MonitorId == monitorId);
        if (before is DateTimeOffset cursor)
        {
            query = query.Where(check => check.ScheduledAt < cursor);
        }

        var rows = await query.OrderByDescending(check => check.ScheduledAt)
            .Take(take + 1)
            .ToListAsync(cancellationToken);
        var page = rows.Take(take).Select(ToCheckView).ToArray();
        var nextCursor = rows.Count > take ? page[^1].ScheduledAt.ToString("O") : null;
        return Results.Ok(new ListResponse<MonitorCheckView>(page, nextCursor));
    }

    private static async Task<IResult> ListLocationsAsync(
        MonitoringDbContext db,
        CancellationToken cancellationToken)
    {
        var locations = await db.MonitorLocations.AsNoTracking()
            .OrderBy(location => location.Name)
            .Select(location => new MonitorLocationView(
                location.LocationId,
                location.Name,
                location.Provider,
                location.Region,
                location.LastHeartbeatAt,
                location.Enabled))
            .ToListAsync(cancellationToken);
        return Results.Ok(new ListResponse<MonitorLocationView>(locations, null));
    }

    private static string? Validate(
        string projectId,
        string? name,
        string? targetUrl,
        string? method,
        MonitorAssertions? assertions,
        int? intervalSeconds,
        int? timeoutMs,
        string? criticality)
    {
        if (string.IsNullOrWhiteSpace(projectId) || projectId.Length > 128)
            return "Project ID is required and must be at most 128 characters.";
        if (string.IsNullOrWhiteSpace(name) || name.Trim().Length > 200)
            return "Name is required and must be at most 200 characters.";
        if (!Uri.TryCreate(targetUrl, UriKind.Absolute, out var uri)
            || (uri.Scheme != Uri.UriSchemeHttp && uri.Scheme != Uri.UriSchemeHttps)
            || string.IsNullOrEmpty(uri.Host)
            || !string.IsNullOrEmpty(uri.UserInfo))
            return "Target URL must be an absolute HTTP or HTTPS URL without embedded credentials.";
        if (NormalizeMethod(method) is not ("GET" or "HEAD"))
            return "Method must be GET or HEAD in the foundation release.";
        if (intervalSeconds is < 60 or > 86_400)
            return "Interval must be between 60 and 86400 seconds.";
        if (timeoutMs is < 100 or > 30_000)
            return "Timeout must be between 100 and 30000 milliseconds.";
        if (NormalizeCriticality(criticality) is not ("reporting" or "critical"))
            return "Criticality must be reporting or critical.";

        var rules = assertions ?? new MonitorAssertions();
        if (rules.StatusMin is < 100 or > 599 || rules.StatusMax is < 100 or > 599
            || rules.StatusMin > rules.StatusMax)
            return "Status range must be between 100 and 599 with min not greater than max.";
        if (rules.WarningLatencyMs is <= 0 || rules.CriticalLatencyMs is <= 0)
            return "Latency thresholds must be positive.";
        if (rules.WarningLatencyMs is int warning && rules.CriticalLatencyMs is int critical
            && warning > critical)
            return "Warning latency must not exceed critical latency.";
        return null;
    }

    private static string NormalizeUrl(string value) => new Uri(value.Trim()).AbsoluteUri;
    private static string NormalizeMethod(string? value) => (value ?? "GET").Trim().ToUpperInvariant();
    private static string NormalizeCriticality(string? value) =>
        (value ?? "reporting").Trim().ToLowerInvariant();

    private static MonitorAssertions DeserializeAssertions(string json) =>
        JsonSerializer.Deserialize<MonitorAssertions>(json, JsonOptions) ?? new MonitorAssertions();

    private static MonitorView ToView(
        MonitorEntity monitor,
        Guid locationId,
        string? latestOutcome,
        DateTimeOffset? latestCheckAt) =>
        new(
            monitor.MonitorId,
            monitor.ProjectId,
            monitor.Name,
            monitor.TargetUrl,
            monitor.Method,
            DeserializeAssertions(monitor.AssertionConfig),
            monitor.IntervalSeconds,
            monitor.TimeoutMs,
            monitor.Enabled,
            monitor.Criticality,
            locationId,
            latestOutcome,
            latestCheckAt,
            monitor.CreatedAt,
            monitor.UpdatedAt);

    private static MonitorCheckView ToCheckView(MonitorCheck check) =>
        new(
            check.CheckId,
            check.MonitorId,
            check.LocationId,
            check.ScheduledAt,
            check.StartedAt,
            check.FinishedAt,
            check.Outcome,
            check.FailureKind,
            check.StatusCode,
            check.TotalMs,
            JsonSerializer.Deserialize<object>(check.AssertionResults, JsonOptions) ?? new { },
            check.ErrorSummary);
}
