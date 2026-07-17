using System.Net;
using System.Net.Http.Json;
using System.Text.Json;

namespace Networker.ControlPlane.Background;

/// <summary>
/// Thread-safe cache of the latest known <c>networker-tester</c> release version
/// — the C# port of the Rust <c>Arc&lt;RwLock&lt;String&gt;&gt;</c> that
/// <c>version_refresh.rs</c> maintains. Seeded with the compile-time floor
/// (the control plane's own version) and updated in the background by
/// <see cref="VersionRefreshService"/>.
/// </summary>
public sealed class LatestVersionCache
{
    private readonly Lock _sync = new();
    private string _value;

    /// <param name="floorVersion">The compile-time version floor (Rust
    /// <c>CARGO_PKG_VERSION</c>). The cache never drops below this.</param>
    public LatestVersionCache(string floorVersion) => _value = floorVersion;

    /// <summary>The current cached latest version string.</summary>
    public string Current
    {
        get
        {
            lock (_sync)
            {
                return _value;
            }
        }
    }

    internal void Set(string value)
    {
        lock (_sync)
        {
            _value = value;
        }
    }
}

/// <summary>
/// Periodic GitHub-release version-cache refresher — the C# port of Rust
/// <c>crates/networker-dashboard/src/services/version_refresh.rs</c>.
///
/// <para>Every <c>6h + random(0..30min)</c> jitter (RR-011: stagger multi-replica
/// deploys off the same 6h mark) it polls the GitHub "latest release" endpoint,
/// picks the higher of the compile-time floor and the remote tag by semver, and
/// stores the result in <see cref="LatestVersionCache"/>. <b>It sleeps first,
/// then refreshes</b> — matching the Rust loop (which sleeps before the first
/// refresh); the cache starts at the floor version. Rate-limit (403/429) is
/// treated as a soft fallback to the floor, logged at debug.</para>
///
/// <para>No leader lock / tick-monitor: this loop is a read-only HTTP poll into
/// an in-process cache — running it on every replica is harmless (the Rust
/// service likewise runs unconditionally, and there is no DB write to serialize).</para>
/// </summary>
public sealed class VersionRefreshService : BackgroundService
{
    /// <summary>Rust <c>REFRESH_BASE</c> = 6h.</summary>
    private static readonly TimeSpan RefreshBase = TimeSpan.FromHours(6);

    /// <summary>Rust <c>REFRESH_JITTER_MAX_SECS</c> = 30min.</summary>
    private const int RefreshJitterMaxSecs = 30 * 60;

    /// <summary>Rust <c>GITHUB_LATEST</c>.</summary>
    public const string GithubLatestUrl =
        "https://api.github.com/repos/irlm/networker-tester/releases/latest";

    private readonly LatestVersionCache _cache;
    private readonly string _floor;
    private readonly IHttpClientFactory? _httpFactory;
    private readonly ILogger<VersionRefreshService> _logger;

    public VersionRefreshService(
        LatestVersionCache cache,
        string floorVersion,
        ILogger<VersionRefreshService> logger,
        IHttpClientFactory? httpFactory = null)
    {
        _cache = cache;
        _floor = floorVersion;
        _logger = logger;
        _httpFactory = httpFactory;
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        _logger.LogInformation(
            "latest-version refresh service started (base {Hours}h + up to {JitterMin}min jitter)",
            RefreshBase.TotalHours, RefreshJitterMaxSecs / 60);

        while (!stoppingToken.IsCancellationRequested)
        {
            // Sleep FIRST (Rust sleeps before the first refresh).
            var jitterSecs = Random.Shared.Next(0, RefreshJitterMaxSecs + 1); // inclusive both ends
            var delay = RefreshBase + TimeSpan.FromSeconds(jitterSecs);
            try
            {
                await Task.Delay(delay, stoppingToken).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                break;
            }

            try
            {
                var v = await RefreshNowAsync(stoppingToken).ConfigureAwait(false);
                _logger.LogInformation("latest-version refresh succeeded version={Version}", v);
            }
            catch (RateLimitedException)
            {
                _logger.LogDebug(
                    "latest-version refresh rate limited; using CARGO_PKG_VERSION floor");
            }
            catch (Exception ex)
            {
                _logger.LogWarning(ex, "latest-version refresh failed");
            }
        }
    }

    /// <summary>
    /// Rust <c>refresh_now</c>: fetch the remote latest tag, pick the higher of
    /// floor vs remote by semver, write it to the cache, return it. Any fetch
    /// error (including rate-limit) falls back to the floor. Always succeeds.
    /// </summary>
    public async Task<string> RefreshNowAsync(CancellationToken ct = default)
    {
        string resolved;
        try
        {
            var remote = await FetchGithubLatestAsync(ct).ConfigureAwait(false);
            resolved = PickHigherSemver(_floor, remote);
        }
        catch (Exception ex)
        {
            // Rust: on any fetch error, log DEBUG and fall back to the floor.
            _logger.LogDebug(ex, "github latest fetch failed; using floor {Floor}", _floor);
            resolved = _floor;
        }

        _cache.Set(resolved);
        return resolved;
    }

