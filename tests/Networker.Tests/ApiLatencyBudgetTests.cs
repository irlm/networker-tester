using System.Globalization;
using System.Net;
using Networker.ControlPlane.Observability;
using Networker.Data.Entities;
using Xunit;

namespace Networker.Tests;

/// <summary>
/// Audit P1-10: nothing asserted that the control plane's read endpoints stay
/// fast, or even that they still report how long they took.
///
/// <para><b>Two different guarantees live here, and only one of them is about
/// time.</b></para>
///
/// <list type="number">
///   <item><b>The instrumentation contract (deterministic).</b> Every response
///   must carry <c>X-Process-Time-Ms</c>. The frontend's api client reads that
///   header to split each request into server vs network time; the control
///   plane didn't emit it for months, so every <c>perf_log</c> row had a null
///   <c>server_ms</c> and slowness could not be attributed to either side (perf
///   sweep 2026-07). A missing header is a hard failure with no timing
///   involved.</item>
///
///   <item><b>The budget (a blow-up detector, not a microbenchmark).</b>
///   Endpoints are driven against a SEEDED VOLUME and their own reported
///   server-side time must stay under a deliberately generous ceiling. This
///   exists to catch an N+1, a missing index, or an accidental full scan —
///   changes that cost orders of magnitude, not percentages. It is explicitly
///   NOT a performance regression detector: shared CI runners are noisy
///   neighbours, so anything tighter would flake and get muted, which is worse
///   than no check at all.</item>
/// </list>
///
/// <para>Timing noise is handled three ways: a warm-up call per route (the
/// first request pays JIT and EF model compilation and is not representative),
/// the MEDIAN of several samples rather than the mean, and server-reported time
/// rather than client wall-clock.</para>
/// </summary>
public class ApiLatencyBudgetTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fx;

    public ApiLatencyBudgetTests(ControlPlaneFixture fx) => _fx = fx;

    private const string Pid = ControlPlaneFixture.SeededProjectId;

    /// <summary>Per-request server-side ceiling. Generous on purpose — see the
    /// class docs. A healthy list endpoint over this volume answers in single
    /// or low double-digit milliseconds, so an order-of-magnitude regression
    /// still trips this while ordinary CI jitter never does.</summary>
    private const double BudgetMs = 1500.0;

    /// <summary>Rows seeded before measuring. Large enough that an N+1 or a
    /// sequential scan is visibly expensive, small enough to seed quickly.</summary>
    private const int SeededRuns = 300;

    private const int Samples = 5;

    private static readonly SemaphoreSlim SeedGate = new(1, 1);
    private static bool _seeded;

    /// <summary>Seed once per test run — the volume is shared and read-only for
    /// these tests, and reseeding per test would dominate the runtime.</summary>
    private async Task EnsureVolumeAsync()
    {
        await SeedGate.WaitAsync();
        try
        {
            if (_seeded)
            {
                return;
            }

            var now = DateTime.UtcNow;
            await using var db = _fx.NewDbContext();

            var configIds = new List<Guid>();
            for (var c = 0; c < 20; c++)
            {
                var cfgId = Guid.NewGuid();
                configIds.Add(cfgId);
                db.TestConfigs.Add(new TestConfig
                {
                    Id = cfgId,
                    ProjectId = Pid,
                    Name = $"latency-cfg-{c:D3}-{cfgId:N}",
                    EndpointKind = "network",
                    EndpointRef = """{"kind":"network","host":"10.30.0.1","port":8444}""",
                    Workload = """{"modes":["http1","http2","download"],"runs":5}""",
                    MaxDurationSecs = 900,
                    CreatedAt = now.AddDays(-30).AddMinutes(c),
                    UpdatedAt = now.AddDays(-30).AddMinutes(c),
                });
            }

            for (var i = 0; i < SeededRuns; i++)
            {
                var started = now.AddHours(-i);
                db.TestRuns.Add(new TestRun
                {
                    Id = Guid.NewGuid(),
                    TestConfigId = configIds[i % configIds.Count],
                    ProjectId = Pid,
                    Status = i % 17 == 0 ? "failed" : "completed",
                    SuccessCount = 40 + (i % 11),
                    FailureCount = i % 17 == 0 ? 3 : 0,
                    StartedAt = started,
                    FinishedAt = started.AddMinutes(4),
                    CreatedAt = started.AddMinutes(-1),
                });
            }

            await db.SaveChangesAsync();
            _seeded = true;
        }
        finally
        {
            SeedGate.Release();
        }
    }

    /// <summary>The server's own reported processing time for one request.</summary>
    private static double ServerMs(HttpResponseMessage resp)
    {
        Assert.True(resp.Headers.TryGetValues(ServerTiming.HeaderName, out var values),
            $"response carried no {ServerTiming.HeaderName} header — the frontend "
            + "cannot attribute latency to server vs network without it, and every "
            + "perf_log row's server_ms goes null (the 2026-07 regression)");

        var raw = values!.First();
        Assert.True(
            double.TryParse(raw, NumberStyles.Float, CultureInfo.InvariantCulture, out var ms),
            $"{ServerTiming.HeaderName} was not a parseable number: '{raw}'");
        return ms;
    }

    private static double Median(List<double> xs)
    {
        xs.Sort();
        return xs.Count % 2 == 1
            ? xs[xs.Count / 2]
            : (xs[(xs.Count / 2) - 1] + xs[xs.Count / 2]) / 2.0;
    }

    public static TheoryData<string> BudgetedRoutes() => new()
    {
        $"/api/v2/projects/{Pid}/test-runs?limit=50",
        $"/api/v2/projects/{Pid}/test-configs",
        $"/api/v2/projects/{Pid}/schedules",
        $"/api/projects/{Pid}/testers",
        $"/api/projects/{Pid}/deployments?limit=50",
        "/api/projects",
    };

    [Theory]
    [MemberData(nameof(BudgetedRoutes))]
    public async Task Read_endpoints_stay_within_the_server_time_budget(string route)
    {
        await EnsureVolumeAsync();
        using var client = _fx.CreateAdminClient();

        // Warm-up: the first call pays JIT + EF model compilation and says
        // nothing about steady-state cost.
        var warm = await client.GetAsync(route);
        Assert.Equal(HttpStatusCode.OK, warm.StatusCode);

        var samples = new List<double>(Samples);
        for (var i = 0; i < Samples; i++)
        {
            var resp = await client.GetAsync(route);
            Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
            samples.Add(ServerMs(resp));
        }

        var median = Median(samples);
        Assert.True(median < BudgetMs,
            $"{route} took {median:F1}ms server-side (median of {Samples}) against "
            + $"{SeededRuns} seeded runs, over the {BudgetMs}ms budget. That margin is "
            + "wide enough that CI noise does not reach it — look for an N+1, a "
            + $"missing index, or an unbounded scan. Samples: [{string.Join(", ", samples.Select(s => s.ToString("F1", CultureInfo.InvariantCulture)))}]");
    }

    [Fact]
    public async Task Every_response_reports_its_server_time()
    {
        // The deterministic half: no timing judgement, just the contract the
        // frontend depends on. Covers a write and an error response too — the
        // middleware sits before the error envelope, so both must be stamped.
        using var client = _fx.CreateAdminClient();

        var ok = await client.GetAsync("/api/projects");
        Assert.Equal(HttpStatusCode.OK, ok.StatusCode);
        Assert.True(ServerMs(ok) >= 0);

        var notFound = await client.GetAsync($"/api/projects/{Guid.NewGuid():N}/testers");
        Assert.True(ServerMs(notFound) >= 0,
            "an error response lost its timing header — the middleware must wrap "
            + "the error envelope, not sit inside it");

        using var anon = _fx.CreateClient();
        var unauthorized = await anon.GetAsync("/api/projects");
        Assert.True(ServerMs(unauthorized) >= 0,
            "an unauthenticated response lost its timing header");
    }

    [Fact]
    public async Task A_larger_page_does_not_cost_disproportionately_more()
    {
        // Shape check rather than an absolute one: 20x the rows must not cost
        // ~20x the time on a properly indexed, single-query list. Ratios are
        // far less noise-sensitive than absolute times, and this is the
        // signature an N+1 leaves behind. The allowance is deliberately loose.
        await EnsureVolumeAsync();
        using var client = _fx.CreateAdminClient();

        var route = $"/api/v2/projects/{Pid}/test-runs";
        await client.GetAsync($"{route}?limit=10");   // warm-up

        var small = new List<double>(Samples);
        var large = new List<double>(Samples);
        for (var i = 0; i < Samples; i++)
        {
            small.Add(ServerMs(await client.GetAsync($"{route}?limit=10")));
            large.Add(ServerMs(await client.GetAsync($"{route}?limit=200")));
        }

        var smallMs = Math.Max(Median(small), 1.0);   // floor: sub-ms ratios are meaningless
        var largeMs = Median(large);
        var ratio = largeMs / smallMs;

        Assert.True(ratio < 25.0,
            $"20x the rows cost {ratio:F1}x the server time ({smallMs:F1}ms → {largeMs:F1}ms). "
            + "A single indexed query should scale far better than that; this is what an "
            + "N+1 or a per-row lookup looks like.");
    }
}
