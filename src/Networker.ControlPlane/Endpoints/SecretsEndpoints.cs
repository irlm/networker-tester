using Networker.ControlPlane.Auth;
using Npgsql;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// <b>GET /api/admin/secrets</b> — how old each operational secret is, and which
/// ones are overdue. Platform-admin only.
///
/// <para>This endpoint NEVER returns secret material, and never a hash or prefix
/// of any. It returns identity + age + status, nothing else. That is a deliberate
/// split: knowing a secret is 400 days old is what an operator needs, and it
/// carries none of the risk of exposing the value. Rotation itself lives in
/// <c>scripts/rotate-secrets.sh</c>, run by an operator — putting a rotate button
/// on an internet-facing control plane would turn any single compromise (XSS, a
/// stolen operator token, an auth bypass) into total credential compromise, and
/// rotation is far too rare for that trade to pay.</para>
///
/// <para>Why it exists: on 2026-08-24 a storage-account key was found in
/// plaintext in a world-readable script on the prod VM and nobody could say how
/// old it was, because nothing recorded rotations. The same shape as the backup
/// gap found the same day — the information needed to notice existed nowhere.</para>
///
/// <para>The inventory is STATIC and the rotation table is a left join, so a
/// secret that has never been rotated still appears, as <c>never</c>. That is the
/// case that matters most, and a design keyed off the table alone would hide it.</para>
/// </summary>
public static class SecretsEndpoints
{
    /// <summary>Days before the deadline at which a secret starts reading "due".</summary>
    internal const int DueWindowDays = 14;

    internal sealed record SecretSpec(
        string Key,
        string Name,
        string Description,
        int MaxAgeDays,
        bool Automated,
        string? Risk);

    /// <summary>
    /// Every operational secret this deployment has. Static on purpose: the list
    /// is a statement of what SHOULD be tracked, so a secret nobody has ever
    /// rotated cannot quietly be absent from the panel.
    /// </summary>
    internal static readonly IReadOnlyList<SecretSpec> Inventory =
    [
        new("jwt-secret", "Session signing key",
            "DASHBOARD_JWT_SECRET — signs the API tokens every session presents.",
            90, true,
            "Rotating signs every user out; they log back in normally."),

        new("credential-key", "Credential encryption key",
            "DASHBOARD_CREDENTIAL_KEY — encrypts stored cloud credentials at rest.",
            365, false,
            "This is a DATA-ENCRYPTION key, not a password. Replacing it without "
            + "re-encrypting makes every stored cloud credential permanently "
            + "unreadable, so rotation is not automated — see "
            + "docs/secret-rotation.md for the re-encryption procedure."),

        new("db-password", "Database password",
            "The password inside DASHBOARD_DB_URL_NPGSQL.",
            180, true,
            "Needs a control-plane restart to take effect."),

        new("github-token", "Canary GitHub token",
            "CANARY_GITHUB_TOKEN — the PAT the canary uses against the GitHub API.",
            90, true, null),

        new("storage-key", "Backup storage account key",
            "Access keys on the Azure storage account holding database backups.",
            90, true,
            "Backups authenticate with the VM's managed identity, so rotating "
            + "these keys does not interrupt them."),
    ];

    /// <summary>
    /// never → no recorded rotation; overdue → past the deadline; due → inside
    /// the last <see cref="DueWindowDays"/> before it; ok → otherwise. Pure, so
    /// the boundaries are unit-tested rather than eyeballed in the UI.
    /// </summary>
    internal static string StatusFor(int? ageDays, int maxAgeDays)
    {
        if (ageDays is null)
        {
            return "never";
        }
        if (ageDays > maxAgeDays)
        {
            return "overdue";
        }
        return ageDays >= maxAgeDays - DueWindowDays ? "due" : "ok";
    }

    public static IEndpointRouteBuilder MapSecretsEndpoints(this IEndpointRouteBuilder app)
    {
        app.MapGet("/api/admin/secrets", async (
            HttpContext ctx,
            NpgsqlDataSource dataSource,
            CancellationToken ct) =>
        {
            var user = ctx.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }
            if (!user.IsPlatformAdmin)
            {
                return Results.StatusCode(StatusCodes.Status403Forbidden);
            }

            var rotated = new Dictionary<string, (DateTime At, string? By)>(StringComparer.Ordinal);
            try
            {
                await using var conn = await dataSource.OpenConnectionAsync(ct);
                await using var cmd = new NpgsqlCommand(
                    "SELECT secret_key, rotated_at, rotated_by FROM secret_rotation", conn);
                await using var reader = await cmd.ExecuteReaderAsync(ct);
                while (await reader.ReadAsync(ct))
                {
                    rotated[reader.GetString(0)] =
                        (reader.GetDateTime(1), reader.IsDBNull(2) ? null : reader.GetString(2));
                }
            }
            catch (PostgresException)
            {
                // Table absent (pre-V055 database): every secret reads "never",
                // which is the truthful answer — nothing has recorded a rotation.
            }

            var now = DateTime.UtcNow;
            var items = Inventory.Select(spec =>
            {
                rotated.TryGetValue(spec.Key, out var r);
                var at = r.At == default ? (DateTime?)null : r.At;
                var age = at is null ? (int?)null : (int)Math.Floor((now - at.Value).TotalDays);
                return new
                {
                    key = spec.Key,
                    name = spec.Name,
                    description = spec.Description,
                    max_age_days = spec.MaxAgeDays,
                    automated = spec.Automated,
                    risk = spec.Risk,
                    rotated_at = at,
                    rotated_by = at is null ? null : r.By,
                    age_days = age,
                    status = StatusFor(age, spec.MaxAgeDays),
                };
            }).ToList();

            return Results.Ok(new
            {
                generated_at = now,
                needs_attention = items.Count(i => i.status is "overdue" or "never"),
                secrets = items,
            });
        })
        .RequireAuthorization()
        .WithName("GetSecretAges");

        return app;
    }
}
