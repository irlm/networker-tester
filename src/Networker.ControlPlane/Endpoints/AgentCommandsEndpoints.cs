using System.IdentityModel.Tokens.Jwt;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.Json.Serialization;
using Microsoft.EntityFrameworkCore;
using Microsoft.IdentityModel.Tokens;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Realtime;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Agent-command orchestration endpoints — the C# port of the Rust dashboard's
/// <c>api/agent_commands.rs</c> (dispatch / fetch / SSE stream) plus the
/// dispatch service from <c>agent_dispatch.rs</c> (INSERT pending row, mint a
/// short-lived per-command JWT, push <c>ControlMessage::Command</c> over the
/// agent socket).
///
/// <para><b>Command token.</b> Rust mints a verb-scoped JWT via
/// <c>auth::commands::mint_command_token</c> with claims
/// <c>{sub: agent_id, aud: config_id|"", scope: [verb], exp, iat}</c>, HS256
/// over the raw DASHBOARD_JWT_SECRET bytes, lifetime
/// <c>max(timeout_secs + 60, 300)</c>. That exact claim set + lifetime rule is
/// reproduced here using <see cref="JwtTokenService.SigningKey"/> (the same
/// secret the user-auth tokens use), so a token minted by this control plane
/// validates against the Rust agent's <c>validate_command_token</c> unchanged.
/// <see cref="JwtTokenService.CreateToken"/> itself is NOT reused — it mints the
/// user-session claim set (email/role/24 h TTL), which is the wrong shape for a
/// command grant.</para>
///
/// <para><b>Offline agent.</b> The Rust dispatch surfaces a send failure as
/// 502 Bad Gateway; here an offline agent is reported as 409 Conflict (the
/// resource exists, its connection state does not allow the operation). In both
/// implementations the pending row is stamped <c>status='error'</c> with an
/// <c>agent_not_connected</c> error_message + finished_at so REST consumers
/// still see the attempt.</para>
///
/// <para><b>SSE stream divergence (documented).</b> Rust streams per-line
/// <c>log</c> events by polling the <c>service_log</c> table (a separate logs
/// DB) for rows tagged with the command_id, because its agent hub persists each
/// <c>CommandLog</c> frame there. The C# <c>AgentProtocolHub.OnCommandLog</c>
/// only stamps <c>agent_command.started_at</c> — log lines are NOT persisted
/// (service_log is not part of the EF model). The stream therefore polls the
/// <c>agent_command</c> row once per second and emits <c>status</c> events on
/// each transition (pending → running → terminal, where "running" is inferred from
/// started_at), then the same final <c>done</c> event Rust sends
/// (<c>{command_id, status, error_message}</c>), then closes. Heartbeat
/// comments are written after 15 s of silence; the loop honours request
/// cancellation.</para>
/// </summary>
public static class AgentCommandsEndpoints
{
    /// <summary>Rust MIN_TOKEN_LIFETIME_SECS — floor on the command token TTL.</summary>
    public const long MinTokenLifetimeSecs = 300;

    /// <summary>Rust TOKEN_LIFETIME_BUFFER_SECS — slack on top of timeout_secs.</summary>
    public const long TokenLifetimeBufferSecs = 60;

    /// <summary>Rust DispatchBody default when timeout_secs is absent.</summary>
    public const long DefaultTimeoutSecs = 60;

    /// <summary>Poll cadence for the command stream (Rust ticks at 500 ms).</summary>
    public static readonly TimeSpan StreamPollInterval = TimeSpan.FromSeconds(1);

    /// <summary>Max silence before a heartbeat comment is emitted.</summary>
    public static readonly TimeSpan HeartbeatInterval = TimeSpan.FromSeconds(15);