    /// <summary>
    /// Rust <c>fetch_github_latest</c>: GET the latest-release endpoint, treat
    /// 403/429 as <see cref="RateLimitedException"/>, read <c>tag_name</c>, strip a
    /// single leading <c>v</c>.
    /// </summary>
    public async Task<string> FetchGithubLatestAsync(CancellationToken ct = default)
    {
        var client = _httpFactory?.CreateClient() ?? new HttpClient();
        client.Timeout = TimeSpan.FromSeconds(15);
        using var request = new HttpRequestMessage(HttpMethod.Get, GithubLatestUrl);
        request.Headers.UserAgent.ParseAdd("networker-dashboard-version-refresh");

        using var response = await client.SendAsync(request, ct).ConfigureAwait(false);

        if (response.StatusCode is HttpStatusCode.Forbidden or HttpStatusCode.TooManyRequests)
        {
            throw new RateLimitedException();
        }

        if (!response.IsSuccessStatusCode)
        {
            throw new InvalidOperationException($"github latest returned {(int)response.StatusCode}");
        }

        using var doc = await JsonDocument
            .ParseAsync(await response.Content.ReadAsStreamAsync(ct).ConfigureAwait(false), cancellationToken: ct)
            .ConfigureAwait(false);

        if (!doc.RootElement.TryGetProperty("tag_name", out var tagEl) ||
            tagEl.ValueKind != JsonValueKind.String)
        {
            throw new InvalidOperationException("github response missing tag_name");
        }

        var tag = tagEl.GetString()!;
        return tag.StartsWith('v') ? tag[1..] : tag;
    }

    /// <summary>
    /// Rust <c>pick_higher_semver</c>: return the string whose parsed
    /// (major,minor,patch) tuple is higher; on parse ambiguity prefer the one
    /// that parses, else <paramref name="a"/>. Returns the original string
    /// (prefix preserved).
    /// </summary>
    public static string PickHigherSemver(string a, string? b)
    {
        if (b is null)
        {
            return a;
        }

        var pa = ParseSemver(a);
        var pb = ParseSemver(b);

        if (pa is not null && pb is not null)
        {
            return Compare(pb.Value, pa.Value) > 0 ? b : a;
        }

        if (pa is not null)
        {
            return a;
        }

        if (pb is not null)
        {
            return b;
        }

        return a;
    }

    private static int Compare((uint Major, uint Minor, uint Patch) x, (uint Major, uint Minor, uint Patch) y)
    {
        if (x.Major != y.Major)
        {
            return x.Major.CompareTo(y.Major);
        }

        if (x.Minor != y.Minor)
        {
            return x.Minor.CompareTo(y.Minor);
        }

        return x.Patch.CompareTo(y.Patch);
    }

    /// <summary>
    /// Rust <c>parse_semver</c>: strip leading <c>v</c>, split on <c>.</c>, parse
    /// major/minor as u32, and take the leading digits of the third part for
    /// patch (so <c>0.25.0-rc.1</c> =&gt; patch 0). Any failure =&gt; null.
    /// </summary>
    public static (uint Major, uint Minor, uint Patch)? ParseSemver(string s)
    {
        var v = s.StartsWith('v') ? s[1..] : s;
        var parts = v.Split('.');
        if (parts.Length < 3)
        {
            return null;
        }

        if (!uint.TryParse(parts[0], out var major))
        {
            return null;
        }

        if (!uint.TryParse(parts[1], out var minor))
        {
            return null;
        }

        // Patch: leading ASCII digits only (split on first non-digit).
        var patchPart = parts[2];
        var end = 0;
        while (end < patchPart.Length && char.IsAsciiDigit(patchPart[end]))
        {
            end++;
        }

        if (end == 0 || !uint.TryParse(patchPart[..end], out var patch))
        {
            return null;
        }

        return (major, minor, patch);
    }

    /// <summary>Rust typed <c>RateLimited</c> error (GitHub 403/429).</summary>
    public sealed class RateLimitedException : Exception
    {
        public RateLimitedException() : base("github latest rate limited") { }
    }
}

/// <summary>
/// DI wiring for the version-refresh loop. Registers the
/// <see cref="LatestVersionCache"/> singleton (seeded with the floor version) and
/// the <see cref="VersionRefreshService"/> hosted service.
///
/// <para><c>Program.cs</c> (the floor is the control plane's assembly version,
/// derived from Directory.Build.props — never a hardcoded string):
/// <code>builder.Services.AddVersionRefresh(VersionEndpoints.DashboardVersion);</code></para>
/// </summary>
public static class VersionRefreshExtensions
{
    public static IServiceCollection AddVersionRefresh(this IServiceCollection services, string floorVersion)
    {
        services.AddSingleton(new LatestVersionCache(floorVersion));
        services.AddHostedService(sp => new VersionRefreshService(
            sp.GetRequiredService<LatestVersionCache>(),
            floorVersion,
            sp.GetRequiredService<ILogger<VersionRefreshService>>(),
            sp.GetService<IHttpClientFactory>()));
        return services;
    }
}
