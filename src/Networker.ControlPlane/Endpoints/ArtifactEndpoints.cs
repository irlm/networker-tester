using System.Net.Http.Headers;
using System.Reflection;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Security;
using Networker.Data;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Serves the installers and the release binaries from laghound.com, so that
/// nothing outside this process needs a GitHub credential.
///
/// <para><b>Why this exists.</b> The repo is private, and a private repo's
/// <c>releases/download/</c> URLs return 404 to every caller — a Bearer token
/// on them does not help. Assets are reachable only through the API by numeric
/// id. That left two choices for the tester VMs: put a GitHub token in every
/// VM's cloud-init, or put it in exactly one place. This is the second.</para>
///
/// <para><b>Why not a token in cloud-init.</b> User-data is readable by any
/// process on the VM and, on Azure/AWS/GCP, through the instance metadata
/// service — a standard SSRF target. A repo-scoped GitHub token there would
/// grant read access to the WHOLE private repo from every ephemeral runner.
/// The agent api-key already in that user-data is a different risk class: it
/// is per-agent, revocable on its own, and useless against GitHub.</para>
///
/// <list type="bullet">
/// <item><c>GET /install.sh</c>, <c>GET /install.ps1</c> — anonymous, by
/// design: this is the <c>curl | bash</c> URL. Served from the embedded copy,
/// so the script always matches the running control plane.</item>
/// <item><c>GET /api/artifacts/{name}</c> — authenticated with the agent
/// api-key the VM already holds; streams the release asset.</item>
/// </list>
/// </summary>
public static class ArtifactEndpoints
{
    /// <summary>Env var holding a GitHub token with <c>contents:read</c> on
    /// this repo. Without it the artifact proxy reports 503 rather than
    /// pretending an asset is missing.</summary>
    public const string TokenEnv = "RELEASE_ASSET_TOKEN";

    private const string Repo = "irlm/networker-tester";

    /// <summary>Assets the proxy will serve. An allow-list, not a passthrough:
    /// the name reaches us from a VM, and it must never be able to steer the
    /// upstream request at anything other than a known release artifact.</summary>
    private static readonly HashSet<string> Allowed = new(StringComparer.Ordinal)
    {
        "networker-tester-x86_64-unknown-linux-musl.tar.gz",
        "networker-tester-x86_64-apple-darwin.tar.gz",
        "networker-tester-aarch64-apple-darwin.tar.gz",
        "networker-tester-x86_64-pc-windows-msvc.zip",
        "networker-endpoint-x86_64-unknown-linux-musl.tar.gz",
        "networker-endpoint-x86_64-apple-darwin.tar.gz",
        "networker-endpoint-aarch64-apple-darwin.tar.gz",
        "networker-endpoint-x86_64-pc-windows-msvc.zip",
        "networker-agent-cs-linux-x64.tar.gz",
        "networker-agent-cs-win-x64.zip",
        "alethabench-x86_64-unknown-linux-musl.tar.gz",
    };

    public static void MapArtifactEndpoints(this IEndpointRouteBuilder app)
    {
        // ── The curl | bash URL ────────────────────────────────────────────
        app.MapGet("/install.sh", () => Installer("install.sh", "text/x-shellscript"))
            .AllowAnonymous()
            .WithName("InstallSh");

        app.MapGet("/install.ps1", () => Installer("install.ps1", "text/plain"))
            .AllowAnonymous()
            .WithName("InstallPs1");

        // ── Release binaries, for machines that hold an agent key ───────────
        app.MapGet("/api/artifacts/{name}", async (
                string name,
                string? tag,
                HttpContext ctx,
                NetworkerDbContext db,
                IHttpClientFactory http,
                CancellationToken ct) =>
            await ProxyAssetAsync(name, tag, ctx, db, http, ct))
            .AllowAnonymous()   // authenticated below by agent api-key, not JWT
            .WithName("ReleaseArtifact");
    }

    private static IResult Installer(string resource, string contentType)
    {
        var stream = Assembly.GetExecutingAssembly()
            .GetManifestResourceStream($"Networker.ControlPlane.{resource}");
        if (stream is null)
        {
            // Embedded at build time; absence is a packaging fault, not a 404.
            return Results.Problem(
                $"{resource} was not embedded in this build", statusCode: 500);
        }

        return Results.Stream(stream, contentType);
    }