    public static IEndpointRouteBuilder MapAgentCommandsEndpoints(this IEndpointRouteBuilder app)
    {
        // POST /api/projects/{projectId}/agents/{agentId}/commands — dispatch a
        // typed command (project operator). Mirrors Rust dispatch(): agent must
        // live in this project (404), insert pending agent_command row, mint the
        // command JWT, push over the live connection. 202 {command_id, agent_id, verb}.
        app.MapPost("/api/projects/{projectId}/agents/{agentId:guid}/commands", async (
            string projectId,
            Guid agentId,
            DispatchCommandRequest body,
            HttpContext ctx,
            NetworkerDbContext db,
            AgentConnectionRegistry registry,
            JwtTokenService tokens,
            ILoggerFactory loggerFactory,
            CancellationToken ct) =>
        {
            var user = ctx.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }

            if (string.IsNullOrWhiteSpace(body.Verb))
            {
                return Results.BadRequest(new { error = "verb is required" });
            }

            var agentInProject = await db.Agents
                .AsNoTracking()
                .AnyAsync(a => a.AgentId == agentId && a.ProjectId == projectId, ct);
            if (!agentInProject)
            {
                return Results.NotFound(new { error = $"agent {agentId} not in this project" });
            }

            var commandId = Guid.NewGuid();
            var timeoutSecs = body.TimeoutSecs ?? DefaultTimeoutSecs;
            var argsJson = body.Args is { ValueKind: not JsonValueKind.Undefined } args
                ? args.GetRawText()
                : "{}";

            // 1. INSERT pending row (Rust agent_commands::insert_pending).
            db.AgentCommands.Add(new AgentCommand
            {
                CommandId = commandId,
                AgentId = agentId,
                ConfigId = body.ConfigId,
                Verb = body.Verb,
                Args = argsJson,
                Status = "pending",
                CreatedBy = user.UserId,
                CreatedAt = DateTime.UtcNow,
            });
            await db.SaveChangesAsync(ct);

            // 2. Mint the short-lived, verb-scoped command JWT.
            var lifetime = CommandTokenLifetimeSecs(timeoutSecs);
            var token = MintCommandToken(tokens, agentId, body.ConfigId, body.Verb, lifetime);

            // 3. Push the Command envelope over the agent's live connection.
            using var argsDoc = JsonDocument.Parse(argsJson);
            var envelope = new CommandMessage(
                commandId, body.ConfigId, token, body.Verb, argsDoc.RootElement.Clone(), timeoutSecs);

            var sent = await registry.SendCommandAsync(agentId, envelope, ct);
            var logger = loggerFactory.CreateLogger("Networker.ControlPlane.AgentCommands");
            if (!sent)
            {
                // Rust mark_dispatch_error: status='error', error_message,
                // finished_at=now(), started_at left NULL (it never ran).
                await db.AgentCommands
                    .Where(c => c.CommandId == commandId)
                    .ExecuteUpdateAsync(s => s
                        .SetProperty(c => c.Status, "error")
                        .SetProperty(c => c.ErrorMessage, "agent_not_connected: no live connection")
                        .SetProperty(c => c.FinishedAt, DateTime.UtcNow), ct);

                logger.LogWarning(
                    "Dispatch of {Verb} to agent {AgentId} failed: not connected (command {CommandId})",
                    body.Verb, agentId, commandId);

                // Rust returns 502; 409 Conflict here (agent offline is a state
                // conflict, not an upstream proxy failure). See class doc.
                return Results.Conflict(new
                {
                    error = $"dispatch to agent {agentId} failed: agent not connected",
                    command_id = commandId,
                });
            }

            logger.LogInformation(
                "Dispatched agent command {CommandId} verb={Verb} to {AgentId}",
                commandId, body.Verb, agentId);

            return Results.Accepted(
                $"/api/projects/{projectId}/commands/{commandId}",
                new { command_id = commandId, agent_id = agentId, verb = body.Verb });
        })
        .RequireAuthorization(AuthPolicies.ProjectOperator);

