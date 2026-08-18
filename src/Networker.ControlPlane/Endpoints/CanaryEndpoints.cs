using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using Networker.ControlPlane.Auth;

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
///     token is wired into the lab).</item>
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
            req.Headers.TryAddWithoutValidation("Accept", "application/vnd.github+json");
            req.Headers.TryAddWithoutValidation("Authorization", $"Bearer {token}");
            req.Headers.TryAddWithoutValidation("X-GitHub-Api-Version", "2022-11-28");
            req.Headers.TryAddWithoutValidation("User-Agent", "networker-control-plane");
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
                log.LogInformation(
                    "Canary workflow dispatched by {Admin} (ref={Ref}, inputs={Inputs})",
                    ctx.GetAuthUser()?.Email, gitRef,
                    string.Join(",", inputs.Select(kv => $"{kv.Key}={kv.Value}")));
                return Results.Ok(new
                {
                    status = "dispatched",
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

    private static string ActionsUrl(string owner, string repo)
        => $"https://github.com/{owner}/{repo}/actions/workflows/{WorkflowFile}";

    /// <summary>Workflow inputs are strings; the dispatch API wants "1"/"0".</summary>
    private static string Flag(bool? value, bool fallback)
        => (value ?? fallback) ? "1" : "0";

    private static string Truncate(string s, int max)
        => s.Length <= max ? s : s[..max] + "…";
}
