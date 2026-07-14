using System.Text.Json.Serialization;
using Microsoft.AspNetCore.Mvc;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Phase-2 M5: TEST VISIBILITY RULES — the C# port of the Rust
/// <c>crates/networker-dashboard/src/api/visibility.rs</c> (list / add /
/// remove; all project-admin).
///
/// A rule grants a user (or every member, when <c>user_id</c> is null) explicit
/// visibility of one resource when the project's <c>settings.test_visibility</c>
/// is "explicit". This module is only the rule CRUD — the filter application
/// (Rust <c>visible_resources</c>) lives with the list endpoints that consume it.
///
/// Rust does not validate <c>resource_type</c> here (any string ≤ 20 chars),
/// so neither does this port; <c>resource_id</c> is required (serde would 400
/// a missing field, mirrored with an explicit empty-Guid check).
/// </summary>
public static class VisibilityRulesEndpoints
{
    public static IEndpointRouteBuilder MapVisibilityRulesEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/projects/{projectId}/visibility-rules — list (project admin).
        // Mirrors Rust list_rules → db::visibility::list_rules: bare array,
        // LEFT JOIN dash_user for the subject's email, INNER JOIN for the
        // creator's email, newest first.
        app.MapGet("/api/projects/{projectId}/visibility-rules", async (
            string projectId,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var rules = await (
                from r in db.TestVisibilityRules.AsNoTracking()
                join cb in db.DashUsers on r.CreatedBy equals cb.UserId
                join u in db.DashUsers on r.UserId equals (Guid?)u.UserId into gj
                from u in gj.DefaultIfEmpty()
                where r.ProjectId == projectId
                orderby r.CreatedAt descending
                select new VisibilityRuleRow(
                    r.RuleId,
                    r.ProjectId,
                    r.UserId,
                    r.ResourceType,
                    r.ResourceId,
                    r.CreatedBy,
                    r.CreatedAt,
                    u != null ? u.Email : null,
                    cb.Email ?? string.Empty)).ToListAsync(ct);

            return Results.Ok(rules);
        }).RequireAuthorization(AuthPolicies.ProjectAdmin);

        // POST /api/projects/{projectId}/visibility-rules — add (project admin).
        // Mirrors Rust add_rule → db::visibility::add_rule. Returns { rule_id }.
        app.MapPost("/api/projects/{projectId}/visibility-rules", async (
            string projectId,
            [FromBody] AddVisibilityRuleRequest req,
            HttpContext ctx,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            var user = ctx.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }

            // serde: resource_type and resource_id are required fields → 400.
            if (string.IsNullOrEmpty(req.ResourceType) || req.ResourceId == Guid.Empty)
            {
                return Results.BadRequest(new { error = "resource_type and resource_id are required" });
            }

            var rule = new TestVisibilityRule
            {
                RuleId = Guid.NewGuid(),
                ProjectId = projectId,
                UserId = req.UserId,
                ResourceType = req.ResourceType,
                ResourceId = req.ResourceId,
                CreatedBy = user.UserId,
            };
            db.TestVisibilityRules.Add(rule);
            await db.SaveChangesAsync(ct);

            return Results.Ok(new { rule_id = rule.RuleId });
        }).RequireAuthorization(AuthPolicies.ProjectAdmin);

        // DELETE /api/projects/{projectId}/visibility-rules/{ruleId} — remove
        // (project admin). Mirrors Rust remove_rule: scoped to the project;
        // { deleted: true } regardless of affected-row count (Rust ignores it),
        // so a cross-tenant ruleId probe learns nothing.
        app.MapDelete("/api/projects/{projectId}/visibility-rules/{ruleId:guid}", async (
            string projectId,
            Guid ruleId,
            NetworkerDbContext db,
            CancellationToken ct) =>
        {
            await db.TestVisibilityRules
                .Where(r => r.RuleId == ruleId && r.ProjectId == projectId)
                .ExecuteDeleteAsync(ct);

            return Results.Ok(new { deleted = true });
        }).RequireAuthorization(AuthPolicies.ProjectAdmin);

        return app;
    }
}

/// <summary>POST visibility-rules body — Rust <c>AddRuleRequest</c>.</summary>
public sealed record AddVisibilityRuleRequest(
    [property: JsonPropertyName("user_id")] Guid? UserId,
    [property: JsonPropertyName("resource_type")] string? ResourceType,
    [property: JsonPropertyName("resource_id")] Guid ResourceId);

/// <summary>One row of GET visibility-rules — the Rust <c>VisibilityRuleRow</c> serde shape.</summary>
public sealed record VisibilityRuleRow(
    Guid rule_id,
    string project_id,
    Guid? user_id,
    string resource_type,
    Guid resource_id,
    Guid created_by,
    DateTime created_at,
    string? user_email,
    string created_by_email);
