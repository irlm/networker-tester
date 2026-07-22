using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Alerting;
using Networker.ControlPlane.Auth;
using Networker.Data;
using Networker.Security;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// REST v2 endpoints for the alerting module (wave 1 — backend): CRUD for
/// notification channels + threshold rules, the paginated event history, and
/// a channel test-fire. Route shape follows the sibling v2 modules
/// (<see cref="SchedulesEndpoints"/> / <see cref="ComparisonGroupsEndpoints"/>):
/// project-scoped collection routes under
/// <c>/api/v2/projects/{projectId}/...</c> gated by the ProjectMember /
/// ProjectOperator policies, flat per-row routes gated by row-level
/// <see cref="ProjectAccessChecker"/> (no access → 404, never an existence
/// oracle). JSON is snake_case; 4xx bodies use the shared
/// <see cref="ApiError"/> envelope.
///
/// Evaluation/delivery live in <see cref="Alerting.AlertEvaluator"/> /
/// <see cref="Alerting.AlertNotifier"/> — this module only owns the CRUD +
/// history surface the frontend (wave 2) consumes.
/// </summary>
public static class AlertsEndpoints
{
    private const int DefaultLimit = 50;
    private const int MaxLimit = 200;

    /// <summary>
    /// Placeholder returned instead of a webhook secret on reads. A PATCH that
    /// sends it back verbatim keeps the stored secret (so round-tripping the
    /// DTO through an edit form never wipes the secret).
    /// </summary>
    public const string SecretMask = "********";

    public static IEndpointRouteBuilder MapAlertsEndpoints(this IEndpointRouteBuilder app)
    {
        MapChannelEndpoints(app);
        MapRuleEndpoints(app);
        MapEventEndpoints(app);
        return app;
    }

    // ── Channels ─────────────────────────────────────────────────────────────

