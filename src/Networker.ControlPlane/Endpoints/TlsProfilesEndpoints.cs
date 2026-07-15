using System.Text.Json;
using Networker.ControlPlane.Auth;
using Npgsql;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// C# port of the Rust dashboard's <c>api/tls_profiles.rs</c> project-scoped read
/// endpoints (TLS endpoint profiling results).
///
/// <para>Routes (all under <c>/api/projects/{projectId}</c>, ProjectMember /
/// Viewer role, matching the Rust <c>project_scoped</c> group):</para>
/// <list type="bullet">
///   <item><b>GET .../tls-profiles</b> — list summaries scoped to the project
///     (limit/offset, default 50, max 200; offset floored at 0).</item>
///   <item><b>GET .../tls-profiles/{run_id}</b> — full detail, scoped by
///     (ProjectId, Id). 404 when absent. Emits the stored <c>ProfileJson</c>
///     JSONB as the raw <c>profile</c> field.</item>
/// </list>
///
/// <para>Raw-SQL divergence: <c>TlsProfileRun</c> is NOT in the EF model (it uses
/// PascalCase-folded columns and only exists once TLS profiling has run). Read
/// with raw Npgsql via <see cref="NpgsqlDataSource"/>, mirroring
/// <c>db::tls_profiles</c> verbatim. A missing table (undefined_table, 42P01)
/// yields an empty list / null, exactly as in Rust.</para>
/// </summary>
public static class TlsProfilesEndpoints
{
    private const int DefaultLimit = 50;
    private const int MaxLimit = 200;

    public static IEndpointRouteBuilder MapTlsProfilesEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/projects/{projectId}/tls-profiles — list summaries.
        app.MapGet("/api/projects/{projectId}/tls-profiles", async (
            string projectId,
            int? limit,
            int? offset,
            NpgsqlDataSource dataSource,
            CancellationToken ct) =>
        {
            var take = Math.Clamp(limit ?? DefaultLimit, 1, MaxLimit);
            var skip = Math.Max(offset ?? 0, 0);

            var rows = await ListAsync(dataSource, projectId, take, skip, ct);
            return Results.Ok(rows);
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        // GET /api/projects/{projectId}/tls-profiles/{run_id} — full detail.
        app.MapGet("/api/projects/{projectId}/tls-profiles/{run_id:guid}", async (
            string projectId,
            Guid run_id,
            NpgsqlDataSource dataSource,
            CancellationToken ct) =>
        {
            var detail = await GetAsync(dataSource, projectId, run_id, ct);
            return detail is null ? Results.NotFound() : Results.Ok(detail);
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        return app;
    }

    private static bool IsUndefinedTable(PostgresException ex) => ex.SqlState == "42P01";

    private static async Task<List<TlsProfileSummaryRow>> ListAsync(
        NpgsqlDataSource dataSource, string projectId, int limit, int offset, CancellationToken ct)
    {
        const string sql =
            "SELECT Id, StartedAt, Host, Port, TargetKind, CoverageLevel, SummaryStatus, SummaryScore " +
            "FROM TlsProfileRun " +
            "WHERE ProjectId = $1 " +
            "ORDER BY StartedAt DESC " +
            "LIMIT $2 OFFSET $3";

        var result = new List<TlsProfileSummaryRow>();
        try
        {
            await using var cmd = dataSource.CreateCommand(sql);
            cmd.Parameters.AddWithValue(projectId);
            cmd.Parameters.AddWithValue((long)limit);
            cmd.Parameters.AddWithValue((long)offset);
            await using var reader = await cmd.ExecuteReaderAsync(ct);
            while (await reader.ReadAsync(ct))
            {
                result.Add(new TlsProfileSummaryRow
                {
                    id = reader.GetGuid(0),
                    started_at = reader.GetDateTime(1),
                    host = reader.GetString(2),
                    port = reader.GetInt32(3),
                    target_kind = reader.GetString(4),
                    coverage_level = reader.GetString(5),
                    summary_status = reader.GetString(6),
                    summary_score = reader.IsDBNull(7) ? null : reader.GetInt32(7),
                });
            }
        }
        catch (PostgresException ex) when (IsUndefinedTable(ex))
        {
            return new List<TlsProfileSummaryRow>();
        }

        return result;
    }

    private static async Task<TlsProfileDetail?> GetAsync(
        NpgsqlDataSource dataSource, string projectId, Guid id, CancellationToken ct)
    {
        const string sql =
            "SELECT Id, StartedAt, Host, Port, TargetKind, CoverageLevel, SummaryStatus, SummaryScore, ProfileJson " +
            "FROM TlsProfileRun " +
            "WHERE ProjectId = $1 AND Id = $2";

        try
        {
            await using var cmd = dataSource.CreateCommand(sql);
            cmd.Parameters.AddWithValue(projectId);
            cmd.Parameters.AddWithValue(id);
            await using var reader = await cmd.ExecuteReaderAsync(ct);
            if (!await reader.ReadAsync(ct))
            {
                return null;
            }

            // ProfileJson is JSONB — emit as raw JSON (matches Rust's typed
            // TlsEndpointProfile serialization, which is that same JSON value).
            object profile;
            var profileText = reader.IsDBNull(8) ? "null" : reader.GetString(8);
            try
            {
                using var doc = JsonDocument.Parse(profileText);
                profile = doc.RootElement.Clone();
            }
            catch (JsonException)
            {
                profile = profileText;
            }

            return new TlsProfileDetail
            {
                id = reader.GetGuid(0),
                started_at = reader.GetDateTime(1),
                host = reader.GetString(2),
                port = reader.GetInt32(3),
                target_kind = reader.GetString(4),
                coverage_level = reader.GetString(5),
                summary_status = reader.GetString(6),
                summary_score = reader.IsDBNull(7) ? null : reader.GetInt32(7),
                profile = profile,
            };
        }
        catch (PostgresException ex) when (IsUndefinedTable(ex))
        {
            return null;
        }
    }
}

public sealed class TlsProfileSummaryRow
{
    public Guid id { get; set; }
    public DateTime started_at { get; set; }
    public string host { get; set; } = string.Empty;
    public int port { get; set; }
    public string target_kind { get; set; } = string.Empty;
    public string coverage_level { get; set; } = string.Empty;
    public string summary_status { get; set; } = string.Empty;
    public int? summary_score { get; set; }
}

public sealed class TlsProfileDetail
{
    public Guid id { get; set; }
    public DateTime started_at { get; set; }
    public string host { get; set; } = string.Empty;
    public int port { get; set; }
    public string target_kind { get; set; } = string.Empty;
    public string coverage_level { get; set; } = string.Empty;
    public string summary_status { get; set; } = string.Empty;
    public int? summary_score { get; set; }
    public object profile { get; set; } = new();
}
