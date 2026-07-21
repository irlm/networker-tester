using System.Text.Json;
using System.Text.Json.Serialization;
using Microsoft.AspNetCore.Mvc;
using Microsoft.EntityFrameworkCore;
using Npgsql;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Dispatch;
using Networker.Data;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// REST v2 <b>write</b> endpoints for test configs — the C# port of the Rust
/// <c>crates/networker-dashboard/src/api/test_configs.rs</c> create / patch /
/// delete / launch handlers (M1 ported only the reads). Request/response field
/// names are snake_case to match the Rust <c>CreateTestConfigRequest</c> /
/// <c>UpdateTestConfigRequest</c> / <c>LaunchRequest</c> and the
/// <c>networker_common::TestConfig</c> wire shapes so the existing frontend
/// consumes either backend unchanged.
///
/// <para>Polymorphic <c>endpoint</c> / <c>workload</c> / <c>methodology</c> are
/// carried as raw JSON (<see cref="JsonElement"/>) and stored verbatim in the
/// JSONB-as-text columns — this module does not re-model those shapes, exactly
/// like the read side. <c>endpoint_kind</c> is derived from <c>endpoint.kind</c>
/// (the Rust <c>#[serde(tag = "kind")]</c> discriminator), matching the Rust
/// <c>db::test_configs::create</c>.</para>
/// </summary>
public static class TestConfigWriteEndpoints
{
    private const int DefaultMaxDurationSecs = 900;

    public static IEndpointRouteBuilder MapTestConfigWriteEndpoints(this IEndpointRouteBuilder app)
    {
        // POST /api/v2/projects/{projectId}/test-configs — create. ProjectOperator.
        // Mirrors Rust create_handler: derive endpoint_kind from endpoint.kind,
        // store polymorphic blocks as JSONB, surface UNIQUE(project_id,name)
        // violations (23505) as 409.
        app.MapPost("/api/v2/projects/{projectId}/test-configs", async (
            string projectId,
            [FromBody] CreateTestConfigRequest req,
            HttpContext http,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var user = http.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }

            if (string.IsNullOrWhiteSpace(req.Name))
            {
                return ApiError.BadRequest("name is required");
            }
            if (req.Endpoint.ValueKind != JsonValueKind.Object)
            {
                return ApiError.BadRequest("endpoint is required");
            }
            if (req.Workload.ValueKind != JsonValueKind.Object)
            {
                return ApiError.BadRequest("workload is required");
            }

            var endpointKind = DeriveEndpointKind(req.Endpoint);
            if (endpointKind is null)
            {
                return ApiError.BadRequest("endpoint.kind is required");
            }

            var now = DateTime.UtcNow;
            var cfg = new Data.Entities.TestConfig
            {
                Id = Guid.NewGuid(),
                ProjectId = projectId,
                Name = req.Name,
                Description = req.Description,
                EndpointKind = endpointKind,
                EndpointRef = req.Endpoint.GetRawText(),
                Workload = req.Workload.GetRawText(),
                Methodology = req.Methodology is { ValueKind: not JsonValueKind.Null } m
                    ? m.GetRawText()
                    : null,
                MaxDurationSecs = req.MaxDurationSecs ?? DefaultMaxDurationSecs,
                CreatedBy = user.UserId,
                CreatedAt = now,
                UpdatedAt = now,
            };

            db.TestConfigs.Add(cfg);
            try
            {
                await db.SaveChangesAsync(ct);
            }
            catch (DbUpdateException ex) when (IsUniqueViolation(ex))
            {
                // UNIQUE(project_id, name) → 409, matching Rust's 23505 mapping.
                return ApiError.Conflict("a test config with this name already exists");
            }

            return Results.Ok(ToDto(cfg));
        }).RequireAuthorization(AuthPolicies.ProjectOperator);