    private static void MapChannelEndpoints(IEndpointRouteBuilder app)
    {
        // POST /api/v2/projects/{projectId}/alert-channels — create (operator).
        app.MapPost("/api/v2/projects/{projectId}/alert-channels", async (
            string projectId,
            ChannelRequest body,
            NetworkerDbContext db,
            CredentialCipher cipher,
            CancellationToken ct) =>
        {
            if (string.IsNullOrWhiteSpace(body.name))
            {
                return ApiError.BadRequest("name is required");
            }
            if (body.kind is not ("webhook" or "email"))
            {
                return ApiError.BadRequest("kind must be 'webhook' or 'email'");
            }
            var (config, configError) = ValidateChannelConfig(body.kind, body.config, existingConfig: null);
            if (configError is not null)
            {
                return ApiError.BadRequest(configError);
            }
            // Encrypt the webhook secret before it touches the database (no-op for
            // email / secret-less configs).
            config = AlertSecretCipher.ProtectConfigSecret(cipher, config!);

            var row = new Data.Entities.AlertChannel
            {
                ChannelId = Guid.NewGuid(),
                ProjectId = projectId,
                Kind = body.kind,
                Name = body.name.Trim(),
                Config = config!,
                Enabled = body.enabled ?? true,
                CreatedAt = DateTime.UtcNow,
            };
            db.AlertChannels.Add(row);
            await db.SaveChangesAsync(ct);

            return Results.Ok(ChannelDto(row));
        }).RequireAuthorization(AuthPolicies.ProjectOperator);

        // GET /api/v2/projects/{projectId}/alert-channels — list (member).
        app.MapGet("/api/v2/projects/{projectId}/alert-channels", async (
            string projectId,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var rows = await db.AlertChannels
                .AsNoTracking()
                .Where(c => c.ProjectId == projectId)
                .OrderByDescending(c => c.CreatedAt)
                .ToListAsync(ct);

            return Results.Ok(rows.Select(ChannelDto));
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        // PATCH /api/v2/alert-channels/{id} — update name/config/enabled (operator).
        app.MapPatch("/api/v2/alert-channels/{id:guid}", async (
            Guid id,
            ChannelRequest body,
            HttpContext ctx,
            NetworkerDbContext db,
            ProjectAccessChecker access,
            CredentialCipher cipher,
            CancellationToken ct) =>
        {
            var row = await db.AlertChannels.FirstOrDefaultAsync(c => c.ChannelId == id, ct);
            if (row is null || !await access.HasRoleAsync(ctx, row.ProjectId, ProjectRole.Operator, ct))
            {
                return Results.NotFound();
            }

            if (body.kind is not null && body.kind != row.Kind)
            {
                return ApiError.BadRequest("kind cannot be changed");
            }
            if (body.name is not null)
            {
                if (string.IsNullOrWhiteSpace(body.name))
                {
                    return ApiError.BadRequest("name is required");
                }
                row.Name = body.name.Trim();
            }
            if (body.config is not null)
            {
                var (config, configError) = ValidateChannelConfig(row.Kind, body.config, row.Config);
                if (configError is not null)
                {
                    return ApiError.BadRequest(configError);
                }
                // Encrypt the webhook secret at rest (no-op for email / secret-less).
                row.Config = AlertSecretCipher.ProtectConfigSecret(cipher, config!);
            }
            if (body.enabled is not null)
            {
                row.Enabled = body.enabled.Value;
            }

            await db.SaveChangesAsync(ct);
            return Results.Ok(ChannelDto(row));
        }).RequireAuthorization();

        // DELETE /api/v2/alert-channels/{id} — 204; 409 while rules reference it.
        app.MapDelete("/api/v2/alert-channels/{id:guid}", async (
            Guid id,
            HttpContext ctx,
            NetworkerDbContext db,
            ProjectAccessChecker access,
            CancellationToken ct) =>
        {
            var row = await db.AlertChannels.FirstOrDefaultAsync(c => c.ChannelId == id, ct);
            if (row is null || !await access.HasRoleAsync(ctx, row.ProjectId, ProjectRole.Operator, ct))
            {
                return Results.NotFound();
            }

            if (await db.AlertRules.AnyAsync(r => r.ChannelId == id, ct))
            {
                return ApiError.Conflict("channel is referenced by alert rules — delete or repoint them first");
            }

            db.AlertChannels.Remove(row);
            await db.SaveChangesAsync(ct);
            return Results.NoContent();
        }).RequireAuthorization();

        // POST /api/v2/alert-channels/{id}/test — synchronous test delivery
        // (operator). Sends a payload with state "test" through the channel and
        // reports the delivery outcome without recording an event.
        app.MapPost("/api/v2/alert-channels/{id:guid}/test", async (
            Guid id,
            HttpContext ctx,
            NetworkerDbContext db,
            ProjectAccessChecker access,
            IAlertNotifier notifier,
            CancellationToken ct) =>
        {
            var row = await db.AlertChannels
                .AsNoTracking()
                .FirstOrDefaultAsync(c => c.ChannelId == id, ct);
            if (row is null || !await access.HasRoleAsync(ctx, row.ProjectId, ProjectRole.Operator, ct))
            {
                return Results.NotFound();
            }

            var notification = new AlertNotification(
                event_id: Guid.NewGuid(),
                rule_id: Guid.Empty,
                project_id: row.ProjectId.TrimEnd(),
                test_config_id: null,
                run_id: Guid.Empty,
                metric: AlertRuleLogic.MetricP95Ms,
                comparator: AlertRuleLogic.ComparatorGt,
                threshold: 0,
                value: 0,
                state: "test",
                message: $"Test notification for channel '{row.Name}'",
                fired_at: DateTime.UtcNow);

            var status = await notifier.DeliverAsync(row, notification, ct);
            return Results.Ok(new { delivery_status = status });
        }).RequireAuthorization();
    }

    // ── Rules ────────────────────────────────────────────────────────────────

    private static void MapRuleEndpoints(IEndpointRouteBuilder app)
    {
        // POST /api/v2/projects/{projectId}/alert-rules — create (operator).
        app.MapPost("/api/v2/projects/{projectId}/alert-rules", async (
            string projectId,
            RuleRequest body,
            HttpContext ctx,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            if (body.metric is null || !AlertRuleLogic.Metrics.Contains(body.metric))
            {
                return ApiError.BadRequest(
                    $"metric must be one of: {string.Join(", ", AlertRuleLogic.Metrics)}");
            }
            if (body.comparator is null || !AlertRuleLogic.Comparators.Contains(body.comparator))
            {
                return ApiError.BadRequest("comparator must be 'gt' or 'lt'");
            }
            if (body.threshold is not { } threshold || !double.IsFinite(threshold))
            {
                return ApiError.BadRequest("threshold must be a finite number");
            }
            var window = body.window_runs ?? AlertRuleLogic.MinWindowRuns;
            if (window is < AlertRuleLogic.MinWindowRuns or > AlertRuleLogic.MaxWindowRuns)
            {
                return ApiError.BadRequest(
                    $"window_runs must be between {AlertRuleLogic.MinWindowRuns} and {AlertRuleLogic.MaxWindowRuns}");
            }
            if (body.channel_id is not { } channelId)
            {
                return ApiError.BadRequest("channel_id is required");
            }
            if (!await db.AlertChannels.AnyAsync(
                    c => c.ChannelId == channelId && c.ProjectId == projectId, ct))
            {
                return ApiError.BadRequest("channel_id does not name a channel in this project");
            }
            if (body.test_config_id is { } configId
                && !await db.TestConfigs.AnyAsync(
                    c => c.Id == configId && c.ProjectId == projectId, ct))
            {
                return ApiError.BadRequest("test_config_id does not name a config in this project");
            }

            var user = ctx.GetAuthUser();
            var row = new Data.Entities.AlertRule
            {
                RuleId = Guid.NewGuid(),
                ProjectId = projectId,
                TestConfigId = body.test_config_id,
                Metric = body.metric,
                Comparator = body.comparator,
                Threshold = threshold,
                WindowRuns = window,
                Enabled = body.enabled ?? true,
                ChannelId = channelId,
                CreatedBy = user?.UserId,
                CreatedAt = DateTime.UtcNow,
            };
            db.AlertRules.Add(row);
            await db.SaveChangesAsync(ct);

            return Results.Ok(RuleDto(row));
        }).RequireAuthorization(AuthPolicies.ProjectOperator);

        // GET /api/v2/projects/{projectId}/alert-rules — list (member).
        app.MapGet("/api/v2/projects/{projectId}/alert-rules", async (
            string projectId,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var rows = await db.AlertRules
                .AsNoTracking()
                .Where(r => r.ProjectId == projectId)
                .OrderByDescending(r => r.CreatedAt)
                .ToListAsync(ct);

            return Results.Ok(rows.Select(RuleDto));
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        // PATCH /api/v2/alert-rules/{id} — update; only supplied fields apply.
        app.MapPatch("/api/v2/alert-rules/{id:guid}", async (
            Guid id,
            RuleRequest body,
            HttpContext ctx,
            NetworkerDbContext db,
            ProjectAccessChecker access,
            CancellationToken ct) =>
        {
            var row = await db.AlertRules.FirstOrDefaultAsync(r => r.RuleId == id, ct);
            if (row is null || !await access.HasRoleAsync(ctx, row.ProjectId, ProjectRole.Operator, ct))
            {
                return Results.NotFound();
            }

            if (body.metric is not null)
            {
                if (!AlertRuleLogic.Metrics.Contains(body.metric))
                {
                    return ApiError.BadRequest(
                        $"metric must be one of: {string.Join(", ", AlertRuleLogic.Metrics)}");
                }
                row.Metric = body.metric;
            }
            if (body.comparator is not null)
            {
                if (!AlertRuleLogic.Comparators.Contains(body.comparator))
                {
                    return ApiError.BadRequest("comparator must be 'gt' or 'lt'");
                }
                row.Comparator = body.comparator;
            }
            if (body.threshold is { } threshold)
            {
                if (!double.IsFinite(threshold))
                {
                    return ApiError.BadRequest("threshold must be a finite number");
                }
                row.Threshold = threshold;
            }
            if (body.window_runs is { } window)
            {
                if (window is < AlertRuleLogic.MinWindowRuns or > AlertRuleLogic.MaxWindowRuns)
                {
                    return ApiError.BadRequest(
                        $"window_runs must be between {AlertRuleLogic.MinWindowRuns} and {AlertRuleLogic.MaxWindowRuns}");
                }
                row.WindowRuns = window;
            }
            if (body.channel_id is { } channelId)
            {
                if (!await db.AlertChannels.AnyAsync(
                        c => c.ChannelId == channelId && c.ProjectId == row.ProjectId, ct))
                {
                    return ApiError.BadRequest("channel_id does not name a channel in this project");
                }
                row.ChannelId = channelId;
            }
            if (body.test_config_id is { } configId)
            {
                if (!await db.TestConfigs.AnyAsync(
                        c => c.Id == configId && c.ProjectId == row.ProjectId, ct))
                {
                    return ApiError.BadRequest("test_config_id does not name a config in this project");
                }
                row.TestConfigId = configId;
            }
            if (body.enabled is not null)
            {
                row.Enabled = body.enabled.Value;
            }

            await db.SaveChangesAsync(ct);
            return Results.Ok(RuleDto(row));
        }).RequireAuthorization();

        // DELETE /api/v2/alert-rules/{id} — 204 (events cascade with the rule).
        app.MapDelete("/api/v2/alert-rules/{id:guid}", async (
            Guid id,
            HttpContext ctx,
            NetworkerDbContext db,
            ProjectAccessChecker access,
            CancellationToken ct) =>
        {
            var row = await db.AlertRules.FirstOrDefaultAsync(r => r.RuleId == id, ct);
            if (row is null || !await access.HasRoleAsync(ctx, row.ProjectId, ProjectRole.Operator, ct))
            {
                return Results.NotFound();
            }

            db.AlertRules.Remove(row);
            await db.SaveChangesAsync(ct);
            return Results.NoContent();
        }).RequireAuthorization();
    }

    // ── Events ───────────────────────────────────────────────────────────────

    private static void MapEventEndpoints(IEndpointRouteBuilder app)
    {
        // GET /api/v2/projects/{projectId}/alert-events — newest first,
        // limit/offset paginated, optional ?rule_id= filter (member). Joined
        // with the rule so each row carries its threshold context.
        app.MapGet("/api/v2/projects/{projectId}/alert-events", async (
            string projectId,
            Guid? rule_id,
            int? limit,
            int? offset,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var take = Math.Clamp(limit ?? DefaultLimit, 1, MaxLimit);
            var skip = Math.Max(offset ?? 0, 0);

            var query = db.AlertEvents
                .AsNoTracking()
                .Where(e => e.Rule.ProjectId == projectId);

            if (rule_id is { } ruleId)
            {
                query = query.Where(e => e.RuleId == ruleId);
            }

            var rows = await query
                .OrderByDescending(e => e.FiredAt)
                .Skip(skip)
                .Take(take)
                .Select(e => new
                {
                    event_id = e.EventId,
                    rule_id = e.RuleId,
                    run_id = e.RunId,
                    fired_at = e.FiredAt,
                    state = e.State,
                    value = e.Value,
                    message = e.Message,
                    delivery_status = e.DeliveryStatus,
                    // Rule context so the history renders standalone.
                    metric = e.Rule.Metric,
                    comparator = e.Rule.Comparator,
                    threshold = e.Rule.Threshold,
                    test_config_id = e.Rule.TestConfigId,
                    channel_id = e.Rule.ChannelId,
                })
                .ToListAsync(ct);

            return Results.Ok(rows);
        }).RequireAuthorization(AuthPolicies.ProjectMember);
    }

    // ── Helpers / DTOs ───────────────────────────────────────────────────────

    /// <summary>
    /// Validate + normalize a channel config for its kind. Returns the JSON to
    /// store, or an error. On PATCH, a webhook secret equal to
    /// <see cref="SecretMask"/> keeps the previously stored secret.
    /// </summary>
    internal static (string? Config, string? Error) ValidateChannelConfig(
        string kind, JsonElement? config, string? existingConfig)
    {
        if (config is not { ValueKind: JsonValueKind.Object } cfg)
        {
            return (null, "config is required and must be an object");
        }

        if (kind == "webhook")
        {
            if (!cfg.TryGetProperty("url", out var urlEl)
                || urlEl.ValueKind != JsonValueKind.String
                || !Uri.TryCreate(urlEl.GetString(), UriKind.Absolute, out var uri)
                || (uri.Scheme != Uri.UriSchemeHttp && uri.Scheme != Uri.UriSchemeHttps))
            {
                return (null, "webhook config requires an absolute http(s) 'url'");
            }

            string? secret = null;
            if (cfg.TryGetProperty("secret", out var secretEl))
            {
                if (secretEl.ValueKind != JsonValueKind.String)
                {
                    return (null, "webhook 'secret' must be a string");
                }
                secret = secretEl.GetString();
                if (secret == SecretMask && existingConfig is not null)
                {
                    // Round-tripped mask → preserve the stored secret.
                    using var existing = JsonDocument.Parse(existingConfig);
                    secret = existing.RootElement.TryGetProperty("secret", out var old)
                        && old.ValueKind == JsonValueKind.String
                        ? old.GetString()
                        : null;
                }
            }

            var stored = string.IsNullOrEmpty(secret)
                ? JsonSerializer.Serialize(new { url = uri.ToString() })
                : JsonSerializer.Serialize(new { url = uri.ToString(), secret });
            return (stored, null);
        }

        // email
        if (!cfg.TryGetProperty("to", out var toEl) || toEl.ValueKind != JsonValueKind.Array)
        {
            return (null, "email config requires a 'to' array of addresses");
        }
        var recipients = toEl.EnumerateArray()
            .Where(e => e.ValueKind == JsonValueKind.String)
            .Select(e => e.GetString()!.Trim())
            .Where(a => a.Length > 0 && a.Contains('@'))
            .ToList();
        if (recipients.Count == 0)
        {
            return (null, "email config requires at least one valid address in 'to'");
        }

        return (JsonSerializer.Serialize(new { to = recipients }), null);
    }

    /// <summary>Wire shape for a channel; webhook secrets are masked on the way out.</summary>
    private static object ChannelDto(Data.Entities.AlertChannel c) => new
    {
        channel_id = c.ChannelId,
        project_id = c.ProjectId.TrimEnd(),
        kind = c.Kind,
        name = c.Name,
        config = MaskedConfig(c.Config),
        enabled = c.Enabled,
        created_at = c.CreatedAt,
    };

    private static JsonElement MaskedConfig(string configJson)
    {
        using var doc = JsonDocument.Parse(configJson);
        if (doc.RootElement.ValueKind != JsonValueKind.Object
            || !doc.RootElement.TryGetProperty("secret", out _))
        {
            return doc.RootElement.Clone();
        }

        var masked = doc.RootElement.EnumerateObject()
            .ToDictionary(
                p => p.Name,
                p => p.Name == "secret"
                    ? JsonSerializer.SerializeToElement(SecretMask)
                    : p.Value.Clone());
        return JsonSerializer.SerializeToElement(masked);
    }

    private static object RuleDto(Data.Entities.AlertRule r) => new
    {
        rule_id = r.RuleId,
        project_id = r.ProjectId.TrimEnd(),
        test_config_id = r.TestConfigId,
        metric = r.Metric,
        comparator = r.Comparator,
        threshold = r.Threshold,
        window_runs = r.WindowRuns,
        enabled = r.Enabled,
        channel_id = r.ChannelId,
        created_by = r.CreatedBy,
        created_at = r.CreatedAt,
    };

    // ── Request bodies (snake_case JSON) ─────────────────────────────────────

    /// <summary>Create/patch body for channels; PATCH applies only supplied fields.</summary>
    public sealed record ChannelRequest(
        string? kind,
        string? name,
        JsonElement? config,
        bool? enabled);

    /// <summary>Create/patch body for rules; PATCH applies only supplied fields.
    /// (test_config_id cannot be cleared back to project-wide via PATCH —
    /// recreate the rule for that.)</summary>
    public sealed record RuleRequest(
        Guid? test_config_id,
        string? metric,
        string? comparator,
        double? threshold,
        int? window_runs,
        Guid? channel_id,
        bool? enabled);
}