    private static async Task<IResult> ProxyAssetAsync(
        string name,
        string? tag,
        HttpContext ctx,
        NetworkerDbContext db,
        IHttpClientFactory http,
        CancellationToken ct)
    {
        if (!Allowed.Contains(name))
        {
            return Results.NotFound(new { error = "unknown artifact" });
        }

        if (!await IsKnownAgentAsync(ctx, db, ct))
        {
            return Results.Unauthorized();
        }

        var token = Environment.GetEnvironmentVariable(TokenEnv);
        if (string.IsNullOrWhiteSpace(token))
        {
            return Results.Problem(
                $"{TokenEnv} is not configured; the control plane cannot reach "
                + "release assets for a private repo.",
                statusCode: 503);
        }

        var client = http.CreateClient(nameof(ArtifactEndpoints));
        client.DefaultRequestHeaders.UserAgent.ParseAdd("laghound-control-plane");
        client.DefaultRequestHeaders.Authorization = new AuthenticationHeaderValue("Bearer", token);

        var relPath = string.IsNullOrWhiteSpace(tag) ? "releases/latest" : $"releases/tags/{Uri.EscapeDataString(tag)}";
        using var relResp = await client.GetAsync(
            $"https://api.github.com/repos/{Repo}/{relPath}", ct);
        if (!relResp.IsSuccessStatusCode)
        {
            return Results.Problem(
                $"could not resolve release ({(int)relResp.StatusCode})", statusCode: 502);
        }

        using var doc = await System.Text.Json.JsonDocument.ParseAsync(
            await relResp.Content.ReadAsStreamAsync(ct), cancellationToken: ct);
        long? assetId = null;
        if (doc.RootElement.TryGetProperty("assets", out var assets))
        {
            foreach (var a in assets.EnumerateArray())
            {
                if (a.TryGetProperty("name", out var n)
                    && string.Equals(n.GetString(), name, StringComparison.Ordinal)
                    && a.TryGetProperty("id", out var id))
                {
                    assetId = id.GetInt64();
                    break;
                }
            }
        }

        if (assetId is null)
        {
            return Results.NotFound(new { error = $"release has no asset named {name}" });
        }

        // Stream it through rather than buffering: these are tens of MB and the
        // control plane also serves the API and the agent WS hubs.
        var req = new HttpRequestMessage(
            HttpMethod.Get, $"https://api.github.com/repos/{Repo}/releases/assets/{assetId}");
        req.Headers.Accept.ParseAdd("application/octet-stream");
        var assetResp = await client.SendAsync(req, HttpCompletionOption.ResponseHeadersRead, ct);
        if (!assetResp.IsSuccessStatusCode)
        {
            assetResp.Dispose();
            return Results.Problem(
                $"asset download failed ({(int)assetResp.StatusCode})", statusCode: 502);
        }

        ctx.Response.RegisterForDispose(assetResp);
        return Results.Stream(
            await assetResp.Content.ReadAsStreamAsync(ct), "application/octet-stream");
    }

    /// <summary>
    /// True when the request carries an api-key belonging to a registered
    /// agent. Same credential and same hashing as the agent WS handshake
    /// (<see cref="AgentApiKeys"/>) — the key is looked up BY HASH, and the
    /// digest is then compared in constant time.
    /// </summary>
    private static async Task<bool> IsKnownAgentAsync(
        HttpContext ctx, NetworkerDbContext db, CancellationToken ct)
    {
        var key = ctx.Request.Headers["X-Agent-Key"].FirstOrDefault()
                  ?? ctx.Request.Query["key"].FirstOrDefault();
        if (string.IsNullOrWhiteSpace(key))
        {
            return false;
        }

        var hash = AgentApiKeys.HashHex(key);
        var stored = await db.Agents
            .AsNoTracking()
            .Where(a => a.ApiKeyHash == hash)
            .Select(a => a.ApiKeyHash)
            .FirstOrDefaultAsync(ct);

        return AgentApiKeys.FixedTimeEqualsHex(stored, hash);
    }
}