        // PATCH /api/v2/test-configs/{id} — partial update. Auth only (flat route
        // has no {projectId} for the project-scope policy — same follow-up caveat
        // as the M1 read side). Mirrors Rust patch_handler: every field optional;
        // absent fields are left unchanged. Description / methodology / baseline
        // use the double-Option semantics (present-null clears; absent no-ops).
        app.MapPatch("/api/v2/test-configs/{id:guid}", async (
            Guid id,
            [FromBody] UpdateTestConfigRequest req,
            HttpContext ctx,
            NetworkerDbContext db,
            ProjectAccessChecker access,
            CancellationToken ct) =>
        {
            var cfg = await db.TestConfigs.FirstOrDefaultAsync(c => c.Id == id, ct);
            // Row-level project authorization (flat route): require Operator on the
            // config's project; 404 on absent or no-access (no existence oracle).
            if (cfg is null || !await access.HasRoleAsync(ctx, cfg.ProjectId, ProjectRole.Operator, ct))
            {
                return Results.NotFound();
            }

            if (req.Name is not null)
            {
                cfg.Name = req.Name;
            }
            if (req.Description.IsSet)
            {
                cfg.Description = req.Description.Value;
            }
            if (req.Endpoint is { ValueKind: JsonValueKind.Object } ep)
            {
                var kind = DeriveEndpointKind(ep);
                if (kind is null)
                {
                    return ApiError.BadRequest("endpoint.kind is required");
                }
                cfg.EndpointRef = ep.GetRawText();
                cfg.EndpointKind = kind;
            }
            if (req.Workload is { ValueKind: JsonValueKind.Object } wl)
            {
                cfg.Workload = wl.GetRawText();
            }
            if (req.Methodology.IsSet)
            {
                cfg.Methodology = req.Methodology.Value is { ValueKind: not JsonValueKind.Null } mv
                    ? mv.GetRawText()
                    : null;
            }
            if (req.BaselineRunId.IsSet)
            {
                cfg.BaselineRunId = req.BaselineRunId.Value;
            }
            if (req.MaxDurationSecs is int md)
            {
                cfg.MaxDurationSecs = md;
            }

            cfg.UpdatedAt = DateTime.UtcNow;

            try
            {
                await db.SaveChangesAsync(ct);
            }
            catch (DbUpdateException ex) when (IsUniqueViolation(ex))
            {
                return ApiError.Conflict("a test config with this name already exists");
            }

            return Results.Ok(ToDto(cfg));
        }).RequireAuthorization();

        // DELETE /api/v2/test-configs/{id} — 204 on success, 404 if absent.
        // Mirrors Rust delete_handler.
        app.MapDelete("/api/v2/test-configs/{id:guid}", async (
            Guid id,
            HttpContext ctx,
            NetworkerDbContext db,
            ProjectAccessChecker access,
            CancellationToken ct) =>
        {
            // Resolve the config's project and require Operator before deleting
            // (flat route). 404 on absent or no-access.
            var owner = await db.TestConfigs.AsNoTracking()
                .Where(c => c.Id == id)
                .Select(c => c.ProjectId)
                .FirstOrDefaultAsync(ct);
            if (owner is null || !await access.HasRoleAsync(ctx, owner, ProjectRole.Operator, ct))
            {
                return Results.NotFound();
            }

            var affected = await db.TestConfigs
                .Where(c => c.Id == id)
                .ExecuteDeleteAsync(ct);

            return affected > 0 ? Results.NoContent() : Results.NotFound();
        }).RequireAuthorization();

