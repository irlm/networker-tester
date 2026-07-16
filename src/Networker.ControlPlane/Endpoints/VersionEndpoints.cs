using System.Diagnostics;
using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Background;
using Networker.Data;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// C# port of the Rust dashboard's <c>api/version.rs</c> — the authenticated
/// <c>GET /api/version</c> handler the React frontend polls for the "Latest
/// version" toast and the tester-upgrade badge (dashboard/src/api/client.ts
/// <c>get('/version')</c>).
///
/// <para>Auth: Rust merges <c>version::router</c> into <c>protected_flat</c>
/// (nested under <c>/api</c>, requires a valid JWT but no project scope), so
/// this uses a bare <c>.RequireAuthorization()</c> — any authenticated user.</para>
///
/// <para>Response shape (matches serde exactly): <c>{ dashboard_version,
/// tester_version, latest_release, update_available, endpoints: [{ host,
/// version, reachable }] }</c>.</para>
///
/// <para>Faithful behavior:
/// <list type="bullet">
/// <item><c>dashboard_version</c> — the compile-time version (Rust
/// <c>CARGO_PKG_VERSION</c>); here the same floor wired into
/// <see cref="LatestVersionCache"/> at startup.</item>
/// <item><c>tester_version</c> — best-effort local <c>networker-tester
/// --version</c>; null when the binary isn't on this host (matches Rust's
/// <c>get_tester_version</c> returning None on the control-plane box).</item>
/// <item><c>latest_release</c> — read from the shared
/// <see cref="LatestVersionCache"/> (populated by
/// <see cref="VersionRefreshService"/> on a 5-min cadence). The bootstrap
/// value equal to <c>dashboard_version</c> is treated as "unknown" → null,
/// exactly as Rust does, so the UI doesn't flash a spurious "no update".</item>
/// <item><c>endpoints</c> — completed deployments' endpoint IPs, each probed
/// (HTTPS :8443 then HTTP :8080 <c>/health</c>) with the same tight
/// connect/read budget as Rust so a dead endpoint can't drag the response.</item>
/// </list></para>
/// </summary>
public static class VersionEndpoints
{
    public static IEndpointRouteBuilder MapVersionEndpoints(this IEndpointRouteBuilder app)
    {
        app.MapGet("/api/version", async (NetworkerDbContext db, LatestVersionCache cache) =>
        {
            var dashboardVersion = DashboardVersion;

            var testerVersion = await GetTesterVersionAsync();

            // Treat the bootstrap-fallback value (equal to our own version) as
            // "unknown" so the UI doesn't show a spurious "no update" message
            // before the refresh loop has populated the cache — matches Rust.
            var cached = cache.Current;
            string? latestRelease =
                string.IsNullOrEmpty(cached) || cached == dashboardVersion ? null : cached;

            var updateAvailable =
                testerVersion is not null && latestRelease is not null
                && VersionNewer(
                    latestRelease.TrimStart('v'),
                    testerVersion.TrimStart('v'));

            // Completed deployments only; dedup IPs; probe concurrently.
            var deployments = await db.Deployments
                .AsNoTracking()
                .Where(d => d.Status == "completed")
                .OrderByDescending(d => d.CreatedAt)
                .Take(20)
                .Select(d => d.EndpointIps)
                .ToListAsync();

            var hosts = new SortedSet<string>(StringComparer.Ordinal);
            foreach (var ipsJson in deployments)
            {
                if (string.IsNullOrWhiteSpace(ipsJson))
                {
                    continue;
                }

                try
                {
                    var ips = JsonSerializer.Deserialize<List<string>>(ipsJson);
                    if (ips is not null)
                    {
                        foreach (var ip in ips)
                        {
                            hosts.Add(ip);
                        }
                    }
                }
                catch (JsonException)
                {
                    // Match Rust's .ok().unwrap_or_default() — skip malformed rows.
                }
            }

            var endpoints = await Task.WhenAll(hosts.Select(CheckEndpointVersionAsync));

            return Results.Json(new VersionInfoDto(
                dashboardVersion,
                testerVersion,
                latestRelease,
                updateAvailable,
                endpoints));
        })
        .RequireAuthorization();

        return app;
    }

    /// <summary>
    /// Compile-time version floor — kept in sync with the value wired into
    /// <c>AddVersionRefresh(...)</c> in Program.cs (Rust <c>CARGO_PKG_VERSION</c>).
    /// Public because the tester create flow stamps it into
    /// <c>project_tester.installer_version</c>, matching Rust's
    /// <c>env!("CARGO_PKG_VERSION")</c>.
    /// </summary>
    public const string DashboardVersion = "0.28.26";