        // GET /api/projects/{projectId}/commands/{commandId} — current row
        // (project member). Mirrors Rust fetch(): 404 when the command doesn't
        // exist OR its agent is not in this project (no existence oracle).
        app.MapGet("/api/projects/{projectId}/commands/{commandId:guid}", async (
            string projectId, Guid commandId, NetworkerDbContext db, CancellationToken ct) =>
        {
            var row = await FetchInProjectAsync(db, projectId, commandId, ct);
            return row is null
                ? Results.NotFound(new { error = $"command {commandId} not found" })
                : Results.Ok(ShapeCommand(row));
        })
        .RequireAuthorization(AuthPolicies.ProjectMember);

        // GET /api/projects/{projectId}/commands/{commandId}/stream — SSE
        // (project member). Polls the agent_command row; emits `status` events
        // on transitions and a final `done` event once finished_at is set.
        app.MapGet("/api/projects/{projectId}/commands/{commandId:guid}/stream", async (
            string projectId, Guid commandId, HttpContext ctx, NetworkerDbContext db) =>
        {
            var initial = await FetchInProjectAsync(db, projectId, commandId, ctx.RequestAborted);
            if (initial is null)
            {
                return Results.NotFound(new { error = $"command {commandId} not found" });
            }

            var response = ctx.Response;
            response.Headers.ContentType = "text/event-stream";
            response.Headers.CacheControl = "no-cache";
            response.Headers["X-Accel-Buffering"] = "no";

            var ct = ctx.RequestAborted;
            var lastWrite = DateTimeOffset.UtcNow;
            string? lastEffective = null;
            var row = initial;

            try
            {
                while (true)
                {
                    var effective = EffectiveStatus(row.Status, row.StartedAt, row.FinishedAt);
                    var wrote = false;

                    if (effective != lastEffective)
                    {
                        var payload = JsonSerializer.Serialize(new
                        {
                            command_id = commandId,
                            status = effective,
                            started_at = row.StartedAt,
                            finished_at = row.FinishedAt,
                        });
                        await response.WriteAsync(ServerSentEvents.FormatEvent("status", payload), ct);
                        lastEffective = effective;
                        wrote = true;
                    }

                    if (row.FinishedAt is not null)
                    {
                        // Terminal: emit the Rust DoneEvent shape and close.
                        var done = JsonSerializer.Serialize(new
                        {
                            command_id = commandId,
                            status = row.Status,
                            error_message = row.ErrorMessage,
                        });
                        await response.WriteAsync(ServerSentEvents.FormatEvent("done", done), ct);
                        await response.Body.FlushAsync(ct);
                        break;
                    }

                    if (wrote)
                    {
                        await response.Body.FlushAsync(ct);
                        lastWrite = DateTimeOffset.UtcNow;
                    }
                    else if (DateTimeOffset.UtcNow - lastWrite >= HeartbeatInterval)
                    {
                        await response.WriteAsync(ServerSentEvents.FormatComment("tick"), ct);
                        await response.Body.FlushAsync(ct);
                        lastWrite = DateTimeOffset.UtcNow;
                    }

                    await Task.Delay(StreamPollInterval, ct);

                    var next = await db.AgentCommands
                        .AsNoTracking()
                        .FirstOrDefaultAsync(c => c.CommandId == commandId, ct);
                    if (next is null)
                    {
                        // Row deleted mid-stream (shouldn't happen) — close politely.
                        var gone = JsonSerializer.Serialize(new
                        {
                            command_id = commandId,
                            status = "unknown",
                            error_message = "command row disappeared",
                        });
                        await response.WriteAsync(ServerSentEvents.FormatEvent("done", gone), ct);
                        await response.Body.FlushAsync(ct);
                        break;
                    }

                    row = next;
                }
            }
            catch (OperationCanceledException)
            {
                // Client went away — normal SSE termination.
            }

            return Results.Empty;
        })
        .RequireAuthorization(AuthPolicies.ProjectMember);