        // POST /api/v2/test-configs/{id}/launch — create a queued run + dispatch.
        // Auth only. Mirrors Rust launch_handler, delegating the create+dispatch
        // to the M3 IRunDispatcher. tester_id is threaded through so dispatch
        // prefers the agent BOUND to that project_tester (LaunchRequest.tester_id
        // — a project_tester id, previously dropped). Returns 200 with the FULL serialized test_run row, re-read
        // after the dispatch attempt (the frontend inserts this response straight
        // into the runs list; status may already be running/provisioning).
        app.MapPost("/api/v2/test-configs/{id:guid}/launch", async (
            Guid id,
            [FromBody] LaunchRequest? req,
            HttpContext http,
            IRunDispatcher dispatcher,
            NetworkerDbContext db,
            ProjectAccessChecker access,
            CancellationToken ct) =>
        {
            var user = http.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }

            // Row-level project authorization (flat route): the caller must be an
            // Operator on the config's project to launch a run in it (else any
            // authenticated user could spend another tenant's cloud budget).
            var owner = await db.TestConfigs.AsNoTracking()
                .Where(c => c.Id == id)
                .Select(c => c.ProjectId)
                .FirstOrDefaultAsync(ct);
            if (owner is null || !await access.HasRoleAsync(http, owner, ProjectRole.Operator, ct))
            {
                return Results.NotFound();
            }

            // An explicitly-pinned tester_id must belong to the SAME project as
            // the config, else it must not influence routing (project-isolation
            // audit §6c / P2). A foreign or non-existent tester is rejected.
            if (req?.TesterId is Guid pinnedTesterId)
            {
                var testerProject = await db.ProjectTesters.AsNoTracking()
                    .Where(t => t.TesterId == pinnedTesterId)
                    .Select(t => t.ProjectId)
                    .FirstOrDefaultAsync(ct);
                if (testerProject is null || !string.Equals(testerProject, owner, StringComparison.Ordinal))
                {
                    return Results.BadRequest(new
                    {
                        error = "tester_id does not belong to this config's project",
                    });
                }
            }

            Guid runId;
            try
            {
                runId = await dispatcher.LaunchAsync(
                    id, req?.ComparisonGroupId, req?.TesterId, user, ct);
            }
            catch (RunDispatchNotFoundException)
            {
                return Results.NotFound();
            }

            var run = await db.TestRuns
                .AsNoTracking()
                .FirstOrDefaultAsync(r => r.Id == runId, ct);

            return run is null
                ? Results.NotFound()
                : Results.Ok(TestRunResponse.ToDto(run));
        }).RequireAuthorization();

        return app;
    }

    // ── Request DTOs (snake_case bodies) ──────────────────────────────────────

    /// <summary>Mirrors Rust <c>CreateTestConfigRequest</c>.</summary>
    public sealed record CreateTestConfigRequest(
        [property: JsonPropertyName("name")] string Name,
        [property: JsonPropertyName("description")] string? Description,
        [property: JsonPropertyName("endpoint")] JsonElement Endpoint,
        [property: JsonPropertyName("workload")] JsonElement Workload,
        [property: JsonPropertyName("methodology")] JsonElement? Methodology,
        [property: JsonPropertyName("max_duration_secs")] int? MaxDurationSecs);

    /// <summary>
    /// Mirrors Rust <c>UpdateTestConfigRequest</c>. The double-Option fields
    /// (<c>description</c>, <c>methodology</c>, <c>baseline_run_id</c>) use
    /// <see cref="OptionalField{T}"/> so "absent" (no-op) is distinguishable from
    /// "present but null" (clear), matching Rust's <c>Option&lt;Option&lt;T&gt;&gt;</c>.
    /// </summary>
    public sealed class UpdateTestConfigRequest
    {
        [JsonPropertyName("name")]
        public string? Name { get; set; }

        [JsonPropertyName("description")]
        public OptionalField<string?> Description { get; set; }

        [JsonPropertyName("endpoint")]
        public JsonElement? Endpoint { get; set; }

        [JsonPropertyName("workload")]
        public JsonElement? Workload { get; set; }

        [JsonPropertyName("methodology")]
        public OptionalField<JsonElement?> Methodology { get; set; }

        [JsonPropertyName("baseline_run_id")]
        public OptionalField<Guid?> BaselineRunId { get; set; }

        [JsonPropertyName("max_duration_secs")]
        public int? MaxDurationSecs { get; set; }
    }

    /// <summary>Mirrors Rust <c>LaunchRequest</c>. <c>tester_id</c> seeds
    /// <c>test_run.tester_id</c> (a project_tester FK — dispatch prefers the
    /// agent bound to that tester); the comparison group is carried through to
    /// the run.</summary>
    public sealed record LaunchRequest(
        [property: JsonPropertyName("tester_id")] Guid? TesterId,
        [property: JsonPropertyName("comparison_group_id")] Guid? ComparisonGroupId);

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// <summary>
    /// Derive the <c>endpoint_kind</c> column from the polymorphic endpoint's
    /// <c>kind</c> discriminator (Rust <c>#[serde(tag = "kind")]</c>). Returns
    /// null when absent/blank.
    /// </summary>
    private static string? DeriveEndpointKind(JsonElement endpoint)
    {
        if (endpoint.ValueKind == JsonValueKind.Object &&
            endpoint.TryGetProperty("kind", out var kind) &&
            kind.ValueKind == JsonValueKind.String)
        {
            var value = kind.GetString();
            return string.IsNullOrWhiteSpace(value) ? null : value;
        }
        return null;
    }

    private static bool IsUniqueViolation(DbUpdateException ex)
        => ex.InnerException is PostgresException { SqlState: PostgresErrorCodes.UniqueViolation };

    private static object ToDto(Data.Entities.TestConfig c) => new
    {
        id = c.Id,
        project_id = c.ProjectId,
        name = c.Name,
        description = c.Description,
        endpoint = RawJson(c.EndpointRef),
        workload = RawJson(c.Workload),
        methodology = RawJsonOrNull(c.Methodology),
        baseline_run_id = c.BaselineRunId,
        max_duration_secs = c.MaxDurationSecs,
        created_by = c.CreatedBy,
        created_at = c.CreatedAt,
        updated_at = c.UpdatedAt,
    };

    private static object RawJson(string value)
    {
        try
        {
            using var doc = JsonDocument.Parse(value);
            return doc.RootElement.Clone();
        }
        catch (JsonException)
        {
            return value;
        }
    }

    private static object? RawJsonOrNull(string? value)
        => value is null ? null : RawJson(value);
}