    private const string TesterBinaryEnvVar = "AGENT_TESTERPATH";

    /// <summary>
    /// Best-effort local <c>networker-tester --version</c>. Returns the trailing
    /// token of stdout (e.g. "networker-tester 0.28.14" → "0.28.14"), or null
    /// when the binary is absent or errors — matching Rust <c>get_tester_version</c>.
    /// </summary>
    private static async Task<string?> GetTesterVersionAsync()
    {
        var bin = LocateTesterBinary();
        if (bin is null)
        {
            return null;
        }

        try
        {
            using var proc = new Process
            {
                StartInfo = new ProcessStartInfo
                {
                    FileName = bin,
                    Arguments = "--version",
                    RedirectStandardOutput = true,
                    RedirectStandardError = true,
                    UseShellExecute = false,
                    CreateNoWindow = true,
                },
            };

            if (!proc.Start())
            {
                return null;
            }

            var stdout = await proc.StandardOutput.ReadToEndAsync();
            using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(5));
            await proc.WaitForExitAsync(timeout.Token);

            if (proc.ExitCode != 0)
            {
                return null;
            }

            var token = stdout
                .Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries)
                .LastOrDefault();
            return string.IsNullOrWhiteSpace(token) ? null : token.Trim();
        }
        catch (Exception)
        {
            return null;
        }
    }

    private static string? LocateTesterBinary()
    {
        var fromEnv = Environment.GetEnvironmentVariable(TesterBinaryEnvVar);
        if (!string.IsNullOrWhiteSpace(fromEnv) && File.Exists(fromEnv))
        {
            return fromEnv;
        }

        var exe = OperatingSystem.IsWindows() ? "networker-tester.exe" : "networker-tester";
        foreach (var dir in new[]
                 {
                     AppContext.BaseDirectory,
                     Directory.GetCurrentDirectory(),
                     "/usr/local/bin",
                     "/usr/bin",
                 })
        {
            var candidate = Path.Combine(dir, exe);
            if (File.Exists(candidate))
            {
                return candidate;
            }
        }

        return null;
    }

    /// <summary>
    /// Probe a deployment host's <c>/health</c> (HTTPS :8443 then HTTP :8080)
    /// with a tight budget — a dead endpoint must not drag the summary past
    /// ~1.5s. Accepts self-signed certs, matching Rust's
    /// <c>danger_accept_invalid_certs(true)</c>.
    /// </summary>
    private static async Task<EndpointVersionDto> CheckEndpointVersionAsync(string host)
    {
        using var handler = new HttpClientHandler
        {
            ServerCertificateCustomValidationCallback =
                HttpClientHandler.DangerousAcceptAnyServerCertificateValidator,
        };
        using var client = new HttpClient(handler)
        {
            Timeout = TimeSpan.FromMilliseconds(1500),
        };

        foreach (var url in new[]
                 {
                     $"https://{host}:8443/health",
                     $"http://{host}:8080/health",
                 })
        {
            try
            {
                using var resp = await client.GetAsync(url);
                var body = await resp.Content.ReadAsStringAsync();
                using var doc = JsonDocument.Parse(body);
                string? version =
                    doc.RootElement.TryGetProperty("version", out var v)
                    && v.ValueKind == JsonValueKind.String
                        ? v.GetString()
                        : null;
                return new EndpointVersionDto(host, version, true);
            }
            catch (Exception)
            {
                // Try the next URL; unreachable if both fail.
            }
        }

        return new EndpointVersionDto(host, null, false);
    }

    /// <summary>Semver "is a newer than b" — port of Rust <c>version_newer</c>:
    /// dot-split, non-numeric segments dropped, missing trailing parts = 0.</summary>
    public static bool VersionNewer(string a, string b)
    {
        static List<uint> Parse(string s) =>
            s.Split('.')
                .Select(p => uint.TryParse(p, out var n) ? (uint?)n : null)
                .Where(n => n.HasValue)
                .Select(n => n!.Value)
                .ToList();

        var va = Parse(a);
        var vb = Parse(b);
        for (var i = 0; i < Math.Max(va.Count, vb.Count); i++)
        {
            var pa = i < va.Count ? va[i] : 0;
            var pb = i < vb.Count ? vb[i] : 0;
            if (pa > pb)
            {
                return true;
            }

            if (pa < pb)
            {
                return false;
            }
        }

        return false;
    }

    // snake_case DTOs so System.Text.Json emits the exact serde field names.
    private sealed record VersionInfoDto(
        string dashboard_version,
        string? tester_version,
        string? latest_release,
        bool update_available,
        EndpointVersionDto[] endpoints);

    private sealed record EndpointVersionDto(
        string host,
        string? version,
        bool reachable);
}