        return app;
    }

    /// <summary>
    /// Token TTL rule — Rust <c>agent_dispatch</c>:
    /// <c>(timeout_secs + TOKEN_LIFETIME_BUFFER_SECS).max(MIN_TOKEN_LIFETIME_SECS)</c>.
    /// </summary>
    public static long CommandTokenLifetimeSecs(long timeoutSecs)
        => Math.Max(timeoutSecs + TokenLifetimeBufferSecs, MinTokenLifetimeSecs);

    /// <summary>
    /// Mint the per-command JWT with the exact Rust <c>CommandClaims</c> shape:
    /// <c>sub</c> = agent UUID, <c>aud</c> = config UUID or empty string,
    /// <c>scope</c> = [verb], <c>exp</c>/<c>iat</c> unix seconds, HS256 over the
    /// shared DASHBOARD_JWT_SECRET (via <see cref="JwtTokenService.SigningKey"/>).
    /// </summary>
    public static string MintCommandToken(
        JwtTokenService tokens, Guid agentId, Guid? configId, string verb, long lifetimeSecs)
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();

        var handler = new JwtSecurityTokenHandler { SetDefaultTimesOnTokenCreation = false };
        var creds = new SigningCredentials(tokens.SigningKey, SecurityAlgorithms.HmacSha256);
        var token = new JwtSecurityToken(signingCredentials: creds);

        // Hand-build the payload so scope stays a JSON array and exp/iat stay
        // JSON numbers (serde-compatible), mirroring JwtTokenService.CreateToken.
        var payload = token.Payload;
        payload["sub"] = agentId.ToString();
        payload["aud"] = configId?.ToString() ?? string.Empty;
        payload["scope"] = new List<string> { verb };
        payload["exp"] = now + lifetimeSecs;
        payload["iat"] = now;

        return handler.WriteToken(token);
    }

    /// <summary>
    /// Presentation status for the stream: a non-terminal row with started_at
    /// stamped is shown as "running" (the C# hub stamps started_at on the first
    /// CommandLog frame — the closest observable analogue of the Rust log tail).
    /// Terminal rows report the stored status verbatim (ok/error/timeout/cancelled).
    /// </summary>
    public static string EffectiveStatus(string status, DateTime? startedAt, DateTime? finishedAt)
        => finishedAt is null && startedAt is not null && status == "pending" ? "running" : status;

    /// <summary>
    /// Load a command row iff its agent belongs to <paramref name="projectId"/> —
    /// the EF mirror of Rust's fetch_by_id + ensure_agent_in_project pair
    /// (collapsing both misses into null → 404).
    /// </summary>
    private static Task<AgentCommand?> FetchInProjectAsync(
        NetworkerDbContext db, string projectId, Guid commandId, CancellationToken ct)
        => db.AgentCommands
            .AsNoTracking()
            .FirstOrDefaultAsync(c => c.CommandId == commandId
                && db.Agents.Any(a => a.AgentId == c.AgentId && a.ProjectId == projectId), ct);

    /// <summary>Snake_case row shape — Rust <c>AgentCommandRow</c> field-for-field.</summary>
    private static object ShapeCommand(AgentCommand c) => new
    {
        command_id = c.CommandId,
        agent_id = c.AgentId,
        config_id = c.ConfigId,
        verb = c.Verb,
        args = ParseJson(c.Args),
        status = c.Status,
        result = ParseJson(c.Result),
        error_message = c.ErrorMessage,
        created_by = c.CreatedBy,
        created_at = c.CreatedAt,
        started_at = c.StartedAt,
        finished_at = c.FinishedAt,
    };

    private static JsonNode? ParseJson(string? raw)
    {
        if (string.IsNullOrWhiteSpace(raw))
        {
            return null;
        }

        try
        {
            return JsonNode.Parse(raw);
        }
        catch (JsonException)
        {
            return null;
        }
    }
}

/// <summary>
/// POST dispatch body — Rust <c>DispatchBody</c>: verb required, args default
/// <c>{}</c>, config_id optional, timeout_secs default 60.
/// </summary>
public sealed record DispatchCommandRequest(
    [property: JsonPropertyName("verb")] string? Verb,
    [property: JsonPropertyName("args")] JsonElement? Args,
    [property: JsonPropertyName("config_id")] Guid? ConfigId,
    [property: JsonPropertyName("timeout_secs")] long? TimeoutSecs);