/// <summary>
/// A PATCH field that distinguishes "absent from the body" (no-op) from "present
/// but null" (clear to null) — the C# analogue of Rust's
/// <c>Option&lt;Option&lt;T&gt;&gt;</c> with <c>#[serde(default)]</c>. When the
/// JSON key is omitted, <see cref="IsSet"/> is false and the value is left
/// untouched. When present (including explicit <c>null</c>), <see cref="IsSet"/>
/// is true and <see cref="Value"/> holds the deserialized (possibly null) value.
/// </summary>
[JsonConverter(typeof(OptionalFieldJsonConverterFactory))]
public readonly struct OptionalField<T>
{
    public bool IsSet { get; }
    public T Value { get; }

    public OptionalField(T value)
    {
        IsSet = true;
        Value = value;
    }
}

/// <summary>
/// Deserializes a present JSON key (any value, including <c>null</c>) into an
/// <see cref="OptionalField{T}"/> with <c>IsSet = true</c>. Absent keys never
/// reach the converter, so the default <c>IsSet = false</c> stands. Serialization
/// is not required (these are inbound request DTOs) and writes the inner value.
/// </summary>
public sealed class OptionalFieldJsonConverterFactory : JsonConverterFactory
{
    public override bool CanConvert(Type typeToConvert)
        => typeToConvert.IsGenericType &&
           typeToConvert.GetGenericTypeDefinition() == typeof(OptionalField<>);

    public override JsonConverter CreateConverter(Type typeToConvert, JsonSerializerOptions options)
    {
        var inner = typeToConvert.GetGenericArguments()[0];
        var converterType = typeof(OptionalFieldJsonConverter<>).MakeGenericType(inner);
        return (JsonConverter)Activator.CreateInstance(converterType)!;
    }
}

internal sealed class OptionalFieldJsonConverter<T> : JsonConverter<OptionalField<T>>
{
    public override OptionalField<T> Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
    {
        var value = JsonSerializer.Deserialize<T>(ref reader, options);
        return new OptionalField<T>(value!);
    }

    public override void Write(Utf8JsonWriter writer, OptionalField<T> value, JsonSerializerOptions options)
        => JsonSerializer.Serialize(writer, value.Value, options);
}
