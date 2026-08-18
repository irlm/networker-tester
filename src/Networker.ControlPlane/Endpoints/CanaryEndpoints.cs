using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Admin-only surface for the prod run-execution canary
/// (<c>.github/workflows/soak-canary.yml</c>). Today an administrator can only
/// trigger the canary from the GitHub Actions UI and read its logs there; these
/// endpoints bring the trigger in-product and point at where the results show
/// up (the real runs/deployments the canary drives, plus the Actions run log).
///
/// <para>Everything here is platform-global and gated by
/// <see cref="AuthPolicies.GlobalAdmin"/> — the same gate as
/// <see cref="AdminEndpoints"/> and the bench-tokens revoke-all.</para>
///
/// <list type="bullet">
///   <item><b>GET /api/admin/canary</b> — report whether dispatch is configured
///     (a GitHub token is present) and the links to view results.</item>
///   <item><b>POST /api/admin/canary/dispatch</b> — trigger
///     <c>soak-canary.yml</c> via the GitHub REST
///     <c>workflow_dispatch</c> API. Requires <c>CANARY_GITHUB_TOKEN</c>; when
///     it is absent the endpoint returns a clear "not configured" error rather
///     than failing opaquely (this is the expected local-lab behaviour — no
///     token is wired into the lab). On success it also records the dispatch in
///     <c>canary_dispatch</c> (who/when/ref/inputs) so the history is durable.</item>
///   <item><b>GET /api/admin/canary/history</b> — the in-product dispatch
///     history straight from OUR database (survives GitHub being unreachable and
///     is visible from any deployment sharing this database). Run id / status /
///     conclusion are backfilled by <c>CanaryRunPoller</c>.</item>
///   <item><b>GET /api/admin/canary/runs</b> — the live list of recent
///     <c>soak-canary.yml</c> runs pulled from GitHub (includes runs triggered
///     outside the product). Needs the token; returns an empty list otherwise.</item>
/// </list>
///
/// <para><b>Never hardcode a token.</b> The token comes only from
/// <c>CANARY_GITHUB_TOKEN</c> (a PAT / fine-grained token with
/// <c>actions:write</c> on the repo). The repo slug defaults to the LagHound
/// repo and can be overridden with <c>CANARY_GITHUB_REPO</c> (<c>owner/name</c>).</para>
/// </summary>
public static class CanaryEndpoints
{
    /// <summary>The workflow file dispatched — matches the file name on disk.</summary>
    public const string WorkflowFile = "soak-canary.yml";

    /// <summary>Default repo slug when <c>CANARY_GITHUB_REPO</c> is unset.</summary>
    public const string DefaultRepo = "irlm/networker-tester";

    /// <summary>How many history rows / live runs to return by default.</summary>
    private const int DefaultPageSize = 30;

    public static IEndpointRouteBuilder MapCanaryEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/admin/canary — config + links. Needs no token to answer, so
        // the panel renders (and can say "not configured") on the local lab.
        app.MapGet("/api/admin/canary", () =>
        {
            var (owner, repo) = RepoSlug();
            return Results.Ok(new
            {
                configured = GitHubToken() is not null,
                owner,
                repo,
                workflow = WorkflowFile,
                actions_url = ActionsUrl(owner, repo),
            });
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        // POST /api/admin/canary/dispatch — fire the workflow_dispatch.
        app.MapPost("/api/admin/canary/dispatch", async (
            CanaryDispatchBody? body,
            HttpContext ctx,
            NetworkerDbContext db,
            IHttpClientFactory httpFactory,
            ILoggerFactory lf,
            CancellationToken ct) =>
        {
            var log = lf.CreateLogger("Networker.Canary");
            var (owner, repo) = RepoSlug();

            var token = GitHubToken();
            if (token is null)
            {
                // Expected on the local lab: a clear, actionable message, not a 500.
                return Results.Json(new
                {
                    error = "Canary dispatch is not configured. Set CANARY_GITHUB_TOKEN " +
                            "(a token with actions:write on the repo) on the control plane to enable it.",
                    configured = false,
                }, statusCode: StatusCodes.Status409Conflict);
            }

            var b = body ?? new CanaryDispatchBody();
            var inputs = new Dictionary<string, string>(StringComparer.Ordinal)
            {
                ["reuse_runner"] = Flag(b.ReuseRunner, true),
                ["apibench"] = Flag(b.Apibench, true),
                ["mode_coverage"] = Flag(b.ModeCoverage, true),
                ["matrix_flow"] = Flag(b.MatrixFlow, false),
                ["windows"] = Flag(b.Windows, false),
            };
            var gitRef = string.IsNullOrWhiteSpace(b.Ref) ? "main" : b.Ref!.Trim();
            var payload = JsonSerializer.Serialize(new { @ref = gitRef, inputs });

            var client = httpFactory.CreateClient();
            using var req = new HttpRequestMessage(
                HttpMethod.Post,
                $"https://api.github.com/repos/{owner}/{repo}/actions/workflows/{WorkflowFile}/dispatches");
            AddGitHubHeaders(req.Headers, token);
            req.Content = new StringContent(payload, Encoding.UTF8, "application/json");

            HttpResponseMessage resp;
            try
            {
                resp = await client.SendAsync(req, ct);
            }
            catch (Exception ex) when (ex is not OperationCanceledException)
            {
                log.LogError(ex, "canary dispatch: GitHub request failed");
                return Results.Json(
                    new { error = $"GitHub request failed: {ex.Message}" },
                    statusCode: StatusCodes.Status502BadGateway);
            }

            // GitHub answers 204 No Content on a successful dispatch.
            if (resp.StatusCode == System.Net.HttpStatusCode.NoContent)
            {
                var admin = ctx.GetAuthUser()?.Email;
                log.LogInformation(
                    "Canary workflow dispatched by {Admin} (ref={Ref}, inputs={Inputs})",
                    admin, gitRef,
                    string.Join(",", inputs.Select(kv => $"{kv.Key}={kv.Value}")));

                // Persist the dispatch so the history is durable even if GitHub
                // is later unreachable. The run itself already fired; never fail
                // the request just because we could not write the history row.
                Guid? dispatchId = null;
                try
                {
                    var now = DateTime.UtcNow;
                    var row = new CanaryDispatch
                    {
                        Id = Guid.NewGuid(),
                        RequestedBy = admin,
                        RequestedAt = now,
                        GitRef = gitRef,
                        Inputs = JsonSerializer.Serialize(inputs),
                        UpdatedAt = now,
                    };
                    db.CanaryDispatches.Add(row);
                    await db.SaveChangesAsync(ct);
                    dispatchId = row.Id;
                }
                catch (Exception ex) when (ex is not OperationCanceledException)
                {
                    log.LogWarning(ex,
                        "canary dispatch fired but the history row could not be persisted");
                }

                return Results.Ok(new
                {
                    status = "dispatched",
                    dispatch_id = dispatchId,
                    actions_url = ActionsUrl(owner, repo),
                });
            }

            var errBody = await resp.Content.ReadAsStringAsync(ct);
            log.LogWarning(
                "canary dispatch: GitHub returned {Status}: {Body}", (int)resp.StatusCode, errBody);
            return Results.Json(new
            {
                error = $"GitHub dispatch returned HTTP {(int)resp.StatusCode}.",
                detail = Truncate(errBody, 500),
            }, statusCode: StatusCodes.Status502BadGateway);
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        // GET /api/admin/canary/history — durable, DB-backed dispatch history.
        // No GitHub call: this is exactly the view that must keep working when
        // GitHub is down or you are on a different deployment.
        app.MapGet("/api/admin/canary/history", async (
            NetworkerDbContext db,
            int? limit,
            CancellationToken ct) =>
        {
            var take = Math.Clamp(limit ?? DefaultPageSize, 1, 200);
            var rows = await db.CanaryDispatches
                .AsNoTracking()
                .OrderByDescending(d => d.RequestedAt)
                .Take(take)
                .ToListAsync(ct);

            var items = rows.Select(r => new
            {
                id = r.Id,
                requested_by = r.RequestedBy,
                requested_at = r.RequestedAt,
                git_ref = r.GitRef,
                inputs = ParseInputs(r.Inputs),
                run_id = r.RunId,
                run_url = r.RunUrl,
                run_status = r.RunStatus,
                conclusion = r.Conclusion,
                updated_at = r.UpdatedAt,
            });
            return Results.Ok(new { items });
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        // GET /api/admin/canary/runs — live recent runs from GitHub (includes
        // runs triggered outside the product). Best-effort: no token or a GitHub
        // error yields an empty list with a reason, never a 500.
        app.MapGet("/api/admin/canary/runs", async (
            IHttpClientFactory httpFactory,
            ILoggerFactory lf,
            int? limit,
            CancellationToken ct) =>
        {
            var log = lf.CreateLogger("Networker.Canary");
            var (owner, repo) = RepoSlug();
            var take = Math.Clamp(limit ?? DefaultPageSize, 1, 100);

            var token = GitHubToken();
            if (token is null)
            {
                return Results.Ok(new
                {
                    configured = false,
                    runs = Array.Empty<object>(),
                    detail = "No CANARY_GITHUB_TOKEN configured; live GitHub run list is unavailable.",
                });
            }

            try
            {
                var runs = await FetchRunsAsync(httpFactory, owner, repo, token, take, ct);
                return Results.Ok(new { configured = true, runs });
            }
            catch (Exception ex) when (ex is not OperationCanceledException)
            {
                log.LogWarning(ex, "canary runs: GitHub fetch failed");
                return Results.Ok(new
                {
                    configured = true,
                    runs = Array.Empty<object>(),
                    detail = $"GitHub run list unavailable: {ex.Message}",
                });
            }
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        return app;
    }

    /// <summary>Body for POST /api/admin/canary/dispatch — mirrors the
    /// workflow's <c>workflow_dispatch</c> inputs. All optional; omitted flags
    /// fall back to the workflow's own defaults.</summary>
    public sealed record CanaryDispatchBody
    {
        [JsonPropertyName("reuse_runner")] public bool? ReuseRunner { get; init; }
        [JsonPropertyName("apibench")] public bool? Apibench { get; init; }
        [JsonPropertyName("mode_coverage")] public bool? ModeCoverage { get; init; }
        [JsonPropertyName("matrix_flow")] public bool? MatrixFlow { get; init; }
        [JsonPropertyName("windows")] public bool? Windows { get; init; }

        /// <summary>Git ref to run against (branch/tag). Defaults to <c>main</c>.</summary>
        [JsonPropertyName("ref")] public string? Ref { get; init; }
    }

    // ── GitHub run listing (feature A + poller share this) ─────────────────

    /// <summary>Fetch the most recent <c>soak-canary.yml</c> runs from GitHub,
    /// projected to the compact shape the UI and poller consume.</summary>
    public static async Task<IReadOnlyList<CanaryRunView>> FetchRunsAsync(
        IHttpClientFactory httpFactory,
        string owner,
        string repo,
        string token,
        int perPage,
        CancellationToken ct)
    {
        var client = httpFactory.CreateClient();
        using var req = new HttpRequestMessage(
            HttpMethod.Get,
            $"https://api.github.com/repos/{owner}/{repo}/actions/workflows/{WorkflowFile}/runs" +
            $"?per_page={Math.Clamp(perPage, 1, 100)}");
        AddGitHubHeaders(req.Headers, token);

        using var resp = await client.SendAsync(req, ct);
        resp.EnsureSuccessStatusCode();
        await using var stream = await resp.Content.ReadAsStreamAsync(ct);
        using var doc = await JsonDocument.ParseAsync(stream, cancellationToken: ct);

        var list = new List<CanaryRunView>();
        if (doc.RootElement.TryGetProperty("workflow_runs", out var runs) &&
            runs.ValueKind == JsonValueKind.Array)
        {
            foreach (var r in runs.EnumerateArray())
            {
                list.Add(new CanaryRunView(
                    Id: r.TryGetProperty("id", out var id) && id.TryGetInt64(out var idv) ? idv : 0,
                    RunNumber: r.TryGetProperty("run_number", out var rn) && rn.TryGetInt32(out var rnv) ? rnv : 0,
                    Event: Str(r, "event"),
                    Status: Str(r, "status"),
                    Conclusion: Str(r, "conclusion"),
                    Branch: Str(r, "head_branch"),
                    Title: Str(r, "display_title"),
                    Actor: r.TryGetProperty("actor", out var a) ? Str(a, "login") : null,
                    HtmlUrl: Str(r, "html_url"),
                    CreatedAt: Str(r, "created_at"),
                    UpdatedAt: Str(r, "updated_at")));
            }
        }
        return list;
    }

    /// <summary>Fetch a single run by id (poller uses this once a run is linked).</summary>
    public static async Task<CanaryRunView?> FetchRunAsync(
        IHttpClientFactory httpFactory,
        string owner,
        string repo,
        string token,
        long runId,
        CancellationToken ct)
    {
        var client = httpFactory.CreateClient();
        using var req = new HttpRequestMessage(
            HttpMethod.Get,
            $"https://api.github.com/repos/{owner}/{repo}/actions/runs/{runId}");
        AddGitHubHeaders(req.Headers, token);

        using var resp = await client.SendAsync(req, ct);
        if (!resp.IsSuccessStatusCode)
        {
            return null;
        }
        await using var stream = await resp.Content.ReadAsStreamAsync(ct);
        using var doc = await JsonDocument.ParseAsync(stream, cancellationToken: ct);
        var r = doc.RootElement;
        return new CanaryRunView(
            Id: r.TryGetProperty("id", out var id) && id.TryGetInt64(out var idv) ? idv : runId,
            RunNumber: r.TryGetProperty("run_number", out var rn) && rn.TryGetInt32(out var rnv) ? rnv : 0,
            Event: Str(r, "event"),
            Status: Str(r, "status"),
            Conclusion: Str(r, "conclusion"),
            Branch: Str(r, "head_branch"),
            Title: Str(r, "display_title"),
            Actor: r.TryGetProperty("actor", out var a) ? Str(a, "login") : null,
            HtmlUrl: Str(r, "html_url"),
            CreatedAt: Str(r, "created_at"),
            UpdatedAt: Str(r, "updated_at"));
    }

    /// <summary>Compact projection of a GitHub Actions workflow run.</summary>
    public sealed record CanaryRunView(
        long Id,
        int RunNumber,
        string? Event,
        string? Status,
        string? Conclusion,
        string? Branch,
        string? Title,
        string? Actor,
        string? HtmlUrl,
        string? CreatedAt,
        string? UpdatedAt);

    // ── Config helpers (env only; never hardcode a token) ──────────────────

    /// <summary>The GitHub token from <c>CANARY_GITHUB_TOKEN</c>, or null when
    /// unset/empty.</summary>
    public static string? GitHubToken()
    {
        var v = Environment.GetEnvironmentVariable("CANARY_GITHUB_TOKEN");
        return string.IsNullOrWhiteSpace(v) ? null : v.Trim();
    }

    /// <summary>The (owner, repo) from <c>CANARY_GITHUB_REPO</c>
    /// (<c>owner/name</c>), falling back to <see cref="DefaultRepo"/>.</summary>
    public static (string Owner, string Repo) RepoSlug()
    {
        var slug = Environment.GetEnvironmentVariable("CANARY_GITHUB_REPO");
        if (string.IsNullOrWhiteSpace(slug))
        {
            slug = DefaultRepo;
        }
        var parts = slug.Trim().Split('/', 2, StringSplitOptions.TrimEntries);
        return parts.Length == 2 && parts[0].Length > 0 && parts[1].Length > 0
            ? (parts[0], parts[1])
            : (DefaultRepo.Split('/')[0], DefaultRepo.Split('/')[1]);
    }

    internal static void AddGitHubHeaders(System.Net.Http.Headers.HttpRequestHeaders headers, string token)
    {
        headers.TryAddWithoutValidation("Accept", "application/vnd.github+json");
        headers.TryAddWithoutValidation("Authorization", $"Bearer {token}");
        headers.TryAddWithoutValidation("X-GitHub-Api-Version", "2022-11-28");
        headers.TryAddWithoutValidation("User-Agent", "networker-control-plane");
    }

    private static string ActionsUrl(string owner, string repo)
        => $"https://github.com/{owner}/{repo}/actions/workflows/{WorkflowFile}";

    /// <summary>Workflow inputs are strings; the dispatch API wants "1"/"0".</summary>
    private static string Flag(bool? value, bool fallback)
        => (value ?? fallback) ? "1" : "0";

    private static string? Str(JsonElement e, string prop)
        => e.TryGetProperty(prop, out var v) && v.ValueKind == JsonValueKind.String ? v.GetString() : null;

    /// <summary>Parse the stored inputs jsonb back into an object for the UI;
    /// returns an empty object if it is somehow unparseable.</summary>
    private static JsonElement ParseInputs(string json)
    {
        try
        {
            using var doc = JsonDocument.Parse(string.IsNullOrWhiteSpace(json) ? "{}" : json);
            return doc.RootElement.Clone();
        }
        catch (JsonException)
        {
            using var empty = JsonDocument.Parse("{}");
            return empty.RootElement.Clone();
        }
    }

    private static string Truncate(string s, int max)
        => s.Length <= max ? s : s[..max] + "…";
}
